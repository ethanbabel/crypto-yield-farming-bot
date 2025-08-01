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
use crypto_yield_farming_bot::gm_token_txs::{
    gm_tx_manager::GmTxManager,
    types::{
        GmDepositRequest,
        GmWithdrawalRequest,
        GmShiftRequest,
        GmTxRequest
    }
};

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

    // Initialize GM Transaction Manager
    let gm_tx_manager = GmTxManager::new(cfg.clone(), wallet_manager.clone());
    info!("GM Transaction Manager initialized");

    // Example usage of GM Transaction Manager Deposit
    let deposit_request = GmDepositRequest {
        market: Address::from_str("0x70d95587d40A2caf56bd97485aB3Eec10Bee6336").unwrap(), // ETH/USD [ETH - USDC]
        long_amount: Decimal::zero(), // 0 WETH
        short_amount: Decimal::from_f64(0.5).unwrap(), // 0.5 USDC
    };
    info!("Executing deposit request: {:?}", deposit_request);
    if let Err(e) = gm_tx_manager.execute_transaction(&GmTxRequest::Deposit(deposit_request)).await {
        error!(error = ?e, "Failed to execute deposit request");
        return Err(e.into());
    }

    // Log all wallet balances after deposit
    wallet_manager.log_all_balances(false).await?;
    let eth_usd_gm_token_balance = wallet_manager.get_token_balance(
        Address::from_str("0x70d95587d40A2caf56bd97485aB3Eec10Bee6336").unwrap() // ETH/USD [ETH - USDC]
    ).await?;

    // Example usage of GM Transaction Manager Shift -> Can only shift between markets with the same long & short collateral tokens
    let shift_request = GmShiftRequest {
        from_market: Address::from_str("0x70d95587d40A2caf56bd97485aB3Eec10Bee6336").unwrap(), // ETH/USD [ETH - USDC]
        to_market: Address::from_str("0x77B2eC357b56c7d05a87971dB0188DBb0C7836a5").unwrap(), // AAVE/USD [ETH - USDC]
        amount: eth_usd_gm_token_balance, // Use all the balance gained from the deposit
    };
    info!("Executing shift request: {:?}", shift_request);
    if let Err(e) = gm_tx_manager.execute_transaction(&GmTxRequest::Shift(shift_request)).await {
        error!(error = ?e, "Failed to execute shift request");
        return Err(e.into());
    }

    // Log all wallet balances after shift
    wallet_manager.log_all_balances(false).await?;
    let aave_usd_gm_token_balance = wallet_manager.get_token_balance(
        Address::from_str("0x77B2eC357b56c7d05a87971dB0188DBb0C7836a5").unwrap() // AAVE/USD [ETH - USDC]
    ).await?;

    // Example usage of GM Transaction Manager Withdrawal
    let withdrawal_request = GmWithdrawalRequest {
        market: Address::from_str("0x77B2eC357b56c7d05a87971dB0188DBb0C7836a5").unwrap(), // AAVE/USD [ETH - USDC]
        amount: aave_usd_gm_token_balance, // Use all the balance gained from the shift
    };
    info!("Executing withdrawal request: {:?}", withdrawal_request);
    if let Err(e) = gm_tx_manager.execute_transaction(&GmTxRequest::Withdrawal(withdrawal_request)).await {
        error!(error = ?e, "Failed to execute withdrawal request");
        return Err(e.into());
    }

    // Log final wallet balances
    wallet_manager.log_all_balances(false).await?;

    tokio::time::sleep(std::time::Duration::from_secs(3)).await; // Allow time for logging to flush
    Ok(())
}
