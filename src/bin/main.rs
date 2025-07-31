use dotenvy::dotenv;
use eyre::Result;
use tracing::{debug, info, error};
use ethers::types::{Address, U256};
use ethers::middleware::Middleware;
use std::str::FromStr;

use crypto_yield_farming_bot::logging;
use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::wallet::WalletManager;
use crypto_yield_farming_bot::db::db_manager::DbManager;
use crypto_yield_farming_bot::gmx::{
    exchange_router_utils,
    exchange_router,
    datastore,
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
    info!("Wallet manager initialized and tokens loaded");

    // Log all wallet balances
    wallet_manager.log_all_balances(false).await?;

    // Calculate execution fee for a deposit
    let num_swaps = U256::from(0);
    let callback_gas_limit = U256::zero(); 
    let (execution_fee, gas_limit, gas_price) = calculate_execution_fee(
        &cfg, &wallet_manager, num_swaps, callback_gas_limit, GmxTransactionType::Deposit
    ).await?;

    // Example usage of create_deposit
    let deposit_params = exchange_router_utils::CreateDepositParams {
        receiver: wallet_manager.address,
        callback_contract: Address::zero(), 
        ui_fee_receiver: Address::zero(), 
        market: Address::from_str("0x70d95587d40A2caf56bd97485aB3Eec10Bee6336").unwrap(), // ETH/USD [ETH - USDC]
        initial_long_token: Address::from_str("0x82aF49447D8a07e3bd95BD0d56f35241523fBab1").unwrap(), // WETH
        initial_short_token: Address::from_str("0xaf88d065e77c8cC2239327C5EDb3A432268e5831").unwrap(), // USDC
        long_token_swap_path: vec![], // No swaps needed
        short_token_swap_path: vec![], // No swaps needed
        min_market_tokens: U256::zero(), 
        should_unwrap_native_token: false,
        execution_fee,
        callback_gas_limit,
    };

    // Create deposit - 0 WETH, 0.5 USDC
    if let Err(e) = exchange_router::create_deposit(&cfg, &wallet_manager, deposit_params, U256::zero(), U256::from(500_000), gas_limit, gas_price).await {
        error!(error = ?e, "Failed to create deposit");
        return Err(e.into());
    } 
    info!("Deposit created successfully");

    // Print wallet balances after deposit
    wallet_manager.log_all_balances(false).await?;
    let eth_usd_gm_token_balance = wallet_manager.get_token_balance_u256(
        Address::from_str("0x70d95587d40A2caf56bd97485aB3Eec10Bee6336").unwrap() // ETH/USD [ETH - USDC]
    ).await?;

    // Re-calculate execution fee for a shift
    let num_swaps = U256::from(0);
    let (execution_fee, gas_limit, gas_price) = calculate_execution_fee(
        &cfg, &wallet_manager, num_swaps, callback_gas_limit, GmxTransactionType::Shift
    ).await?;

    // Example usage of create_shift -> Can only shift between markets with the same long & short collateral tokens
    let shift_params = exchange_router_utils::CreateShiftParams {
        receiver: wallet_manager.address,
        callback_contract: Address::zero(), 
        ui_fee_receiver: Address::zero(),
        from_market: Address::from_str("0x70d95587d40A2caf56bd97485aB3Eec10Bee6336").unwrap(), // ETH/USD [ETH - USDC]
        to_market: Address::from_str("0x77B2eC357b56c7d05a87971dB0188DBb0C7836a5").unwrap(), // AAVE/USD [ETH - USDC]
        min_market_tokens: U256::zero(), 
        execution_fee,
        callback_gas_limit,
    };

    // Create shift - all ETH/USD GM tokens to AAVE/USD GM tokens
    if let Err(e) = exchange_router::create_shift(&cfg, &wallet_manager, shift_params, eth_usd_gm_token_balance, gas_limit, gas_price).await {
        error!(error = ?e, "Failed to create shift");
        return Err(e.into());
    }
    info!("Shift created successfully");

    // Print wallet balances after shift
    wallet_manager.log_all_balances(false).await?;
    let aave_usd_gm_token_balance = wallet_manager.get_token_balance_u256(
        Address::from_str("0x77B2eC357b56c7d05a87971dB0188DBb0C7836a5").unwrap() // AAVE/USD [ETH - USDC]
    ).await?;

    // Re-calculate execution fee for a withdrawal
    let num_swaps = U256::from(0);
    let (execution_fee, gas_limit, gas_price) = calculate_execution_fee(
        &cfg, &wallet_manager, num_swaps, callback_gas_limit, GmxTransactionType::Withdrawal
    ).await?;

    // Example usage of create_withdrawal
    let withdrawal_params = exchange_router_utils::CreateWithdrawalParams {
        receiver: wallet_manager.address,
        callback_contract: Address::zero(),
        ui_fee_receiver: Address::zero(),
        market: Address::from_str("0x77B2eC357b56c7d05a87971dB0188DBb0C7836a5").unwrap(), // AAVE/USD [ETH - USDC]
        long_token_swap_path: vec![], // No swaps needed
        short_token_swap_path: vec![], // No swaps needed
        min_long_token_amount: U256::zero(),
        min_short_token_amount: U256::zero(),
        should_unwrap_native_token: false,
        execution_fee,
        callback_gas_limit,
    };

    // Create withdrawal - all GM tokens back to WETH and USDC
    if let Err(e) = exchange_router::create_withdrawal(&cfg, &wallet_manager, withdrawal_params, aave_usd_gm_token_balance, gas_limit, gas_price).await {
        error!(error = ?e, "Failed to create withdrawal");
        return Err(e.into());
    }
    info!("Withdrawal created successfully");

    // Print final wallet balances
    wallet_manager.log_all_balances(false).await?;

    tokio::time::sleep(std::time::Duration::from_secs(3)).await; // Allow time for logging to flush
    Ok(())
}

async fn calculate_execution_fee(
    config: &config::Config, 
    wallet_manager: &WalletManager,
    num_swaps: U256,
    callback_gas_limit: U256, 
    gmx_transaction_type: GmxTransactionType,
) -> Result<(U256, U256, U256)> {
    if !num_swaps.is_zero() && gmx_transaction_type == GmxTransactionType::Shift {
        return Err(eyre::eyre!("Shifts do not support swaps, num_swaps must be 0"));
    }
    let gas_per_swap = datastore::estimate_execute_gas_limit_per_swap(config).await?;
    let gas_for_swaps = gas_per_swap * num_swaps;
    let gas_limit = match gmx_transaction_type {
        GmxTransactionType::Deposit => datastore::get_deposit_gas_limit(config).await?,
        GmxTransactionType::Withdrawal => datastore::get_withdrawal_gas_limit(config).await?,
        GmxTransactionType::Shift => datastore::get_shift_gas_limit(config).await?,
    };
    let estimated_gas_limit = gas_limit + callback_gas_limit + gas_for_swaps;
    debug!(?estimated_gas_limit, "Estimated total gas limit for deposit");

    let oracle_price_count = match gmx_transaction_type {
        GmxTransactionType::Deposit => datastore::estimate_deposit_oracle_price_count(num_swaps),
        GmxTransactionType::Withdrawal => datastore::estimate_withdrawal_oracle_price_count(num_swaps),
        GmxTransactionType::Shift => datastore::estimate_shift_oracle_price_count(num_swaps),
    };
    let adjusted_gas_limit = datastore::adjust_gas_limit_for_estimate(
        config,
        estimated_gas_limit,
        oracle_price_count,
    ).await?;
    debug!(?adjusted_gas_limit, "Adjusted gas limit for estimate");

    let gas_price = wallet_manager.signer.provider().get_gas_price().await?;
    let gas_price = gas_price + (gas_price / 10); // Add 10% buffer
    debug!(?gas_price, "Gas price with buffer");

    let execution_fee = adjusted_gas_limit * gas_price;
    debug!(?execution_fee, "Calculated execution fee for deposit");

    Ok((execution_fee, adjusted_gas_limit, gas_price))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GmxTransactionType {
    Deposit,
    Withdrawal,
    Shift,
}
