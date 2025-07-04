use ethers::types::Address;

use super::{
    pnl_model, fee_model, allocator, 
    types::{
        MarketStateSlice, 
        MarketDiagnostics, 
        AllocationPlan,
        PnLSimulationConfig,
        TokenCategory,
    },
    strategy_constants::{
        BLUE_CHIP_MARKETS,
        MID_CAP_MARKETS,
    }
};
use crate::db::db_manager::DbManager;
use std::collections::HashMap;

/// Entry point for the strategy engine — run on each data refresh
pub async fn run_strategy_engine(db_manager: &DbManager) -> AllocationPlan {
    tracing::info!("Starting strategy engine...");

    // Fetch all data from DB
    let market_slices = fetch_market_state_slices(db_manager).await;

    // Run models on each market
    let mut diagnostics: HashMap<Address, MarketDiagnostics> = HashMap::new();

    for slice in &market_slices {
        let token_category = get_token_category(slice);

        let config = PnLSimulationConfig {
            time_horizon_hrs: 72,
            n_simulations: 10000,
            token_category,
        };

        let Some((pnl_return, pnl_var)) = pnl_model::simulate_trader_pnl(slice, &config) else { continue };
        let (fee_return, fee_var) = fee_model::analyze_fees(slice);

        let expected_return = pnl_return + fee_return;
        let variance = pnl_var + fee_var;

        diagnostics.insert(slice.market_address, MarketDiagnostics {
            market_address: slice.market_address,
            display_name: slice.display_name.clone(),
            expected_return,
            variance,
            pnl_return,
            pnl_variance: pnl_var,
            fee_return,
            fee_variance: fee_var,
        });
    }

    // Optimize allocations
    // let weights = allocator::optimize_allocations(&diagnostics);

    tracing::info!("Strategy engine completed.");

    // AllocationPlan { weights, diagnostics }
    AllocationPlan {
        weights: HashMap::new(),
        diagnostics,
    }
}

/// Fetch market state slices from the database
async fn fetch_market_state_slices(db_manager: &DbManager) -> Vec<MarketStateSlice> {
    let start = chrono::Utc::now() - chrono::Duration::days(30); // 30 days for now, to be adjusted later
    let end = chrono::Utc::now();
    
    match db_manager.get_market_state_slices(start, end).await {
        Ok(slices) => slices,
        Err(e) => {
            tracing::error!(err = ?e, "Failed to fetch market state slices");
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