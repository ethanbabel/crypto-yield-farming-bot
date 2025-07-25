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

    // Example SwapRequest 1 (Side = "BUY", ERC20 to ERC20)
    let swap_request1 = SwapRequest {
        from_token_address: Address::from_str("0xaf88d065e77c8cC2239327C5EDb3A432268e5831").unwrap(), // USDC
        to_token_address: Address::from_str("0x5d3a1Ff2b6BAb83b63cd9AD0787074081a52ef34").unwrap(), // USDe
        amount: Decimal::from_f64(0.5).unwrap(), // 0.5 USDe
        side: "BUY".to_string(), // Buying USDe with USDC
    };

    // Execute swap 1
    if let Err(e) = swap_manager.execute_swap(&swap_request1).await {
        error!(error = %e, "Failed to execute swap");
        return Err(e.into());
    }

    // Example SwapRequest 2 (Side = "SELL", ERC20 to Native Token)
    let swap_request2 = SwapRequest {
        from_token_address: Address::from_str("0x5d3a1Ff2b6BAb83b63cd9AD0787074081a52ef34").unwrap(), // USDe
        to_token_address: Address::from_str("0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE").unwrap(), // Native token
        amount: Decimal::from_f64(0.5).unwrap(), // 0.5 USDe
        side: "SELL".to_string(), // Selling USDe for native token
    };

    // Execute swap 2
    if let Err(e) = swap_manager.execute_swap(&swap_request2).await {
        error!(error = %e, "Failed to execute swap");
        return Err(e.into());
    }

    // Example SwapRequest 3 (Side = "BUY", Native Token to ERC20)
    let swap_request3 = SwapRequest {
        from_token_address: Address::from_str("0xEeeeeEeeeEeEeeEeEeEeeEEEeeeeEeeeeeeeEEeE").unwrap(), // Native token
        to_token_address: Address::from_str("0xaf88d065e77c8cC2239327C5EDb3A432268e5831").unwrap(), // USDC
        amount: Decimal::from_f64(0.5).unwrap(), // 0.5 USDC
        side: "BUY".to_string(), // Buying USDC with native token
    };

    // Execute swap 3
    if let Err(e) = swap_manager.execute_swap(&swap_request3).await {
        error!(error = %e, "Failed to execute swap");
        return Err(e.into());
    }

    tokio::time::sleep(std::time::Duration::from_secs(3)).await; // Allow time for logging to flush

    Ok(())
}