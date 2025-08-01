use eyre::Result;
use tracing::{debug, info, instrument};
use std::sync::Arc;
use ethers::prelude::*;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;

use crate::config::Config;
use crate::wallet::WalletManager;
use crate::gmx::{
    exchange_router_utils,
    exchange_router,
    datastore,
};
use super::types::{GmTxRequest, GmDepositRequest, GmWithdrawalRequest, GmShiftRequest};

const MAX_FEE_PER_GAS_BUFFER: f64 = 1.1; // 10% above the current gas price

pub struct GmTxManager {
    config: Arc<Config>,
    wallet_manager: Arc<WalletManager>,
    max_fee_per_gas_buffer: Decimal,
}

impl GmTxManager {
    /// Creates a new instance of TxManager
    pub fn new(config: Arc<Config>, wallet_manager: Arc<WalletManager>) -> Self {
        Self {
            config,
            wallet_manager,
            max_fee_per_gas_buffer: Decimal::from_f64(MAX_FEE_PER_GAS_BUFFER).unwrap(),
        }
    }

    /// Execute a GM transaction request
    #[instrument(skip(self))]
    pub async fn execute_transaction(&self, request: &GmTxRequest) -> Result<()> {
        match request {
            GmTxRequest::Deposit(deposit_request) => self.execute_deposit(deposit_request).await,
            GmTxRequest::Withdrawal(withdrawal_request) => self.execute_withdrawal(withdrawal_request).await,
            GmTxRequest::Shift(shift_request) => self.execute_shift(shift_request).await,
        }
    }

    // ==================== Helper/Private methods ====================

    /// Execute a GM deposit request
    #[instrument(skip(self, request))]
    async fn execute_deposit(&self, request: &GmDepositRequest) -> Result<()> {
        // Validate request
        let log_string = self.validate_deposit_request(&request).await?;

        // Get pre-deposit balances
        let market_token_info = self.wallet_manager.market_tokens.get(&request.market)
            .ok_or_else(|| eyre::eyre!("Market token not found: {}", request.market))?;
        let long_token_info = self.wallet_manager.asset_tokens.get(&market_token_info.long_token_address)
            .ok_or_else(|| eyre::eyre!("Long token not found: {}", market_token_info.long_token_address))?;
        let short_token_info = self.wallet_manager.asset_tokens.get(&market_token_info.short_token_address)
            .ok_or_else(|| eyre::eyre!("Short token not found: {}", market_token_info.short_token_address))?;
        let initial_market_token_balance = self.wallet_manager.get_token_balance(market_token_info.address).await?;
        let initial_long_token_balance = self.wallet_manager.get_token_balance(market_token_info.long_token_address).await?;
        let initial_short_token_balance = self.wallet_manager.get_token_balance(market_token_info.short_token_address).await?;
        let initial_native_token_balance = self.wallet_manager.get_native_balance().await?;

        info!(
            initial_market_token_balance = ?initial_market_token_balance,
            initial_long_token_balance = ?initial_long_token_balance,
            initial_short_token_balance = ?initial_short_token_balance,
            initial_native_token_balance = ?initial_native_token_balance,
            "{} Deposit Initiated",
            log_string
        );

        // Get execution fee
        let (execution_fee, gas_limit, gas_price) = self.calculate_execution_fee(GmTxRequest::Deposit(request.clone())).await?;

        // Verify funds for deposit
        if initial_long_token_balance < request.long_amount {
            return Err(eyre::eyre!(
                "Insufficient long token balance for deposit: need {} but have {}", 
                request.long_amount, initial_long_token_balance
            ));
        }
        if initial_short_token_balance < request.short_amount {
            return Err(eyre::eyre!("Insufficient short token balance for deposit: need {} but have {}", 
                request.short_amount, initial_short_token_balance
            ));
        }
        if initial_native_token_balance < self.u256_to_decimal(execution_fee, 18)? {
            return Err(eyre::eyre!("Insufficient native token balance for deposit: need {} but have {}", 
                execution_fee, initial_native_token_balance
            ));
        }

        // Create deposit params
        let (deposit_params, initial_long_amount, initial_short_amount) = self.create_deposit_params(request, execution_fee)?;

        // Execute deposit
        let (tx_hash, receipt) = exchange_router::create_deposit(
            &self.config, 
            &self.wallet_manager, 
            deposit_params, 
            initial_long_amount, 
            initial_short_amount, 
            gas_limit, 
            gas_price
        ).await?;
        let gas_used = self.u256_to_decimal(receipt.gas_used.unwrap_or(U256::zero()), 0)?;
        let gas_price = self.u256_to_decimal(receipt.effective_gas_price.unwrap_or(U256::zero()), 18)?;
        info!(
            tx_hash = ?tx_hash,
            block_number = ?receipt.block_number.unwrap_or(U64::zero()),
            gas_used = ?gas_used,
            gas_price = ?gas_price,
            gas_cost = ?gas_used * gas_price,
            gas_cost_usd = ?gas_used * gas_price * self.wallet_manager.native_token.last_mid_price_usd,
            "{} Deposit Executed Successfully",
            log_string,
        );

        // Get post-deposit balances
        let final_market_token_balance = self.wallet_manager.get_token_balance(market_token_info.address).await?;
        let final_long_token_balance = self.wallet_manager.get_token_balance(market_token_info.long_token_address).await?;
        let final_short_token_balance = self.wallet_manager.get_token_balance(market_token_info.short_token_address).await?;
        let final_native_token_balance = self.wallet_manager.get_native_balance().await?;

        let market_token_delta = final_market_token_balance - initial_market_token_balance;
        let long_token_delta = final_long_token_balance - initial_long_token_balance;
        let short_token_delta = final_short_token_balance - initial_short_token_balance;
        let native_token_delta = final_native_token_balance - initial_native_token_balance;

        info!(
            final_market_token_balance = ?final_market_token_balance,
            final_long_token_balance = ?final_long_token_balance,
            final_short_token_balance = ?final_short_token_balance,
            final_native_token_balance = ?final_native_token_balance,
            "{} Deposit Completed \n {}{} {} ({:.2} USD) | {}{} {} ({:.2} USD) | {}{} {} ({:.2} USD) | {}{} NATIVE ({:.4} USD)",
            log_string,
            if market_token_delta.is_sign_positive() { "+" } else { "" }, market_token_delta, 
            market_token_info.symbol, market_token_delta * market_token_info.last_mid_price_usd,
            if long_token_delta.is_sign_positive() { "+" } else { "" }, long_token_delta, 
            long_token_info.symbol, long_token_delta * long_token_info.last_mid_price_usd,
            if short_token_delta.is_sign_positive() { "+" } else { "" }, short_token_delta, 
            short_token_info.symbol, short_token_delta * short_token_info.last_mid_price_usd,
            if native_token_delta.is_sign_positive() { "+" } else { "" }, native_token_delta, 
            native_token_delta * self.wallet_manager.native_token.last_mid_price_usd
        );

        Ok(())
    }

    /// Execute a GM withdrawal request
    #[instrument(skip(self, request))]
    async fn execute_withdrawal(&self, request: &GmWithdrawalRequest) -> Result<()> {
        // Validate request
        let log_string = self.validate_withdrawal_request(&request).await?;

        // Get pre-withdrawal balances
        let market_token_info = self.wallet_manager.market_tokens.get(&request.market)
            .ok_or_else(|| eyre::eyre!("Market token not found: {}", request.market))?;
        let long_token_info = self.wallet_manager.asset_tokens.get(&market_token_info.long_token_address)
            .ok_or_else(|| eyre::eyre!("Long token not found: {}", market_token_info.long_token_address))?;
        let short_token_info = self.wallet_manager.asset_tokens.get(&market_token_info.short_token_address)
            .ok_or_else(|| eyre::eyre!("Short token not found: {}", market_token_info.short_token_address))?;
        let initial_market_token_balance = self.wallet_manager.get_token_balance(market_token_info.address).await?;
        let initial_long_token_balance = self.wallet_manager.get_token_balance(market_token_info.long_token_address).await?;
        let initial_short_token_balance = self.wallet_manager.get_token_balance(market_token_info.short_token_address).await?;
        let initial_native_token_balance = self.wallet_manager.get_native_balance().await?;

        info!(
            initial_market_token_balance = ?initial_market_token_balance,
            initial_long_token_balance = ?initial_long_token_balance,
            initial_short_token_balance = ?initial_short_token_balance,
            initial_native_token_balance = ?initial_native_token_balance,
            "{} Withdrawal Initiated",
            log_string
        );

        // Get execution fee
        let (execution_fee, gas_limit, gas_price) = self.calculate_execution_fee(GmTxRequest::Withdrawal(request.clone())).await?;

        // Verify funds for withdrawal
        if initial_market_token_balance < request.amount {
            return Err(eyre::eyre!(
                "Insufficient market token balance for withdrawal: need {} but have {}", 
                request.amount, initial_market_token_balance
            ));
        }
        if initial_native_token_balance < self.u256_to_decimal(execution_fee, 18)? {
            return Err(eyre::eyre!("Insufficient native token balance for withdrawal: need {} but have {}", 
                execution_fee, initial_native_token_balance
            ));
        }

        // Create withdrawal params
        let (withdrawal_params, market_token_amount) = self.create_withdrawal_params(request, execution_fee)?;

        // Execute withdrawal
        let (tx_hash, receipt) = exchange_router::create_withdrawal(
            &self.config, 
            &self.wallet_manager, 
            withdrawal_params, 
            market_token_amount, 
            gas_limit, 
            gas_price
        ).await?;
        let gas_used = self.u256_to_decimal(receipt.gas_used.unwrap_or(U256::zero()), 0)?;
        let gas_price = self.u256_to_decimal(receipt.effective_gas_price.unwrap_or(U256::zero()), 18)?;
        info!(
            tx_hash = ?tx_hash,
            block_number = ?receipt.block_number.unwrap_or(U64::zero()),
            gas_used = ?gas_used,
            gas_price = ?gas_price,
            gas_cost = ?gas_used * gas_price,
            gas_cost_usd = ?gas_used * gas_price * self.wallet_manager.native_token.last_mid_price_usd,
            "{} Withdrawal Executed Successfully",
            log_string,
        );

        // Get post-withdrawal balances
        let final_market_token_balance = self.wallet_manager.get_token_balance(market_token_info.address).await?;
        let final_long_token_balance = self.wallet_manager.get_token_balance(market_token_info.long_token_address).await?;
        let final_short_token_balance = self.wallet_manager.get_token_balance(market_token_info.short_token_address).await?;
        let final_native_token_balance = self.wallet_manager.get_native_balance().await?;

        let market_token_delta = final_market_token_balance - initial_market_token_balance;
        let long_token_delta = final_long_token_balance - initial_long_token_balance;
        let short_token_delta = final_short_token_balance - initial_short_token_balance;
        let native_token_delta = final_native_token_balance - initial_native_token_balance;
        
        info!(
            final_market_token_balance = ?final_market_token_balance,
            final_long_token_balance = ?final_long_token_balance,
            final_short_token_balance = ?final_short_token_balance,
            final_native_token_balance = ?final_native_token_balance,
            "{} Withdrawal Completed \n {}{} {} ({:.2} USD) | {}{} {} ({:.2} USD) | {}{} {} ({:.2} USD) | {}{} NATIVE ({:.4} USD)",
            log_string,
            if market_token_delta.is_sign_positive() { "+" } else { "" }, market_token_delta,
            market_token_info.symbol, market_token_delta * market_token_info.last_mid_price_usd,
            if long_token_delta.is_sign_positive() { "+" } else { "" }, long_token_delta,
            long_token_info.symbol, long_token_delta * long_token_info.last_mid_price_usd,
            if short_token_delta.is_sign_positive() { "+" } else { "" }, short_token_delta,
            short_token_info.symbol, short_token_delta * short_token_info.last_mid_price_usd,
            if native_token_delta.is_sign_positive() { "+" } else { "" }, native_token_delta,
            native_token_delta * self.wallet_manager.native_token.last_mid_price_usd
        );

        Ok(())
    }

    /// Execute a GM shift request
    #[instrument(skip(self, request))]
    async fn execute_shift(&self, request: &GmShiftRequest) -> Result<()> {
        // Validate request
        let log_string = self.validate_shift_request(&request).await?;

        // Get pre-shift balances
        let from_market_info = self.wallet_manager.market_tokens.get(&request.from_market)
            .ok_or_else(|| eyre::eyre!("From market token not found: {}", request.from_market))?;
        let to_market_info = self.wallet_manager.market_tokens.get(&request.to_market)
            .ok_or_else(|| eyre::eyre!("To market token not found: {}", request.to_market))?;
        let initial_from_market_balance = self.wallet_manager.get_token_balance(from_market_info.address).await?;
        let initial_to_market_balance = self.wallet_manager.get_token_balance(to_market_info.address).await?;
        let initial_native_token_balance = self.wallet_manager.get_native_balance().await?;

        info!(
            initial_from_market_balance = ?initial_from_market_balance,
            initial_to_market_balance = ?initial_to_market_balance,
            initial_native_token_balance = ?initial_native_token_balance,
            "{} Shift Initiated",
            log_string
        );

        // Get execution fee
        let (execution_fee, gas_limit, gas_price) = self.calculate_execution_fee(GmTxRequest::Shift(request.clone())).await?;

        // Verify funds for shift
        if initial_from_market_balance < request.amount {
            return Err(eyre::eyre!(
                "Insufficient from market token balance for shift: need {} but have {}", 
                request.amount, initial_from_market_balance
            ));
        }
        if initial_native_token_balance < self.u256_to_decimal(execution_fee, 18)? {
            return Err(eyre::eyre!("Insufficient native token balance for shift: need {} but have {}", 
                execution_fee, initial_native_token_balance
            ));
        }

        // Create shift params
        let (shift_params, from_market_amount) = self.create_shift_params(request, execution_fee)?;

        // Execute shift
        let (tx_hash, receipt) = exchange_router::create_shift(
            &self.config, 
            &self.wallet_manager, 
            shift_params, 
            from_market_amount, 
            gas_limit, 
            gas_price
        ).await?;
        let gas_used = self.u256_to_decimal(receipt.gas_used.unwrap_or(U256::zero()), 0)?;
        let gas_price = self.u256_to_decimal(receipt.effective_gas_price.unwrap_or(U256::zero()), 18)?;
        info!(
            tx_hash = ?tx_hash,
            block_number = ?receipt.block_number.unwrap_or(U64::zero()),
            gas_used = ?gas_used,
            gas_price = ?gas_price,
            gas_cost = ?gas_used * gas_price,
            gas_cost_usd = ?gas_used * gas_price * self.wallet_manager.native_token.last_mid_price_usd,
            "{} Shift Executed Successfully",
            log_string,
        );

        // Get post-shift balances
        let final_from_market_balance = self.wallet_manager.get_token_balance(from_market_info.address).await?;
        let final_to_market_balance = self.wallet_manager.get_token_balance(to_market_info.address).await?;
        let final_native_token_balance = self.wallet_manager.get_native_balance().await?;

        let from_market_delta = final_from_market_balance - initial_from_market_balance;
        let to_market_delta = final_to_market_balance - initial_to_market_balance;
        let native_token_delta = final_native_token_balance - initial_native_token_balance;

        info!(
            final_from_market_balance = ?final_from_market_balance,
            final_to_market_balance = ?final_to_market_balance,
            final_native_token_balance = ?final_native_token_balance,
            "{} Shift Completed \n {}{} {} ({:.2} USD) | {}{} {} ({:.2} USD) | {}{} NATIVE ({:.4} USD)",
            log_string,
            if from_market_delta.is_sign_positive() { "+" } else { "" }, from_market_delta,
            from_market_info.symbol, from_market_delta * from_market_info.last_mid_price_usd,
            if to_market_delta.is_sign_positive() { "+" } else { "" }, to_market_delta,
            to_market_info.symbol, to_market_delta * to_market_info.last_mid_price_usd,
            if native_token_delta.is_sign_positive() { "+" } else { "" }, native_token_delta,
            native_token_delta * self.wallet_manager.native_token.last_mid_price_usd
        );

        Ok(())
    }
    
    /// Validate the GM deposit request, create log string
    #[instrument(skip(self, request))]
    async fn validate_deposit_request(&self, request: &GmDepositRequest) -> Result<String> {
        // Validate request is valid
        if request.long_amount.is_zero() && request.short_amount.is_zero() {
            return Err(eyre::eyre!("Both long and short amounts cannot be zero"));
        }
        let market_token_info = self.wallet_manager.market_tokens.get(&request.market)
            .ok_or_else(|| eyre::eyre!("Market token not found: {}", request.market))?;
        let long_token_info = self.wallet_manager.asset_tokens.get(&market_token_info.long_token_address)
            .ok_or_else(|| eyre::eyre!("Long token not found: {}", market_token_info.long_token_address))?;
        let short_token_info = self.wallet_manager.asset_tokens.get(&market_token_info.short_token_address)
            .ok_or_else(|| eyre::eyre!("Short token not found: {}", market_token_info.short_token_address))?;

        // Create log string
        let log_string = format!(
            "GM DEPOSIT REQUEST | Deposit {} {} and {} {} into {} |",
            request.long_amount, long_token_info.symbol,
            request.short_amount, short_token_info.symbol,
            market_token_info.symbol
        );
        Ok(log_string)
    }

    /// Validate the GM withdrawal request, create log string
    #[instrument(skip(self, request))]
    async fn validate_withdrawal_request(&self, request: &GmWithdrawalRequest) -> Result<String> {
        // Validate request is valid
        if request.amount.is_zero() {
            return Err(eyre::eyre!("Withdrawal amount cannot be zero"));
        }
        let market_token_info = self.wallet_manager.market_tokens.get(&request.market)
            .ok_or_else(|| eyre::eyre!("Market token not found: {}", request.market))?;
        
        // Create log string
        let log_string = format!(
            "GM WITHDRAWAL REQUEST | Withdraw {} {} |",
            request.amount, market_token_info.symbol
        );
        Ok(log_string)
    }

    /// Validate the GM shift request, create log string
    #[instrument(skip(self, request))]
    async fn validate_shift_request(&self, request: &GmShiftRequest) -> Result<String> {
        // Validate request is valid
        if request.amount.is_zero() {
            return Err(eyre::eyre!("Shift amount cannot be zero"));
        }
        if request.from_market == request.to_market {
            return Err(eyre::eyre!("From and to markets cannot be the same"));
        }
        let from_market_info = self.wallet_manager.market_tokens.get(&request.from_market)
            .ok_or_else(|| eyre::eyre!("From market token not found: {}", request.from_market))?;
        let to_market_info = self.wallet_manager.market_tokens.get(&request.to_market)
            .ok_or_else(|| eyre::eyre!("To market token not found: {}", request.to_market))?;
        
        // Create log string
        let log_string = format!(
            "GM SHIFT REQUEST | Shift {} {} to {} |",
            request.amount, from_market_info.symbol, to_market_info.symbol
        );
        Ok(log_string)
    }             

    /// Calculates the execution fee for a GM transaction
    #[instrument(skip(self))]
    async fn calculate_execution_fee(&self, gm_transaction_type: GmTxRequest) -> Result<(U256, U256, U256)> {
        debug!(?gm_transaction_type, "Calculating execution fee");
        let estimated_gas_limit = match gm_transaction_type {
            GmTxRequest::Deposit(_) => datastore::get_deposit_gas_limit(&self.config).await?,
            GmTxRequest::Withdrawal(_) => datastore::get_withdrawal_gas_limit(&self.config).await?,
            GmTxRequest::Shift(_) => datastore::get_shift_gas_limit(&self.config).await?,
        };
        debug!(?estimated_gas_limit, "Estimated total gas limit for deposit");

        let oracle_price_count = match gm_transaction_type {
            GmTxRequest::Deposit(_) => datastore::estimate_deposit_oracle_price_count(U256::zero()),
            GmTxRequest::Withdrawal(_) => datastore::estimate_withdrawal_oracle_price_count(U256::zero()),
            GmTxRequest::Shift(_) => datastore::estimate_shift_oracle_price_count(U256::zero()),
        };
        let adjusted_gas_limit = datastore::adjust_gas_limit_for_estimate(
            &self.config,
            estimated_gas_limit,
            oracle_price_count,
        ).await?;
        debug!(?adjusted_gas_limit, "Adjusted gas limit for estimate");

        let gas_price_dec = self.u256_to_decimal(self.wallet_manager.signer.provider().get_gas_price().await?, 0)?;
        let gas_price_dec_with_buf = gas_price_dec * self.max_fee_per_gas_buffer;
        let gas_price = self.decimal_to_u256(gas_price_dec_with_buf, 0)?;
        debug!(?gas_price, "Gas price with buffer");

        let execution_fee = adjusted_gas_limit * gas_price;
        debug!(?execution_fee, "Calculated execution fee for deposit");

        Ok((execution_fee, adjusted_gas_limit, gas_price))
    }

    /// Creates GM deposit params from the given request
    fn create_deposit_params(&self, request: &GmDepositRequest, execution_fee: U256) -> Result<(exchange_router_utils::CreateDepositParams, U256, U256)> {
        let market_token_info = self.wallet_manager.market_tokens.get(&request.market)
            .ok_or_else(|| eyre::eyre!("Market token not found: {}", request.market))?;
        let long_token_decimals = self.wallet_manager.asset_tokens.get(&market_token_info.long_token_address)
            .ok_or_else(|| eyre::eyre!("Long token not found: {}", market_token_info.long_token_address))?
            .decimals;
        let short_token_decimals = self.wallet_manager.asset_tokens.get(&market_token_info.short_token_address)
            .ok_or_else(|| eyre::eyre!("Short token not found: {}", market_token_info.short_token_address))?
            .decimals;
        let initial_long_amount = self.decimal_to_u256(request.long_amount, long_token_decimals)?;
        let initial_short_amount = self.decimal_to_u256(request.short_amount, short_token_decimals)?;

        let deposit_params = exchange_router_utils::CreateDepositParams {
            receiver: self.wallet_manager.address,
            callback_contract: Address::zero(),
            ui_fee_receiver: Address::zero(),
            market: request.market,
            initial_long_token: market_token_info.long_token_address,
            initial_short_token: market_token_info.short_token_address,
            long_token_swap_path: vec![], 
            short_token_swap_path: vec![],
            min_market_tokens: U256::zero(),
            should_unwrap_native_token: false,
            execution_fee,
            callback_gas_limit: U256::zero(),
        };
        Ok((deposit_params, initial_long_amount, initial_short_amount))
    }

    /// Creates GM withdrawal params from the given request
    fn create_withdrawal_params(&self, request: &GmWithdrawalRequest, execution_fee: U256) -> Result<(exchange_router_utils::CreateWithdrawalParams, U256)> {
        let market_token_amount = self.decimal_to_u256(request.amount, 18)?; // Always 18 decimals for GM market tokens

        let withdrawal_params = exchange_router_utils::CreateWithdrawalParams {
            receiver: self.wallet_manager.address,
            callback_contract: Address::zero(),
            ui_fee_receiver: Address::zero(),
            market: request.market,
            long_token_swap_path: vec![], 
            short_token_swap_path: vec![],
            min_long_token_amount: U256::zero(),
            min_short_token_amount: U256::zero(),
            should_unwrap_native_token: false,
            execution_fee,
            callback_gas_limit: U256::zero(),
        };
        Ok((withdrawal_params, market_token_amount))
    }

    /// Creates GM shift params from the given request
    fn create_shift_params(&self, request: &GmShiftRequest, execution_fee: U256) -> Result<(exchange_router_utils::CreateShiftParams, U256)> {
        let from_market_amount = self.decimal_to_u256(request.amount, 18)?; // Always 18 decimals for GM market tokens  
        
        let shift_params = exchange_router_utils::CreateShiftParams {
            receiver: self.wallet_manager.address,
            callback_contract: Address::zero(),
            ui_fee_receiver: Address::zero(),
            from_market: request.from_market,
            to_market: request.to_market,
            min_market_tokens: U256::zero(),
            execution_fee,
            callback_gas_limit: U256::zero(),
        };
        Ok((shift_params, from_market_amount))
    }

    // Helper to convert Decimal to U256
    fn decimal_to_u256(&self, value: Decimal, decimals: u8) -> Result<U256> {
        let value_str = value.to_string();
        let formatted = ethers::utils::parse_units(&value_str, decimals as usize)
            .map_err(|e| eyre::eyre!("Failed to parse decimal value: {}", e))?;
        
        match formatted {
            ethers::utils::ParseUnits::U256(u256_val) => Ok(u256_val),
            _ => Err(eyre::eyre!("Unexpected parse result type")),
        }
    }

    /// Helper to convert U256 to Decimal
    pub fn u256_to_decimal(&self, value: U256, decimals: u8) -> Result<Decimal> {
        let formatted = ethers::utils::format_units(value, decimals as usize)
            .map_err(|e| eyre::eyre!("Failed to format U256 value: {}", e))?;
        Decimal::from_str(&formatted).map_err(|e| eyre::eyre!("Failed to parse formatted value: {}", e))
    }
}

