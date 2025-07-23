use tracing::{debug, info, error};
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use ndarray::Array1;
use rayon::prelude::*;

use super::{
    pnl_model, fee_model, allocator, covariance,
    types::{
        MarketStateSlice, 
        PnLSimulationConfig,
        TokenCategory,
        PortfolioData,
    },
    strategy_constants::{
        TIME_HORIZON_HRS,
        BLUE_CHIP_MARKETS,
        MID_CAP_MARKETS,
    }
};
use crate::db::db_manager::DbManager;

/// Entry point for the strategy engine — run on each data refresh
pub async fn run_strategy_engine(db_manager: &DbManager) -> Option<PortfolioData> {
    info!("Starting strategy engine...");

    // Fetch all data from DB
    let market_slices = fetch_market_state_slices(db_manager).await;

    if market_slices.is_empty() {
        tracing::warn!("No market slices available");
        return None;
    }

    // // Filter market slices
    let market_slices: Vec<MarketStateSlice> = market_slices
        .into_iter()
        .filter(|slice| {
            // Filter out slices without at least one day of market observations
            slice.timestamps.len() >= 288 &&
            // Filter out slices without at least one day of index token prices
            slice.index_prices.len() >= 288 &&
            // Filter out slices where the oldest market timestamp is too recent 
            slice.timestamps.first().map_or(false, |t| *t < (chrono::Utc::now() - chrono::Duration::days(1))) &&
            // Filter out slices where the oldest index token timestamp is too recent
            slice.index_token_timestamps.first().map_or(false, |t| *t < (chrono::Utc::now() - chrono::Duration::days(1))) &&
            // Filter out slices where the newest market timestamp is too old
            slice.timestamps.last().map_or(false, |t| *t > (chrono::Utc::now() - chrono::Duration::hours(1))) &&
            // Filter out slices where the newest index token timestamp is too old
            slice.index_token_timestamps.last().map_or(false, |t| *t > (chrono::Utc::now() - chrono::Duration::hours(1))) &&
            // Filter out slices without high enough total OI
            slice.oi_long + slice.oi_short > Decimal::from_f64(10000.0).unwrap() 
        })
        .collect();
    if market_slices.is_empty() {
        tracing::warn!("No market slices passed filtering criteria");
        return None;
    }
    debug!("Filtered market slices: {} available", market_slices.len());

    // Calculate covariance matrix using the same ordering as market_slices
    let covariance_matrix = match covariance::calculate_covariance_matrix(&market_slices) {
        Some(matrix) => matrix,
        None => {
            error!("Failed to calculate covariance matrix");
            return None;
        }
    };
    debug!("Covariance matrix calculated");

    let n_markets = market_slices.len();
    let mut market_addresses = Vec::with_capacity(n_markets);
    let mut display_names = Vec::with_capacity(n_markets);
    let mut expected_returns = Array1::zeros(n_markets);

    // Run models on each market in parallel - maintaining same ordering as covariance matrix
    let results: Vec<(usize, f64)> = market_slices
        .par_iter()
        .enumerate()
        .map(|(i, slice)| {
            let token_category = get_token_category(slice);

            let config = PnLSimulationConfig {
                time_horizon_hrs: TIME_HORIZON_HRS,
                n_simulations: 10000,
                token_category,
            };

            let pnl_return = pnl_model::simulate_trader_pnl(slice, &config).unwrap_or(Decimal::ZERO);
            let fee_return = fee_model::simulate_fee_return(slice).unwrap_or(Decimal::ZERO);

            let total_return = (pnl_return + fee_return).to_f64().unwrap_or(0.0);
            
            (i, total_return)
        })
        .collect();

    // Populate arrays maintaining the original market_slices order
    for slice in market_slices.iter() {
        market_addresses.push(slice.market_address);
        display_names.push(slice.display_name.clone());
    }
    
    // Set expected returns using the parallel computation results
    for (i, return_value) in results {
        expected_returns[i] = return_value;
    }
    debug!("Market returns calculated");

    // Create PortfolioData with consistent ordering
    let weights = match allocator::maximize_sharpe(expected_returns.clone(), covariance_matrix.clone()) {
        Ok(weights) => weights,
        Err(e) => {
            error!("Failed to solve MPT QP: {}", e);
            return None;
        }
    };

    info!("Optimal portfolio weights calculated");

    let portfolio_data = PortfolioData::new(market_addresses, display_names, expected_returns, covariance_matrix, weights);
    info!("Strategy engine completed.");

    Some(portfolio_data)
}

/// Fetch market state slices from the database
async fn fetch_market_state_slices(db_manager: &DbManager) -> Vec<MarketStateSlice> {
    let start = chrono::Utc::now() - chrono::Duration::days(30); // 30 days for now, to be adjusted later
    let end = chrono::Utc::now();
    
    match db_manager.get_market_state_slices(start, end).await {
        Ok(slices) => slices,
        Err(e) => {
            error!(err = ?e, "Failed to fetch market state slices");
            Vec::new()
        }
    }
}

fn get_token_category(slice: &MarketStateSlice) -> TokenCategory {
    let address_str = slice.market_address.to_string();

    if BLUE_CHIP_MARKETS.contains(&address_str.as_str()) {
        TokenCategory::BlueChip
    } else if MID_CAP_MARKETS.contains(&address_str.as_str()) {
        TokenCategory::MidCap
    } else {
        TokenCategory::Unreliable
    }
}