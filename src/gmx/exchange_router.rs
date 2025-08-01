use tracing::{debug, instrument};
use eyre::Result;
use ethers::prelude::*;

use crate::config::Config;
use crate::wallet::WalletManager;
use super::exchange_router_utils;

abigen!(
    ExchangeRouter,
    "./abis/ExchangeRouter.json"
);

abigen!(
    ERC20,
    r#"[
        function approve(address spender, uint256 amount) external returns (bool)
        function allowance(address owner, address spender) external view returns (uint256)
    ]"#
);

/// Create a deposit in the GMX Exchange Router
#[instrument(skip(config, wallet_manager, params, initial_long_amount, initial_short_amount, gas_limit, gas_price))]
pub async fn create_deposit(
    config: &Config, 
    wallet_manager: &WalletManager,
    params: exchange_router_utils::CreateDepositParams,
    initial_long_amount: U256,
    initial_short_amount: U256,
    gas_limit: U256,
    gas_price: U256
) -> Result<(TxHash, TransactionReceipt)> {
    let exchange_router = ExchangeRouter::new(config.gmx_exchangerouter, wallet_manager.signer.clone());
    let execution_fee = params.execution_fee;

    // Approve token spending if needed
    approve_token(wallet_manager, params.initial_long_token, config.gmx_baserouter, initial_long_amount).await?;
    approve_token(wallet_manager, params.initial_short_token, config.gmx_baserouter, initial_short_amount).await?;

    // Create token transfer calls
    let mut encoded_calls = Vec::new();

    if initial_long_amount > U256::zero() {
        let call = exchange_router.send_tokens(
            params.initial_long_token, config.gmx_depositvault, initial_long_amount
        ).gas(gas_limit).gas_price(gas_price);
        let calldata = call.calldata().ok_or_else(|| eyre::eyre!("Failed to encode calldata"))?;
        encoded_calls.push(calldata);
    }

    if initial_short_amount > U256::zero() {
        let call = exchange_router.send_tokens(
            params.initial_short_token, config.gmx_depositvault, initial_short_amount
        ).gas(gas_limit).gas_price(gas_price);
        let calldata = call.calldata().ok_or_else(|| eyre::eyre!("Failed to encode calldata"))?;
        encoded_calls.push(calldata);
    }

    if execution_fee > U256::zero() {
        let call = exchange_router.send_wnt(
            config.gmx_depositvault, execution_fee
        ).gas(gas_limit).gas_price(gas_price);
        let calldata = call.calldata().ok_or_else(|| eyre::eyre!("Failed to encode calldata"))?;
        encoded_calls.push(calldata);
    }

    // Create the deposit call
    let call = exchange_router.create_deposit(params.into()).gas(gas_limit).gas_price(gas_price);
    let calldata = call.calldata().ok_or_else(|| eyre::eyre!("Failed to encode calldata"))?;
    encoded_calls.push(calldata);

    // Create the multicall
    let multicall = exchange_router.multicall(encoded_calls)
        .from(wallet_manager.address)
        .gas(gas_limit)
        .gas_price(gas_price)
        .value(execution_fee);
    debug!(multicall = ?multicall, "Creating deposit transaction");
    
    // Send the transaction
    let pending_tx = multicall.send().await?;
    let tx_hash = pending_tx.tx_hash();
    debug!(tx_hash = ?tx_hash, "Deposit transaction sent, waiting for confirmation");

    let receipt = match pending_tx.await? {
        Some(receipt) => {
            if receipt.status == Some(1.into()) {
                receipt
            } else {
                return Err(eyre::eyre!("Deposit creation failed with status {:?}: {:?}", receipt.status, receipt));
            }
        },
        None => {
            return Err(eyre::eyre!("Deposit creation transaction failed: no receipt returned"));
        }
    };

    Ok((tx_hash, receipt))
}

/// Create a withdrawal in the GMX Exchange Router
#[instrument(skip(config, wallet_manager, params, market_token_amount, gas_limit, gas_price))]
pub async fn create_withdrawal(
    config: &Config, 
    wallet_manager: &WalletManager,
    params: exchange_router_utils::CreateWithdrawalParams,
    market_token_amount: U256,
    gas_limit: U256,
    gas_price: U256
) -> Result<(TxHash, TransactionReceipt)> {
    let exchange_router = ExchangeRouter::new(config.gmx_exchangerouter, wallet_manager.signer.clone());
    let execution_fee = params.execution_fee;
    
    // Approve token spending if needed
    approve_token(wallet_manager, params.market, config.gmx_baserouter, market_token_amount).await?;

    // Create token transfer calls
    let mut encoded_calls = Vec::new();

    if market_token_amount > U256::zero() {
        let call = exchange_router.send_tokens(
            params.market, config.gmx_withdrawalvault, market_token_amount
        ).gas(gas_limit).gas_price(gas_price);
        let calldata = call.calldata().ok_or_else(|| eyre::eyre!("Failed to encode calldata"))?;
        encoded_calls.push(calldata);
    }

    if execution_fee > U256::zero() {
        let call = exchange_router.send_wnt(
            config.gmx_withdrawalvault, execution_fee
        ).gas(gas_limit).gas_price(gas_price);
        let calldata = call.calldata().ok_or_else(|| eyre::eyre!("Failed to encode calldata"))?;
        encoded_calls.push(calldata);
    }

    // Create the withdrawal call
    let call = exchange_router.create_withdrawal(params.into()).gas(gas_limit).gas_price(gas_price);
    let calldata = call.calldata().ok_or_else(|| eyre::eyre!("Failed to encode calldata"))?;
    encoded_calls.push(calldata);

    // Create the multicall
    let multicall = exchange_router.multicall(encoded_calls)
        .from(wallet_manager.address)
        .gas(gas_limit)
        .gas_price(gas_price)
        .value(execution_fee);
    debug!(multicall = ?multicall, "Creating withdrawal transaction");

    // Send the transaction
    let pending_tx = multicall.send().await?;
    let tx_hash = pending_tx.tx_hash();
    debug!(tx_hash = ?tx_hash, "Withdrawal transaction sent, waiting for confirmation");
    
    let receipt = match pending_tx.await? {
        Some(receipt) => {
            if receipt.status == Some(1.into()) {
                receipt
            } else {
                return Err(eyre::eyre!("Withdrawal creation failed with status {:?}: {:?}", receipt.status, receipt));
            }
        },
        None => {
            return Err(eyre::eyre!("Withdrawal creation transaction failed: no receipt returned"));
        }
    };

    Ok((tx_hash, receipt))
}

/// Create a shift in the GMX Exchange Router
#[instrument(skip(config, wallet_manager, params, from_token_amount, gas_limit, gas_price))]
pub async fn create_shift(
    config: &Config, 
    wallet_manager: &WalletManager,
    params: exchange_router_utils::CreateShiftParams,
    from_token_amount: U256,
    gas_limit: U256,
    gas_price: U256
) -> Result<(TxHash, TransactionReceipt)> {
    let exchange_router = ExchangeRouter::new(config.gmx_exchangerouter, wallet_manager.signer.clone());
    let execution_fee = params.execution_fee;

    // Approve token spending if needed
    approve_token(wallet_manager, params.from_market, config.gmx_baserouter, from_token_amount).await?;

    // Create token transfer calls
    let mut encoded_calls = Vec::new();
    
    if from_token_amount > U256::zero() {
        let call = exchange_router.send_tokens(
            params.from_market, config.gmx_shiftvault, from_token_amount
        ).gas(gas_limit).gas_price(gas_price);
        let calldata = call.calldata().ok_or_else(|| eyre::eyre!("Failed to encode calldata"))?;
        encoded_calls.push(calldata);
    }

    if execution_fee > U256::zero() {
        let call = exchange_router.send_wnt(
            config.gmx_shiftvault, execution_fee
        ).gas(gas_limit).gas_price(gas_price);
        let calldata = call.calldata().ok_or_else(|| eyre::eyre!("Failed to encode calldata"))?;
        encoded_calls.push(calldata);
    }

    // Create the shift call
    let call = exchange_router.create_shift(params.into()).gas(gas_limit).gas_price(gas_price);
    let calldata = call.calldata().ok_or_else(|| eyre::eyre!("Failed to encode calldata"))?;
    encoded_calls.push(calldata);

    // Create the multicall
    let multicall = exchange_router.multicall(encoded_calls)
        .from(wallet_manager.address)
        .gas(gas_limit)
        .gas_price(gas_price)
        .value(execution_fee);
    debug!(multicall = ?multicall, "Creating shift transaction");

    // Send the transaction
    let pending_tx = multicall.send().await?;
    let tx_hash = pending_tx.tx_hash();
    debug!(tx_hash = ?tx_hash, "Shift transaction sent, waiting for confirmation");

    let receipt = match pending_tx.await? {
        Some(receipt) => {
            if receipt.status == Some(1.into()) {
                receipt
            } else {
                return Err(eyre::eyre!("Shift creation failed with status {:?}: {:?}", receipt.status, receipt));
            }
        },
        None => {
            return Err(eyre::eyre!("Shift creation transaction failed: no receipt returned"));
        }
    };

    Ok((tx_hash, receipt))
}

//----------------------------------------------------------------------------------------------------------------------------------------

/// Helper function to approve token spending
#[instrument(skip(wallet_manager, token_address, spender, amount))]
async fn approve_token(wallet_manager: &WalletManager, token_address: Address, spender: Address, amount: U256) -> Result<()> {
    if amount.is_zero() {
        debug!(?token_address, ?spender, "No approval needed for zero amount");
        return Ok(());
    }
    let amount = amount + (amount / 10); // Add 10% buffer
    let token = ERC20::new(token_address, wallet_manager.signer.clone());
    let allowance = token.allowance(wallet_manager.address, spender).call().await?;
    if allowance < amount {
        let approval_call = token.approve(spender, amount);
        let pending_tx = approval_call.send().await?;
        let receipt = pending_tx.await?;
        match receipt {
            Some(receipt) => {
                if receipt.status == Some(1.into()) {
                    debug!(?token_address, ?spender, ?amount, "Token spending approved successfully");
                } else {
                    return Err(eyre::eyre!("Token approval failed with status {:?}: {:?}", receipt.status, receipt));
                }
            },
            None => {
                return Err(eyre::eyre!("Token approval transaction failed: no receipt returned"));
            }
        }
    } else {
        debug!(?token_address, ?spender, ?amount, "Token spending already approved");
    }
    Ok(())
}

//----------------------------------------------------------------------------------------------------------------------------------------
    
impl From<exchange_router_utils::CreateDepositParams> for CreateDepositParams {
    fn from(params: exchange_router_utils::CreateDepositParams) -> Self {
        CreateDepositParams {
            receiver: params.receiver,
            callback_contract: params.callback_contract,
            ui_fee_receiver: params.ui_fee_receiver,
            market: params.market,
            initial_long_token: params.initial_long_token,
            initial_short_token: params.initial_short_token,
            long_token_swap_path: params.long_token_swap_path,
            short_token_swap_path: params.short_token_swap_path,
            min_market_tokens: params.min_market_tokens,
            should_unwrap_native_token: params.should_unwrap_native_token,
            execution_fee: params.execution_fee,
            callback_gas_limit: params.callback_gas_limit,
        }
    }
}

impl From<exchange_router_utils::CreateWithdrawalParams> for CreateWithdrawalParams {
    fn from(params: exchange_router_utils::CreateWithdrawalParams) -> Self {
        CreateWithdrawalParams {
            receiver: params.receiver,
            callback_contract: params.callback_contract,
            ui_fee_receiver: params.ui_fee_receiver,
            market: params.market,
            long_token_swap_path: params.long_token_swap_path,
            short_token_swap_path: params.short_token_swap_path,
            min_long_token_amount: params.min_long_token_amount,
            min_short_token_amount: params.min_short_token_amount,
            should_unwrap_native_token: params.should_unwrap_native_token,
            execution_fee: params.execution_fee,
            callback_gas_limit: params.callback_gas_limit,
        }
    }
}

impl From<exchange_router_utils::CreateShiftParams> for CreateShiftParams {
    fn from(params: exchange_router_utils::CreateShiftParams) -> Self {
        CreateShiftParams {
            receiver: params.receiver,
            callback_contract: params.callback_contract,
            ui_fee_receiver: params.ui_fee_receiver,
            from_market: params.from_market,
            to_market: params.to_market,
            min_market_tokens: params.min_market_tokens,
            execution_fee: params.execution_fee,
            callback_gas_limit: params.callback_gas_limit,
        }
    }
}
