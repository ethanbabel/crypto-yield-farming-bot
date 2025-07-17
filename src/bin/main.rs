use dotenvy::dotenv;
use tracing::{info, error};
use std::collections::HashMap;
use std::sync::Arc;
use std::str::FromStr;

use crypto_yield_farming_bot::logging;
use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::data_ingestion::token::token_registry;
use crypto_yield_farming_bot::data_ingestion::market::market_registry;
use crypto_yield_farming_bot::gmx;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // Load environment variables from .env file
    dotenv()?;

    // Initialize logging
    if let Err(e) = logging::init_logging(env!("CARGO_BIN_NAME").to_string()) {
        eprintln!("Failed to initialize logging: {}", e);
        return Err(e.into());
    }

    // Load configuration (including provider)
    let cfg = config::Config::load().await;
    info!(network_mode = %cfg.network_mode, "Configuration loaded and logging initialized");

    // Initialize and populate token registry
    let mut token_registry = token_registry::AssetTokenRegistry::new(&cfg);
    if let Err(err) = token_registry.load_from_file() {
        error!(?err, "Failed to load asset tokens from file");
        return Err(err);
    }
    info!(count = token_registry.num_asset_tokens(), "Asset token registry initialized");

    // Initialize and populate market registry
    let mut market_registry = market_registry::MarketRegistry::new(&cfg);
    if let Err(err) = market_registry.populate(&cfg, &token_registry).await {
        error!(?err, "Failed to populate market registry");
        return Err(err);
    }
    info!(
        total_markets = market_registry.num_markets(),
        relevant_markets = market_registry.num_relevant_markets(),
        "Market registry populated"
    );

    // Fetch token prices
    if let Err(e) = token_registry.update_all_gmx_prices().await {
        error!(?e, "Failed to update asset token prices from GMX");
        return Err(e);
    }
    info!("Asset token prices updated from GMX");

    // Fetch market data using multicall
    let dummy_fee_map: HashMap<ethers::types::Address, gmx::event_listener_utils::MarketFees> = HashMap::new();
    if let Err(e) = market_registry.update_all_market_data(Arc::clone(&cfg), &dummy_fee_map).await {
        error!(?e, "Failed to update market data");
        return Err(e);
    }
    info!("Market data updated successfully");
    let address = ethers::types::Address::from_str("0x47c031236e19d024b42f8AE6780E44A573170703").unwrap();
    info!("Market info for address {}: {:?}", address, market_registry.get_market(&address));

    Ok(())
}