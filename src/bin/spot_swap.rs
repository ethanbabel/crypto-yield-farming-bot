use dotenvy::dotenv;
use eyre::Result;
use tracing::{info, error};
use ethers::types::{Address};
use std::str::FromStr;
use std::sync::Arc;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;

use crypto_yield_farming_bot::logging;
use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::wallet::WalletManager;
use crypto_yield_farming_bot::db::db_manager::DbManager;
use crypto_yield_farming_bot::spot_swap::swap_manager::SwapManager;
use crypto_yield_farming_bot::spot_swap::types::SwapRequest;

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
    info!("Database manager initialized");

    // Initialize and load wallet manager
    let mut wallet_manager = WalletManager::new(&cfg)?;
    wallet_manager.load_tokens(&db).await?;
    let wallet_manager = Arc::new(wallet_manager);
    info!("Wallet manager initialized and tokens loaded");

    // Log all wallet balances
    wallet_manager.log_all_balances(false).await?;

    // Initialize Spot Swap Manager
    let swap_manager = SwapManager::new(&cfg, wallet_manager.clone());
    info!("Spot Swap Manager initialized");

    // Example usage of Spot Swap Manager Swap
    let swap_request = SwapRequest {
        from_token_address: Address::from_str("0x82aF49447D8a07e3bd95BD0d56f35241523fBab1").unwrap(), // WETH
        to_token_address: Address::from_str("0xaf88d065e77c8cC2239327C5EDb3A432268e5831").unwrap(), // USDC
        amount: Decimal::from_f64(1.5).unwrap(), // 1.5 USDC
        side: "BUY".to_string(), // Buying USDC with WETH
    };
    info!("Executing swap request: {:?}", swap_request);
    if let Err(e) = swap_manager.execute_swap(&swap_request).await {
        error!(error = ?e, "Failed to execute swap request");
        return Err(e.into());
    }

    // Log all wallet balances after swap
    wallet_manager.log_all_balances(false).await?;

    tokio::time::sleep(std::time::Duration::from_secs(3)).await; // Allow time for logging to flush
    Ok(())
}
