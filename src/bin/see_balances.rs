use dotenvy::dotenv;
use tracing::{instrument, info};
use std::sync::Arc;

use crypto_yield_farming_bot::logging;
use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::wallet::WalletManager;
use crypto_yield_farming_bot::db::db_manager::DbManager;
use crypto_yield_farming_bot::hedging::dydx_client::DydxClient;

#[instrument(name = "trading_bot_main")]
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

    // Initialize db manager
    let db = DbManager::init(&cfg).await?;
    info!("Database manager initialized");

    // Initialize and load wallet manager
    let mut wallet_manager = WalletManager::new(&cfg)?;
    wallet_manager.load_tokens(&db).await?;
    let wallet_manager = Arc::new(wallet_manager);
    info!(address = ?wallet_manager.address, "Wallet manager initialized");

    // Initialize dydx client
    let mut dydx_client = DydxClient::new(cfg.clone(), wallet_manager.clone()).await?;
    info!("dYdX client initialized");

    // Log wallet token balances
    wallet_manager.log_all_balances(false).await?;

    // Log dYdX main account USDC balance
    let main_usdc = dydx_client.get_dydx_usdc_balance().await?;
    info!(main_usdc = %main_usdc, "dYdX main account USDC balance retrieved");

    // Log dYdX subaccount details and balances
    let summary = dydx_client.get_subaccount_summary().await?;
    info!(subaccount_summary = ?summary, "Retrieved dYdX subaccount summary");
    
    tokio::time::sleep(std::time::Duration::from_secs(1)).await; // Allow time for logging to flush

    Ok(())
}
