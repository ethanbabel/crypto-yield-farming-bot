use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use ndarray::Array2;

use super::types::{MarketStateSlice, TokenCategory};
use super::strategy_constants::JUMP_PARAMETERS;

/// Calculate covariance matrix from market slices with consistent ordering
pub fn calculate_covariance_matrix(market_slices: &[MarketStateSlice]) -> Option<Array2<f64>> {
    if market_slices.is_empty() {
        return None;
    }

    // Calculate historical covariance matrix
    let historical_cov = calculate_historical_covariance(market_slices)?;

    // Apply jump risk adjustments (proportional scaling)
    let jump_adjusted_cov = apply_jump_risk_scaling(historical_cov, market_slices);

    Some(jump_adjusted_cov)
}

/// Calculate historical covariance matrix from PnL returns
fn calculate_historical_covariance(market_slices: &[MarketStateSlice]) -> Option<Array2<f64>> {
        let n_markets = market_slices.len();
        
        // Extract PnL returns for each market
        let mut returns_matrix: Vec<Vec<f64>> = Vec::with_capacity(n_markets);
        let mut min_length = usize::MAX;

        for slice in market_slices {
            let returns = calculate_returns(slice)?;
            min_length = min_length.min(returns.len());
            returns_matrix.push(returns);
        }

        if min_length < 2 {
            return None;
        }

        // Truncate all return series to the same length
        for returns in &mut returns_matrix {
            returns.truncate(min_length);
        }

        // Calculate covariance matrix
        let mut cov_matrix = Array2::zeros((n_markets, n_markets));
        
        for i in 0..n_markets {
            for j in 0..n_markets {
                let covariance = calculate_covariance(&returns_matrix[i], &returns_matrix[j]);
                let net_oi_i = market_slices[i].oi_long_via_tokens - market_slices[i].oi_short_via_tokens;
                let net_oi_j = market_slices[j].oi_long_via_tokens - market_slices[j].oi_short_via_tokens;
                let pool_value_i = market_slices[i].pool_long_collateral_usd + market_slices[i].pool_short_collateral_usd - market_slices[i].impact_pool_usd;
                let pool_value_j = market_slices[j].pool_long_collateral_usd + market_slices[j].pool_short_collateral_usd - market_slices[j].impact_pool_usd;
                let net_oi_exposure_i = if pool_value_i > Decimal::ZERO {
                    (net_oi_i / pool_value_i).to_f64().unwrap_or(0.0)
                } else {
                    0.0
                };
                let net_oi_exposure_j = if pool_value_j > Decimal::ZERO {
                    (net_oi_j / pool_value_j).to_f64().unwrap_or(0.0)
                } else {
                    0.0
                };
                cov_matrix[[i, j]] = covariance * net_oi_exposure_i * net_oi_exposure_j;
            }
        }

        Some(cov_matrix)
    }

/// Calculate PnL returns for a single market from historical data
fn calculate_returns(slice: &MarketStateSlice) -> Option<Vec<f64>> {
        if slice.index_prices.len() < 2 {
            return None;
        }

        let current_pool_value = slice.pool_long_collateral_usd + slice.pool_short_collateral_usd - slice.impact_pool_usd;
        
        if current_pool_value <= Decimal::ZERO {
            return None;
        }

        let mut returns = Vec::new();

        for i in 1..slice.index_prices.len() {
            let p0 = slice.index_prices[i - 1];
            let p1 = slice.index_prices[i];
            
            if p0 > Decimal::ZERO && p1 > Decimal::ZERO {
                let token_return = ((p1 - p0) / p0).to_f64().unwrap_or(0.0);
                returns.push(token_return);
            }
        }

        if returns.is_empty() {
            None
        } else {
            Some(returns)
        }
    }

/// Calculate sample covariance between two return series
fn calculate_covariance(returns_x: &[f64], returns_y: &[f64]) -> f64 {
    if returns_x.len() != returns_y.len() || returns_x.len() < 2 {
        return 0.0;
    }

    let n = returns_x.len() as f64;
    let mean_x = returns_x.iter().sum::<f64>() / n;
    let mean_y = returns_y.iter().sum::<f64>() / n;

    let covariance = returns_x.iter()
        .zip(returns_y.iter())
        .map(|(x, y)| (x - mean_x) * (y - mean_y))
        .sum::<f64>() / (n - 1.0); // Sample covariance (divide by n-1)

    covariance
}

/// Apply jump risk scaling to the covariance matrix
fn apply_jump_risk_scaling(mut cov_matrix: Array2<f64>, market_slices: &[MarketStateSlice]) -> Array2<f64> {
    for (i, slice) in market_slices.iter().enumerate() {
        let token_category = get_token_category(slice);
        let jump_multiplier = calculate_jump_variance_multiplier(token_category);
        
        // Scale only the diagonal element (individual variance)
        // This preserves correlations while adding idiosyncratic jump risk
        cov_matrix[[i, i]] *= jump_multiplier;
    }

    cov_matrix
}

/// Calculate the variance multiplier from jump parameters
fn calculate_jump_variance_multiplier(token_category: TokenCategory) -> f64 {
    let (lambda_per_hour, alpha, beta) = get_jump_parameters(token_category);
    
    // For 5-minute intervals, scale lambda appropriately
    // 5 minutes = 1/12 hour, so lambda for 5-min period = lambda_per_hour / 12
    let lambda_5min = lambda_per_hour / 12.0;
    
    // Jump variance contribution: λ × E[Jump²]
    // Jump size ~ Normal(α×σ, β×σ), so E[Jump²] = (α×σ)² + (β×σ)² = σ² × (α² + β²)
    // Additional variance from jumps = λ × σ² × (α² + β²)
    // Total variance = original variance + jump variance = σ² × (1 + λ × (α² + β²))
    let jump_variance_multiplier = lambda_5min * (alpha * alpha + beta * beta);
    
    1.0 + jump_variance_multiplier  // 1.0 + additional jump risk
}

/// Get jump parameters for a token category
fn get_jump_parameters(tier: TokenCategory) -> (f64, f64, f64) {
    JUMP_PARAMETERS
        .iter()
        .find(|(category, _)| *category == tier)
        .map(|(_, params)| *params)
        .expect("Jump parameters not found for token category")
}

/// Determine token category from market slice
fn get_token_category(slice: &MarketStateSlice) -> TokenCategory {
    use super::strategy_constants::{BLUE_CHIP_MARKETS, MID_CAP_MARKETS};
    
    let address_str = slice.market_address.to_string();

    if BLUE_CHIP_MARKETS.contains(&address_str.as_str()) {
        TokenCategory::BlueChip
    } else if MID_CAP_MARKETS.contains(&address_str.as_str()) {
        TokenCategory::MidCap
    } else {
        TokenCategory::Unreliable
    }
}