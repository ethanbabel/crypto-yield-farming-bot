use dotenvy::dotenv;
use eyre::Result;
use tracing::{info};
use std::sync::Arc;

use crypto_yield_farming_bot::logging;
use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::db::db_manager::DbManager;
use crypto_yield_farming_bot::wallet::WalletManager;
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

    // Initialize db manager
    let db = DbManager::init(&cfg).await?;
    let db = Arc::new(db);
    info!("Database manager initialized");

    // Initialize and load wallet manager
    let mut wallet_manager = WalletManager::new(&cfg)?;
    wallet_manager.load_tokens(&db).await?;
    let wallet_manager = Arc::new(wallet_manager);
    info!("Wallet manager initialized and tokens loaded");

    // Initialize dydx client
    let dydx_client = DydxClient::new(wallet_manager.clone()).await?;
    info!("dYdX client initialized successfully");

    // Get perpetual markets
    let token_perp_map = dydx_client.get_token_perp_map().await?;
    for (token, perp_opt) in token_perp_map.iter() {
        let perp_ticker = if let Some(perp) = perp_opt {
            perp.ticker.to_string()
        } else {
            "N/A".to_string()
        };
        info!("Token: {}, Perpetual Market: {}", token, perp_ticker);
    }

    tokio::time::sleep(std::time::Duration::from_secs(3)).await; // Allow time for logging to flush
    Ok(())
}
