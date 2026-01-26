use dotenvy::dotenv;
use eyre::Result;
use tracing::{info};
use std::sync::Arc;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;

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
    let mut dydx_client = DydxClient::new(cfg.clone(), wallet_manager.clone()).await?;
    info!("dYdX client initialized");

    // Set subaccount balance
    dydx_client.set_subaccount_usdc_balance(Decimal::from_str("1")?).await?; // Will deposit/withdraw as needed to reach this balance
    info!("dYdX subaccount USDC balance set");

    // Place order to short ETH-USD perp
    let token = "ETH".to_string();
    let max_leverage = dydx_client.get_max_leverage(&token).await?;
    let perp = dydx_client.get_perpetual_market(&token).await?.ok_or(eyre::eyre!("No perpetual market data available for {}", token))?;
    let cur_oracle_price = perp.oracle_price.clone().ok_or(eyre::eyre!("No oracle price available for {}", token))?;
    let cur_oracle_price_decimal = Decimal::from_str(&cur_oracle_price.to_string())?;
    let unleveraged_size = Decimal::from_str("1")? / cur_oracle_price_decimal; // $1 worth of ETH
    let size = unleveraged_size * (max_leverage / Decimal::from_str("2")?); // Use half the max leverage
    let side_is_buy = false; // Short
    info!(
        token = %token,
        max_leverage = %max_leverage,
        cur_oracle_price = %cur_oracle_price,
        unleveraged_size = %unleveraged_size,
        size = %size,
        "Placing dYdX market order to short ETH-USD perp"
    );
    if let Err(e) = dydx_client.submit_perp_order(&token, size, side_is_buy).await {
        info!(error = %e, "Failed to submit dYdX market order");
    } else {
        info!("dYdX market order submitted successfully");
    }
    
    dydx_client.wait_for_active_tasks().await; // Wait before closing position

    // Close the position
    info!(token = %token, "Closing dYdX perp position");
    if let Err(e) = dydx_client.reduce_perp_position(&token, None).await {
        info!(error = %e, "Failed to close dYdX perp position");
    } else {
        info!("dYdX perp position closed successfully");
    }

    dydx_client.wait_for_active_tasks().await; // Wait before exiting

    tokio::time::sleep(std::time::Duration::from_secs(3)).await; // Allow time for logging to flush
    Ok(())
}
