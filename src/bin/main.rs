use dotenvy::dotenv;
use eyre::Result;
use tracing::{debug, info, error};

use crypto_yield_farming_bot::logging;
use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::hedging::dydx_client::DydxClient;

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

    // Initialize dydx client
    let mut dydx_client = DydxClient::new().await?;
    info!("dYdX client initialized successfully");

    // Get perpetual markets
    match dydx_client.get_perpetual_markets().await {
        Ok(markets) => {
            info!(market_count = markets.len(), "Fetched perpetual markets from dYdX");
            for market in markets {
                info!(market = ?market, "Perpetual Market");
            }
        }
        Err(e) => {
            error!("Failed to fetch perpetual markets: {}", e);
        }
    }

    tokio::time::sleep(std::time::Duration::from_secs(3)).await; // Allow time for logging to flush
    Ok(())
}
