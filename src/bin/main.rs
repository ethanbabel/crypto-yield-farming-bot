use dotenvy::dotenv;
use eyre::Result;
use tracing::{debug, info, error};
use std::sync::Arc;

use crypto_yield_farming_bot::logging;
use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::data_ingestion::token::token_registry;
use crypto_yield_farming_bot::data_ingestion::market::market_registry;

#[tokio::main]
async fn main() -> Result<()> {
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

    // Initialize token registry
    let mut token_registry = token_registry::AssetTokenRegistry::new(&cfg);
    info!("Asset token registry initialized");

    // Initialize market registry
    let mut market_registry = market_registry::MarketRegistry::new(&cfg);
    info!("Market registry initialized");

    // Repopulate the market registry and get new tokens/markets
    let (_new_tokens, _new_market_addresses) = match market_registry.repopulate(&cfg, &mut token_registry).await {
        Ok(result) => result,
        Err(e) => {
            error!(?e, "Failed to repopulate market registry");
            return Err(e);
        }
    };

    // Fetch Asset Token price data from GMX
    if let Err(e) = token_registry.update_all_gmx_prices().await {
        error!(?e, "Failed to update asset token prices from GMX");
        return Err(e);
    }
    debug!("Asset token prices updated from GMX");

    // Update market data
    if let Err(e) = market_registry.update_all_market_data(Arc::clone(&cfg), &Default::default()).await {
        error!(?e, "Failed to update market data");
        return Err(e);
    }

    tokio::time::sleep(std::time::Duration::from_secs(3)).await; // Allow time for logging to flush
    Ok(())
}
