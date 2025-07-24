use dotenvy::dotenv;
use tracing::{info};
use ethers::types::Address;
use std::str::FromStr;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use futures::future::join_all;

use crypto_yield_farming_bot::logging;
use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::wallet::WalletManager;
use crypto_yield_farming_bot::spot_swap::{paraswap_api_client, paraswap_types};

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

    // Initialize and load wallet manager
    let wallet_manager = WalletManager::new(&cfg)?;
    info!(address = %wallet_manager.address, "Wallet manager initialized");

    // Initialize ParaSwap client
    let paraswap_client = paraswap_api_client::ParaSwapClient::new(wallet_manager.address.clone(), &cfg);
    info!("ParaSwap client initialized");

    // Example SwapRequest
    let swap_request = paraswap_types::SwapRequest {
        from_token: Address::from_str("0x82aF49447D8a07e3bd95BD0d56f35241523fBab1").unwrap(), // WETH
        from_token_decimals: 18,
        to_token: Address::from_str("0xaf88d065e77c8cC2239327C5EDb3A432268e5831").unwrap(), // USDC
        to_token_decimals: 6,
        amount: Decimal::from_f64(1.0).unwrap(), // 1 USDC
        side: "BUY".to_string(), // Buying USDC with WETH
        slippage_tolerance: Decimal::from_f64(0.5).unwrap(), // 0.5% slippage
    };

    // Fetch swap quote
    let quote = paraswap_client.get_quote(&swap_request).await?;
    info!("Swap quote fetched successfully: {:#?}", quote);

    // Test rate limiting
    let start_time = std::time::Instant::now();
    let mut tasks = Vec::new();
    for i in 0..500 {
        let client = paraswap_client.clone();
        let request = swap_request.clone();
        let task = tokio::spawn(async move {
            let result = client.get_quote(&request).await;
            let elapsed = start_time.elapsed();
            info!("Quote request {} completed after {:?}: {:?}", i + 1, elapsed, result.is_ok());
            result
        });
        tasks.push(task);
    }

    // Wait for all tasks to complete
    let results = join_all(tasks).await;
    
    let successful_requests = results.iter()
        .filter(|task_result| task_result.is_ok() && task_result.as_ref().unwrap().is_ok())
        .count();
    
    info!("Parallel rate limiting test completed: {}/{} requests successful in {:?}", 
          successful_requests, 500, start_time.elapsed());
    
    tokio::time::sleep(std::time::Duration::from_secs(3)).await; // Allow time for logging to flush

    Ok(())
}