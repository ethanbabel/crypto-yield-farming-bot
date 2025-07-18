use dotenvy::dotenv;
use tracing::{info};
use std::sync::Arc;

use crypto_yield_farming_bot::logging;
use crypto_yield_farming_bot::config;
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

    // Initialize GMX event fetcher
    let mut event_fetcher = gmx::event_fetcher::GmxEventFetcher::init(
        Arc::clone(&cfg.alchemy_provider),
        cfg.gmx_eventemitter,
    );

    // Fetch fees
    let fees_map = event_fetcher.fetch_fees().await?;
    info!("Fetched cumulative fees: {:#?}", fees_map);

    // Wait for 5 mins
    tokio::time::sleep(tokio::time::Duration::from_secs(300)).await;

    // Fetch fees again
    let fees_map = event_fetcher.fetch_fees().await?;
    info!("Fetched cumulative fees again: {:#?}", fees_map);

    Ok(())
}