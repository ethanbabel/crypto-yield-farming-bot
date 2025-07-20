use super::types::{
    MarketStateSlice, 
    TokenCategory, 
    PnLSimulationConfig, 
};
use super::strategy_constants::JUMP_PARAMETERS;

use rust_decimal::prelude::*;
use rand::{Rng, rng};
use rand_distr::{Normal, Poisson, Distribution};
use std::f64::consts::E;
use chrono::{DateTime, Utc};

/// Top-level simulation entry point
pub fn simulate_trader_pnl(
    slice: &MarketStateSlice,
    config: &PnLSimulationConfig,
) -> Option<Decimal> {
    // Get index token returns
    let returns = get_returns(&slice.index_prices)?;
    if returns.len() < 2 {
        return None;
    }

    // Compute S0, mu and sigma
    let (s0, mu, sigma) = compute_gbm_params(&returns, &slice.index_token_timestamps, &slice.index_prices)?;

    // Set up jump parameters
    let (lambda_per_hour, alpha, beta) = get_jump_parameters(config.token_category);
    let lambda = lambda_per_hour * (config.time_horizon_hrs as f64);

    // Simulate paths --> Model token price (rather than pnl directly) to allow for future path dependent sims
    let final_prices = simulate_paths(config.n_simulations, config.time_horizon_hrs, s0, mu, sigma, lambda, alpha, beta);

    // Get trader-side PnL
    let trader_total_pnl_long_samples = final_prices
        .iter()
        .map(|final_price| (Decimal::from_f64(*final_price).unwrap() * slice.oi_long_token_amount) - slice.oi_long)
        .collect::<Vec<Decimal>>();
    let trader_total_pnl_short_samples = final_prices
        .iter()
        .map(|final_price| slice.oi_short - (Decimal::from_f64(*final_price).unwrap() * slice.oi_short_token_amount))
        .collect::<Vec<Decimal>>();

    // Get current PnL and pool composition for return calculations
    let cur_net_pnl = slice.pnl_net;
    let cur_pool_value = slice.pool_long_collateral_usd + slice.pool_short_collateral_usd - slice.impact_pool_usd;

    if cur_pool_value <= Decimal::ZERO {
        return None; 
    }
    
    // Get LP-side net PnL
    let lp_net_pnl_delta_samples = trader_total_pnl_long_samples
        .iter()
        .zip(trader_total_pnl_short_samples.iter())
        .map(|(long, short)| 
            -((*long + *short) - cur_net_pnl) 
        )
        .collect::<Vec<Decimal>>();
    let lp_pnl_return_samples = lp_net_pnl_delta_samples
        .iter()
        .map(|dollar_return| {
            if cur_pool_value > Decimal::ZERO {
                dollar_return / cur_pool_value
            } else {
                Decimal::ZERO
            }
        })
        .collect::<Vec<Decimal>>();

    // Calculate expected return and variance for LP-side PnL
    let n = lp_pnl_return_samples.len() as f64;
    let expected_return = lp_pnl_return_samples.iter().cloned().sum::<Decimal>() / Decimal::from_f64(n).unwrap();

    // Scale to 5 min
    let expected_return = expected_return / Decimal::from_f64(12.0 * config.time_horizon_hrs as f64).unwrap();

    Some(expected_return)
}

/// Extract regular returns from price vector
fn get_returns(prices: &[Decimal]) -> Option<Vec<f64>> {
    if prices.len() < 2 {
        return None;
    }

    let mut returns = Vec::with_capacity(prices.len() - 1);
    for i in 1..prices.len() {
        let p0 = prices[i - 1];
        let p1 = prices[i];
        if p0 > Decimal::ZERO && p1 > Decimal::ZERO {
            let ret = ((p1 - p0) / p0).to_f64().unwrap();
            returns.push(ret);
        }
    }

    Some(returns)
}

/// Compute starting price, drift and volatility from returns, standardized to hourly
fn compute_gbm_params(returns: &[f64], timestamps: &[DateTime<Utc>], prices: &[Decimal]) -> Option<(f64, f64, f64)> {
    let n = returns.len() as f64;
    if n < 2.0 || timestamps.len() < 2 || prices.is_empty() {
        return None;
    }

    // Get the most recent price as starting price
    let s0 = prices[prices.len() - 1];
    let s0 = s0.to_f64().unwrap();

    // Calculate mean and variance of returns
    let mean = returns.iter().sum::<f64>() / n;
    let var = returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / n;
    let sigma = var.sqrt();

    // Calculate time interval between observations in hours
    let total_duration = timestamps[timestamps.len() - 1] - timestamps[0];
    let avg_interval_hours = total_duration.num_minutes() as f64 / 60.0 / (timestamps.len() - 1) as f64;

    // Standardize to hourly
    let mu_hourly = mean * (1.0 / avg_interval_hours);
    let sigma_hourly = sigma * (1.0 / avg_interval_hours).sqrt();

    Some((s0, mu_hourly, sigma_hourly))
}

/// Get jump frequency and size parameters from token category
fn get_jump_parameters(tier: TokenCategory) -> (f64, f64, f64) {
    JUMP_PARAMETERS
        .iter()
        .find(|(category, _)| *category == tier)
        .map(|(_, params)| *params)
        .expect("Jump parameters not found for token category")
}

/// Simulate GBM + Poisson jumps per path
fn simulate_paths(
    n_paths: usize,
    horizon_hrs: u64,
    s0: f64,
    mu: f64,
    sigma: f64,
    lambda: f64,
    alpha: f64,
    beta: f64,
) -> Vec<f64> {
    let mut rng = rng();
    let mut final_prices = Vec::with_capacity(n_paths);

    let poisson = Poisson::new(lambda).unwrap();
    let normal = Normal::new(0.0, 1.0).unwrap();
    let jump_magnitude = Normal::new(alpha * sigma, beta * sigma).unwrap();

    for _ in 0..n_paths {
        let mut current_price = s0;

        // GBM path - simulate hour by hour
        for _ in 0..horizon_hrs {
            let z = normal.sample(&mut rng);
            current_price = current_price * E.powf((mu - 0.5 * sigma * sigma) + sigma * z);
        }

        // Jump events
        let num_jumps = poisson.sample(&mut rng) as usize;
        for _ in 0..num_jumps {
            let mag = jump_magnitude.sample(&mut rng).abs();
            let sign = if rng.random_bool(0.5) { 1.0 } else { -1.0 };
            current_price = current_price * E.powf(sign * mag);
        }

        final_prices.push(current_price);
    }

    final_prices
}