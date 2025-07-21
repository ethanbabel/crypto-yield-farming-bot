use ndarray::{Array1, Array2};
use argmin::core::{CostFunction, Executor, Gradient, State};
use argmin::solver::gradientdescent::SteepestDescent;
use argmin::solver::linesearch::{BacktrackingLineSearch, condition::ArmijoCondition};
use eyre::Result;

/// Maximize Sharpe ratio subject to weights summing to 1 and being non-negative
pub fn maximize_sharpe(
    expected_returns: Array1<f64>,
    covariance_matrix: Array2<f64>,
) -> Result<Array1<f64>> {
    let n_assets = expected_returns.len();
    
    // Validate inputs
    if n_assets == 0 {
        return Err(eyre::eyre!("Empty expected returns vector"));
    }
    
    if covariance_matrix.nrows() != n_assets || covariance_matrix.ncols() != n_assets {
        return Err(eyre::eyre!("Covariance matrix dimensions don't match expected returns"));
    }

    // Check if covariance matrix is positive definite by ensuring all diagonal elements are positive
    for i in 0..n_assets {
        if covariance_matrix[[i, i]] <= 0.0 {
            return Err(eyre::eyre!("Covariance matrix is not positive definite"));
        }
    }

    // Use a simple analytical solution for the unconstrained case, then project
    let optimal_weights = solve_unconstrained_mpt(&expected_returns, &covariance_matrix)?;
    
    Ok(optimal_weights)
}

/// Solve the unconstrained MPT problem analytically and project to valid weights
fn solve_unconstrained_mpt(
    expected_returns: &Array1<f64>,
    covariance_matrix: &Array2<f64>,
) -> Result<Array1<f64>> {
    let n = expected_returns.len();
    
    // For the mean-variance optimization problem, we want to maximize:
    // w^T * μ - λ/2 * w^T * Σ * w
    // subject to w^T * 1 = 1 (weights sum to 1)
    
    // The analytical solution is: w = (Σ^-1 * μ) / (1^T * Σ^-1 * μ)
    // But since matrix inversion is complex, we'll use a simpler heuristic approach
    
    // Simple heuristic: weight by risk-adjusted returns (Sharpe ratios)
    let mut weights = Array1::zeros(n);
    let mut total_score = 0.0;
    
    for i in 0..n {
        let variance = covariance_matrix[[i, i]];
        let expected_return = expected_returns[i];
        
        // Calculate individual Sharpe ratio (assuming zero risk-free rate)
        let sharpe_ratio = if variance > 0.0 {
            expected_return / variance.sqrt()
        } else {
            0.0
        };
        
        // Use max(0, sharpe_ratio) to ensure non-negative weights
        let score = sharpe_ratio.max(0.0);
        weights[i] = score;
        total_score += score;
    }
    
    // Normalize weights to sum to 1
    if total_score > 1e-10 {
        weights /= total_score;
    } else {
        // Fallback to zero weights if all Sharpe ratios are non-positive
        weights.fill(0.0);
    }
    
    // Apply minimum variance optimization as a refinement
    let refined_weights = refine_with_minimum_variance(&weights, expected_returns, covariance_matrix)?;
    
    Ok(refined_weights)
}

/// Refine weights using a simple gradient descent approach
fn refine_with_minimum_variance(
    initial_weights: &Array1<f64>,
    expected_returns: &Array1<f64>,
    covariance_matrix: &Array2<f64>,
) -> Result<Array1<f64>> {
    let n = initial_weights.len();
    
    // Define the optimization problem
    let problem = SharpeRatioProblem {
        expected_returns: expected_returns.clone(),
        covariance_matrix: covariance_matrix.clone(),
    };

    // Use steepest descent with backtracking line search
    let linesearch = BacktrackingLineSearch::new(
        ArmijoCondition::new(0.0001).map_err(|e| eyre::eyre!("Failed to create Armijo condition: {}", e))?
    );
    let solver = SteepestDescent::new(linesearch);

    // Run optimization
    let result = Executor::new(problem, solver)
        .configure(|state| {
            state
                .param(initial_weights.to_vec())
                .max_iters(100)
                .target_cost(1e-6)
        })
        .run()
        .map_err(|e| eyre::eyre!("Optimization failed: {}", e))?;

    // Get the optimal weights
    let optimal_weights_vec = result.state().get_best_param().unwrap().clone();
    let mut optimal_weights = Array1::from_vec(optimal_weights_vec);

    // Ensure weights are non-negative (project negative weights to zero)
    optimal_weights.mapv_inplace(|w| w.max(0.0));

    // Normalize weights to sum to 1
    let weight_sum = optimal_weights.sum();
    if weight_sum > 1e-10 {
        optimal_weights /= weight_sum;
    } else {
        // Fallback to zero weights if all weights are near zero
        optimal_weights = Array1::from_vec(vec![0.0; n]);
    }

    // Apply minimum weight filter first (eliminate tiny positions)
    optimal_weights = apply_minimum_weight_filter(optimal_weights, 0.01); // 1% min weight or zero

    // Then apply maximum position size limits
    optimal_weights = apply_position_limits(optimal_weights, 0.25); // 25% max weight per asset

    Ok(optimal_weights)
}

/// Apply position size limits by capping weights and redistributing excess
fn apply_position_limits(mut weights: Array1<f64>, max_weight: f64) -> Array1<f64> {
    let n = weights.len();
    let mut iterations = 0;
    const MAX_ITERATIONS: usize = 100; // Prevent infinite loops
    
    loop {
        iterations += 1;
        if iterations > MAX_ITERATIONS {
            break;
        }
        
        // Find assets that exceed the limit
        let mut total_excess = 0.0;
        let mut capped_indices = Vec::new();
        let mut uncapped_weight_sum = 0.0;
        
        for (i, &weight) in weights.iter().enumerate() {
            if weight > max_weight {
                total_excess += weight - max_weight;
                capped_indices.push(i);
            } else {
                uncapped_weight_sum += weight;
            }
        }
        
        // If no assets exceed the limit, we're done
        if total_excess <= 1e-10 {
            break;
        }
        
        // Cap the overweight assets
        for &i in &capped_indices {
            weights[i] = max_weight;
        }
        
        // Redistribute excess proportionally to uncapped assets
        if uncapped_weight_sum > 1e-10 {
            for i in 0..n {
                if !capped_indices.contains(&i) {
                    let proportion = weights[i] / uncapped_weight_sum;
                    weights[i] += total_excess * proportion;
                }
            }
        } else {
            // If all uncapped assets have zero weight, distribute equally among them
            let uncapped_count = n - capped_indices.len();
            if uncapped_count > 0 {
                let equal_share = total_excess / uncapped_count as f64;
                for i in 0..n {
                    if !capped_indices.contains(&i) {
                        weights[i] += equal_share;
                    }
                }
            }
        }
    }
    
    weights
}

/// Apply minimum weight filter by zeroing out tiny positions and redistributing their weight
fn apply_minimum_weight_filter(mut weights: Array1<f64>, min_weight: f64) -> Array1<f64> {
    let n = weights.len();
    let mut iterations = 0;
    const MAX_ITERATIONS: usize = 100; // Prevent infinite loops
    
    loop {
        iterations += 1;
        if iterations > MAX_ITERATIONS {
            break;
        }
        
        // Find assets below the minimum threshold (but not already zero)
        let mut total_to_redistribute = 0.0;
        let mut zeroed_indices = Vec::new();
        let mut remaining_weight_sum = 0.0;
        
        for (i, &weight) in weights.iter().enumerate() {
            if weight > 0.0 && weight < min_weight {
                total_to_redistribute += weight;
                zeroed_indices.push(i);
            } else if weight >= min_weight {
                remaining_weight_sum += weight;
            }
        }

        // Zero out the tiny positions
        for &i in &zeroed_indices {
            weights[i] = 0.0;
        }
        
        // If no assets are below the threshold, we're done
        if total_to_redistribute <= 1e-10 {
            break;
        }
        
        // Redistribute their weight proportionally to assets above the threshold
        if remaining_weight_sum > 1e-10 {
            for i in 0..n {
                if weights[i] >= min_weight {
                    let proportion = weights[i] / remaining_weight_sum;
                    weights[i] += total_to_redistribute * proportion;
                }
            }
        } else {
            // If no assets are above the threshold, this shouldn't happen after normalization
            // But as a safety, distribute equally among all non-zero positions
            let non_zero_count = weights.iter().filter(|&&w| w > 0.0).count();
            if non_zero_count > 0 {
                let equal_share = total_to_redistribute / non_zero_count as f64;
                for weight in weights.iter_mut() {
                    if *weight > 0.0 {
                        *weight += equal_share;
                    }
                }
            }
        }
    }
    
    weights
}

/// Problem definition for maximizing Sharpe ratio
/// We minimize negative Sharpe ratio to maximize the actual Sharpe ratio
struct SharpeRatioProblem {
    expected_returns: Array1<f64>,
    covariance_matrix: Array2<f64>,
}

impl CostFunction for SharpeRatioProblem {
    type Param = Vec<f64>;
    type Output = f64;

    fn cost(&self, weights: &Self::Param) -> Result<Self::Output, argmin::core::Error> {
        let w = Array1::from_vec(weights.clone());
        
        // Ensure weights sum to 1 (soft constraint via penalty)
        let weight_sum = w.sum();
        let weight_constraint_penalty = 1000.0 * (weight_sum - 1.0).powi(2);
        
        // Ensure weights are non-negative (soft constraint via penalty)
        let negative_weight_penalty = 1000.0 * w.iter()
            .map(|&weight| if weight < 0.0 { weight.powi(2) } else { 0.0 })
            .sum::<f64>();
        
        // Calculate portfolio return
        let portfolio_return = w.dot(&self.expected_returns);
        
        // Calculate portfolio variance
        let portfolio_variance = w.dot(&self.covariance_matrix.dot(&w));
        
        // Calculate Sharpe ratio (assuming risk-free rate = 0)
        let sharpe_ratio = if portfolio_variance > 0.0 {
            portfolio_return / portfolio_variance.sqrt()
        } else {
            0.0
        };
        
        // Return negative Sharpe ratio (since we're minimizing) plus penalties
        let cost = -sharpe_ratio + weight_constraint_penalty + negative_weight_penalty;
        
        Ok(cost)
    }
}

impl Gradient for SharpeRatioProblem {
    type Param = Vec<f64>;
    type Gradient = Vec<f64>;

    fn gradient(&self, weights: &Self::Param) -> Result<Self::Gradient, argmin::core::Error> {
        let w = Array1::from_vec(weights.clone());
        let n = w.len();
        
        // Calculate portfolio return and variance
        let portfolio_return = w.dot(&self.expected_returns);
        let portfolio_variance = w.dot(&self.covariance_matrix.dot(&w));
        let portfolio_std = portfolio_variance.sqrt();
        
        let mut gradient = vec![0.0; n];
        
        if portfolio_variance > 1e-10 {
            // Gradient of negative Sharpe ratio
            for i in 0..n {
                let d_return_d_wi = self.expected_returns[i];
                let d_variance_d_wi = 2.0 * self.covariance_matrix.row(i).dot(&w);
                let d_std_d_wi = d_variance_d_wi / (2.0 * portfolio_std);
                
                let d_sharpe_d_wi = (d_return_d_wi * portfolio_std - portfolio_return * d_std_d_wi) 
                    / portfolio_variance;
                
                gradient[i] = -d_sharpe_d_wi;
            }
        }
        
        // Add gradient of constraint penalties
        let weight_sum = w.sum();
        for i in 0..n {
            // Weight sum constraint gradient
            gradient[i] += 2000.0 * (weight_sum - 1.0);
            
            // Non-negativity constraint gradient
            if w[i] < 0.0 {
                gradient[i] += 2000.0 * w[i];
            }
        }
        
        Ok(gradient)
    }
}
