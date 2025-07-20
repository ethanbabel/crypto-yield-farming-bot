use tracing::{info, error};
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use ndarray::Array1;

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

    // Calculate covariance matrix using the same ordering as market_slices
    let covariance_matrix = match covariance::calculate_covariance_matrix(&market_slices) {
        Some(matrix) => matrix,
        None => {
            error!("Failed to calculate covariance matrix");
            return None;
        }
    };

    let n_markets = market_slices.len();
    let mut market_addresses = Vec::with_capacity(n_markets);
    let mut display_names = Vec::with_capacity(n_markets);
    let mut expected_returns = Array1::zeros(n_markets);

    // Run models on each market - this ensures same ordering as covariance matrix
    for (i, slice) in market_slices.iter().enumerate() {
        let token_category = get_token_category(slice);

        let config = PnLSimulationConfig {
            time_horizon_hrs: TIME_HORIZON_HRS,
            n_simulations: 10000,
            token_category,
        };

        let pnl_return = pnl_model::simulate_trader_pnl(slice, &config).unwrap_or(Decimal::ZERO);
        let fee_return = fee_model::simulate_fee_return(slice).unwrap_or(Decimal::ZERO);

        market_addresses.push(slice.market_address);
        display_names.push(slice.display_name.clone());
        expected_returns[i] = (pnl_return + fee_return).to_f64().unwrap_or(0.0);
    }

    // Create PortfolioData with consistent ordering
    let portfolio_data = PortfolioData::new(market_addresses, display_names, expected_returns, covariance_matrix);

    // Optimize allocations
    // let weights = allocator::optimize_allocations(&portfolio_data);

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