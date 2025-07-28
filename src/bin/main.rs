use dotenvy::dotenv;
use tracing::{info, error};
use ethers::types::Address;
use std::str::FromStr;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;

use crypto_yield_farming_bot::logging;
use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::wallet::WalletManager;
use crypto_yield_farming_bot::db::db_manager::DbManager;
use crypto_yield_farming_bot::spot_swap::{
    paraswap_types::SwapRequest,
    swap_manager::SwapManager,
};

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
    info!("Wallet manager initialized and tokens loaded");

    // Initialize ParaSwap client
    let swap_manager = SwapManager::new(wallet_manager, &cfg);
    info!("Swap manager initialized");

    // Swap (Wrap) ETH to WETH
    let swap_request = SwapRequest {
        from_token_address: Address::from_str("0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE").unwrap(), // Native token
        to_token_address: Address::from_str("0x82aF49447D8a07e3bd95BD0d56f35241523fBab1").unwrap(), // WETH
        amount: Decimal::from_f64(0.0001).unwrap(), // 0.0001 ETH/WETH
        side: "BUY".to_string(), // Irrelevant for ETH -> WETH Wrap (exchange is always 1:1)
    };

    // Execute the wrap
    if let Err(e) = swap_manager.execute_swap(&swap_request).await {
        error!(error = ?e, "Failed to execute ETH/WETH swap");
        return Err(e.into());
    }

    // Swap (Unwrap) WETH to ETH
    let swap_request = SwapRequest {
        from_token_address: Address::from_str("0x82aF49447D8a07e3bd95BD0d56f35241523fBab1").unwrap(), // WETH
        to_token_address: Address::from_str("0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE").unwrap(), // Native token
        amount: Decimal::from_f64(0.0001).unwrap(), // 0.0001 ETH/WETH
        side: "SELL".to_string(), // Irrelevant for WETH -> ETH unwrap (exchange is always 1:1)
    };

    // Execute the unwrap
    if let Err(e) = swap_manager.execute_swap(&swap_request).await {
        error!(error = ?e, "Failed to execute WETH/ETH swap");
        return Err(e.into());
    }

    tokio::time::sleep(std::time::Duration::from_secs(3)).await; // Allow time for logging to flush

    Ok(())
}