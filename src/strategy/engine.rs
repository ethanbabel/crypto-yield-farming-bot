use tracing::{instrument, debug, info, error};
use eyre::Result;
use std::sync::Arc;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use ndarray::Array1;

use super::{
    fee_model, allocator, covariance,
    types::{
        MarketStateSlice, 
        PortfolioData,
    },
};
use crate::db::db_manager::DbManager;
use crate::hedging::dydx_client::DydxClient;

/// Entry point for the strategy engine — run on each data refresh
#[instrument(name = "strategy_engine", skip(db_manager, dydx_client))]
pub async fn run_strategy_engine(db_manager: Arc<DbManager>, dydx_client: Arc<DydxClient>) -> Result<PortfolioData> {
    info!("Starting strategy engine...");

    // Fetch all data from DB
    let market_slices = fetch_market_state_slices(db_manager).await?;

    if market_slices.is_empty() {
        error!("No market slices fetched from database");
        return Err(eyre::eyre!("No market slices fetched from database"));
    }

    // Filter market slices
    let mut filtered_markets = String::new();
    let market_slices: Vec<MarketStateSlice> = market_slices
        .into_iter()
        .filter(|slice| {

            let name = &slice.display_name;
            // Filter out slices without at least one day of market observations
            if slice.timestamps.len() < 288 {
                filtered_markets.push_str(&format!("{} --> insufficient market timestamps ({} < 288)\n", name, slice.timestamps.len()));
                return false;
            }
            // Filter out slices without at least one day of index token prices
            if slice.index_prices.len() < 288 {
                filtered_markets.push_str(&format!("{} --> insufficient index token timestamps ({} < 288)\n", name, slice.index_prices.len()));
                return false;
            }
            // Filter out slices where the oldest market timestamp is too recent 
            if !slice.timestamps.first().map_or(false, |t| *t < (chrono::Utc::now() - chrono::Duration::days(1))) {
                filtered_markets.push_str(&format!("{} --> oldest market timestamp too recent ({:?})\n", name, slice.timestamps.first()));
                return false;
            }
            // Filter out slices where the oldest index token timestamp is too recent
            if !slice.index_token_timestamps.first().map_or(false, |t| *t < (chrono::Utc::now() - chrono::Duration::days(1))) {
                filtered_markets.push_str(&format!("{} --> oldest index token timestamp too recent ({:?})\n", name, slice.index_token_timestamps.first()));
                return false;
            }
            // Filter out slices where the newest market timestamp is too old
            if !slice.timestamps.last().map_or(false, |t| *t > (chrono::Utc::now() - chrono::Duration::hours(1))) {
                filtered_markets.push_str(&format!("{} --> newest market timestamp too old ({:?})\n", name, slice.timestamps.last()));
                return false;
            }
            // Filter out slices where the newest index token timestamp is too old
            if !slice.index_token_timestamps.last().map_or(false, |t| *t > (chrono::Utc::now() - chrono::Duration::hours(1))) {
                filtered_markets.push_str(&format!("{} --> newest index token timestamp too old ({:?})\n", name, slice.index_token_timestamps.last()));
                return false;
            }
            // Filter out slices without high enough total OI
            let total_oi = slice.oi_long + slice.oi_short;
            if total_oi <= Decimal::from_f64(10000.0).unwrap() {
                filtered_markets.push_str(&format!("{} --> insufficient total OI ({} <= 10000)\n", name, total_oi));
                return false;
            }
            
            true
        })
        .collect();
    if market_slices.is_empty() {
        error!("All markets filtered out:\n{}", filtered_markets);
        return Err(eyre::eyre!("All markets filtered out"));
    } else {
        debug!("Filtered out markets: {} available\nRemoved markets:\n{}", market_slices.len(), filtered_markets);
    }

    // Calculate covariance matrix using the same ordering as market_slices
    let covariance_matrix = match covariance::calculate_covariance_matrix(&market_slices) {
        Some(matrix) => matrix,
        None => {
            error!("Failed to calculate covariance matrix");
            return Err(eyre::eyre!("Failed to calculate covariance matrix"));
        }
    };
    debug!("Covariance matrix calculated");

    let n_markets = market_slices.len();
    let mut market_addresses = Vec::with_capacity(n_markets);
    let mut display_names = Vec::with_capacity(n_markets);
    let mut expected_returns = Array1::zeros(n_markets);

    let token_hedgeinfo_map = dydx_client.get_token_hedgeinfo_map().await?;

    // Run models on each market sequentially to respect rate limits
    for (i, slice) in market_slices.iter().enumerate() {
        market_addresses.push(slice.market_address);
        display_names.push(slice.display_name.clone());
        
        let fee_return = fee_model::simulate_fee_return(&slice).unwrap_or(Decimal::ZERO);
        
        let (long_token_symbol, short_token_symbol) = get_collateral_tokens_from_display_name(slice.display_name.clone())?;
        let long_token_hedgeinfo_opt = token_hedgeinfo_map.get(&long_token_symbol);
        let short_token_hedgeinfo_opt = token_hedgeinfo_map.get(&short_token_symbol);
        if let Some(Some(long_token_hedgeinfo)) = long_token_hedgeinfo_opt {
            let exposed_capital_frac = if short_token_hedgeinfo_opt.unwrap_or(&None).is_some() {
                Decimal::ONE // short token is stablecoin
            } else {
                Decimal::from_str("0.5").unwrap() // short token is not stablecoin
            };
            let funding_rate = long_token_hedgeinfo.0;
            let leverage = long_token_hedgeinfo.1;
            let funding_cost = exposed_capital_frac * (- funding_rate); // funding rate > 0 ==> longs pay shorts ==> income for our short position
            let opportunity_cost = (exposed_capital_frac / leverage) * fee_return;
            let total_return = fee_return - funding_cost - opportunity_cost;
            info!(
                market = %slice.display_name,
                fee_return = %fee_return,
                fee_return_annualized = %((fee_return * Decimal::from_f64(24.0 * 365.0).unwrap())),
                exposed_capital_frac = %exposed_capital_frac,
                leverage = %leverage,
                funding_rate = %funding_rate,
                funding_cost = %funding_cost,
                funding_cost_annualized = %((funding_cost * Decimal::from_f64(24.0 * 365.0).unwrap())),
                opportunity_cost = %opportunity_cost,
                opportunity_cost_annualized = %((opportunity_cost * Decimal::from_f64(24.0 * 365.0).unwrap())),
                total_return = %total_return,
                total_return_annualized = %((total_return * Decimal::from_f64(24.0 * 365.0).unwrap())),
                "Adjusted expected returns with hedge"
            );
            expected_returns[i] = total_return;
        } else {
            expected_returns[i] = fee_return; // No hedge info for long token indicates stablecoin or unsupported by dYdX
            continue;
        }
    }
    debug!("Market returns calculated");

    // Create PortfolioData with consistent ordering
    let weights = allocator::maximize_sharpe(expected_returns.clone(), covariance_matrix.clone())?;

    debug!("Optimal portfolio weights calculated");

    let portfolio_data = PortfolioData::new(market_addresses, display_names, expected_returns, covariance_matrix, weights);

    Ok(portfolio_data)
}

/// Fetch market state slices from the database
#[instrument(name = "fetch_market_state_slices", skip(db_manager))]
async fn fetch_market_state_slices(db_manager: Arc<DbManager>) -> Result<Vec<MarketStateSlice>> {
    let start = chrono::Utc::now() - chrono::Duration::days(30); // 30 days for now, to be adjusted later
    let end = chrono::Utc::now();
    
    let slices = db_manager.get_market_state_slices(start, end).await?;
    Ok(slices)
}

// --- HELPERS ---

fn get_collateral_tokens_from_display_name(display_name: String) -> Result<(String, String)> {
    let collateral_tokens_start_idx = display_name.find('[')
        .ok_or_else(|| eyre::eyre!("Invalid display name format: {}", display_name))? + 1;
    let collateral_tokens_substr = &display_name[collateral_tokens_start_idx..display_name.len()-1];
    let (long_token, short_token) = collateral_tokens_substr.split_once('-')
        .ok_or_else(|| eyre::eyre!("Invalid collateral token format in display name: {}", display_name))?;
    Ok((long_token[..long_token.len()-1].to_string(), short_token[1..].to_string()))
}
