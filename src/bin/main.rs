use dotenvy::dotenv;

use crypto_yield_farming_bot::logging;
use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::token::token_registry;
use crypto_yield_farming_bot::market::market_registry;

use tracing::{info, error};
use std::collections::HashMap;
use std::sync::Arc;


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

    if let Err(e) = token_registry.update_all_gmx_prices().await {
        error!(?e, "Failed to update GMX prices");
        return Err(e.into());
    }

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

    let fee_map = HashMap::new();

    if let Err(err) = market_registry.update_all_market_data(Arc::clone(&cfg), &fee_map).await {
        error!(?err, "Failed to update market data");
        return Err(err);
    }

    market_registry.print_relevant_markets();

    Ok(())
}
