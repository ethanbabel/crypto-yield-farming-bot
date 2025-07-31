use ethers::types::{
    U256, TransactionRequest, U64,
    TxHash, TransactionReceipt,
    transaction::eip2718::TypedTransaction,
};
use ethers::prelude::*;
use tracing::{debug, info, warn, instrument};
use eyre::Result;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;

use crate::config::Config;
use crate::wallet::WalletManager;
use crate::constants::WNT_ADDRESS;
use super::types::{SwapRequest, QuoteRequest, QuoteResponse};
use super::paraswap_api_client::ParaSwapClient;

// Add ERC20 ABI for approve function
abigen!(
    IERC20Approve,
    r#"[
        function approve(address spender, uint256 amount) external returns (bool)
        function allowance(address owner, address spender) external view returns (uint256)
    ]"#
);

// Add WETH9 ABI for wrap/unwrap functions
abigen!(
    WETH9,
    r#"[
        function deposit() public payable
        function withdraw(uint wad) public
    ]"#
);

const MAX_FEE_PER_GAS_BUFFER: f64 = 1.1; // 10% above the current gas price

pub struct SwapManager {
    paraswap_client: ParaSwapClient,
    wallet_manager: WalletManager,
    chain_id: u64,
    max_fee_per_gas_buffer: Decimal,
}

impl SwapManager {
    pub fn new(wallet_manager: WalletManager, config: &Config) -> Self {
        let paraswap_client = ParaSwapClient::new(wallet_manager.address.clone(), config);
        let chain_id = config.chain_id;
        let max_fee_per_gas_buffer = Decimal::from_f64(MAX_FEE_PER_GAS_BUFFER).unwrap();
        Self {
            paraswap_client,
            wallet_manager,
            chain_id,
            max_fee_per_gas_buffer,
        }
    }

    /// Executes a swap request using the ParaSwap API and smart contract - assumes wallet manager tokens have been loaded
    #[instrument(skip(self, swap_request), fields(on_close = true))]
    pub async fn execute_swap(&self, swap_request: &SwapRequest) -> Result<()> {
        let (swap_log_string, quote_request) = self.validate_swap_request(swap_request).await?;

        // Check if this is an ETH/WETH swap
        let weth_address = Address::from_str(WNT_ADDRESS).unwrap();
        if let Some(is_wrap) = self.is_eth_weth_swap(
            quote_request.from_token,
            quote_request.to_token,
            weth_address,
        ) {
            return self.execute_eth_weth_swap(swap_request, is_wrap, &swap_log_string).await;
        }

        // Get initial balances
        let initial_native_balance = self.wallet_manager.get_native_balance().await?;
        let initial_from_balance = if quote_request.from_token == self.wallet_manager.native_token.address {
            initial_native_balance
        } else {
            self.wallet_manager.get_token_balance(quote_request.from_token).await?
        };
        let initial_to_balance = if quote_request.to_token == self.wallet_manager.native_token.address {
            initial_native_balance
        } else {
            self.wallet_manager.get_token_balance(quote_request.to_token).await?
        };

        info!(
            initial_from_balance = %initial_from_balance,
            initial_to_balance = %initial_to_balance,
            initial_native_balance = %initial_native_balance,
            "{} Swap Initiated", 
            swap_log_string
        );

        // Fetch the swap quote from ParaSwap
        let mut quote = self.paraswap_client.get_quote(&quote_request).await?;
        debug!(quote = ?quote, "{} Quote Received", swap_log_string);

        // Validate the transaction
        self.validate_transaction(&quote, initial_from_balance).await?;
        debug!("{} Transaction Validated", swap_log_string);

        // Handle token approval if needed (for ERC20 tokens)
        if quote.from_token != self.wallet_manager.native_token.address {
            self.ensure_token_approval(&quote, quote_request.from_token_decimals).await?;
        }
        debug!("{} Token Approval Ensured", swap_log_string);

        // Build the transaction
        let tx = self.build_transaction(&mut quote).await?;
        debug!(transaction = ?tx, "{} Transaction Built", swap_log_string);

        // Simulate the transaction to ensure it will succeed
        self.simulate_transaction(&tx, initial_native_balance).await?;
        debug!("{} Transaction Simulation Successful", swap_log_string);

        // Execute the transaction
        let (tx_hash, receipt) = self.execute_transaction(tx).await?;
        let gas_used = self.u256_to_decimal(receipt.gas_used.unwrap_or(U256::zero()), 0)?;
        let gas_price = self.u256_to_decimal(receipt.effective_gas_price.unwrap_or(U256::zero()), 18)?;
        info!(
            tx_hash = ?tx_hash,
            block_number = ?receipt.block_number.unwrap_or(U64::zero()),
            gas_used = ?gas_used,
            gas_price = ?gas_price,
            gas_cost = ?gas_used * gas_price,
            "{} Swap Executed Successfully",
            swap_log_string,
        );

        // Get final balances
        let final_native_balance = self.wallet_manager.get_native_balance().await?;
        let final_from_balance = if quote.from_token == self.wallet_manager.native_token.address {
            final_native_balance
        } else {
            self.wallet_manager.get_token_balance(quote.from_token).await?
        };
        let final_to_balance = if quote.to_token == self.wallet_manager.native_token.address {
            final_native_balance
        } else {
            self.wallet_manager.get_token_balance(quote.to_token).await?
        };

        info!(
            final_from_balance = %final_from_balance,
            final_to_balance = %final_to_balance,
            final_native_balance = %final_native_balance,
            "{} Swap Completed",
            swap_log_string
        );

        Ok(())
    }

    /// Validate swap request, create log string and quote request
    #[instrument(skip(self, swap_request))]
    pub async fn validate_swap_request(&self, swap_request: &SwapRequest) -> Result<(String, QuoteRequest)> {
        if swap_request.side != "BUY" && swap_request.side != "SELL" {
            return Err(eyre::eyre!("Invalid swap side: {}", swap_request.side));
        }
        let from_token = if swap_request.from_token_address == self.wallet_manager.native_token.address {
            &self.wallet_manager.native_token
        } else {
            self.wallet_manager.all_tokens.get(&swap_request.from_token_address).ok_or_else(|| {
                eyre::eyre!("From token not found in wallet manager: {:?}", swap_request.from_token_address)
            })?
        };
        let to_token = if swap_request.to_token_address == self.wallet_manager.native_token.address {
            &self.wallet_manager.native_token
        } else {
            self.wallet_manager.all_tokens.get(&swap_request.to_token_address).ok_or_else(|| {
                eyre::eyre!("To token not found in wallet manager: {:?}", swap_request.to_token_address)
            })?
        };

        let swap_log_string = format!(
            "SWAP REQUEST | {} -> {} | {} {} {} |",
            from_token.symbol, to_token.symbol, swap_request.side, swap_request.amount,
            if swap_request.side == "BUY" { &to_token.symbol } else { &from_token.symbol }
        );

        let request = QuoteRequest {
            from_token: from_token.address,
            from_token_decimals: from_token.decimals,
            to_token: to_token.address,
            to_token_decimals: to_token.decimals,
            amount: swap_request.amount,
            side: swap_request.side.clone(),
            slippage_tolerance: Decimal::from_f64(0.5).unwrap(), // Default 0.5% slippage
        };

        Ok((swap_log_string, request))
    }

    /// Validate that we have sufficient balance for the swap
    #[instrument(skip(self, quote))]
    async fn validate_transaction(&self, quote: &QuoteResponse, from_token_balance: Decimal) -> Result<()> {
        debug!(
            from_token = ?quote.from_token,
            to_token = ?quote.to_token,
            from_amount = %quote.from_amount,
            to_amount = %quote.to_amount,
            "Validating transaction requirements"
        );
        
        if from_token_balance < quote.from_amount {
            return Err(eyre::eyre!(
                "Insufficient balance: need {} but have {} of token {:?}",
                quote.from_amount,
                from_token_balance,
                quote.from_token
            ));
        }
        
        debug!(
            from_token_balance = %from_token_balance,
            required_amount = %quote.from_amount,
            "Transaction validation successful"
        );
        
        Ok(())
    }

    /// Ensure the ParaSwap contract has sufficient allowance to spend our tokens
    #[instrument(skip(self, quote, from_token_decimals))]
    async fn ensure_token_approval(&self, quote: &QuoteResponse, from_token_decimals: u8) -> Result<()> {
        debug!("Checking token approval for ParaSwap contract");
        
        let token_contract = IERC20Approve::new(quote.from_token, self.wallet_manager.signer.clone());
        
        // Check current allowance
        let current_allowance = token_contract
            .allowance(self.wallet_manager.address, quote.to_contract)
            .call()
            .await?;

        let required_amount = self.decimal_to_u256(quote.from_amount, from_token_decimals)?;

        if current_allowance < required_amount {
            debug!(
                current_allowance = %current_allowance,
                required_amount = %required_amount,
                "Insufficient allowance, approving tokens"
            );
            
            // Approve maximum amount to avoid repeated approvals
            let max_approval = U256::MAX;
            let approve_tx = token_contract.approve(quote.to_contract, max_approval);
            
            let pending_tx = approve_tx.send().await?;
            let receipt = pending_tx.await?;
            
            match receipt {
                Some(receipt) => {
                    if receipt.status == Some(U64::from(1)) {
                        debug!(
                            tx_hash = ?receipt.transaction_hash,
                            "Token approval successful"
                        );
                    } else {
                        return Err(eyre::eyre!("Token approval failed: {:?}", receipt));
                    }
                }
                None => {
                    return Err(eyre::eyre!("Token approval receipt not found"));
                }
            }
        } else {
            debug!(
                current_allowance = %current_allowance,
                required_amount = %required_amount,
                "Sufficient allowance already exists"
            );
        }
        
        Ok(())
    }

    /// Build the transaction from the quote
    #[instrument(skip(self, quote))]
    async fn build_transaction(&self, quote: &mut QuoteResponse) -> Result<TransactionRequest> {
        debug!("Building transaction from quote");

        // Build the transaction request
        let tx = TransactionRequest::new()
            .to(quote.to_contract)
            .from(self.wallet_manager.address)
            .data(quote.transaction_data.clone())
            .value(
                if quote.from_token == self.wallet_manager.native_token.address {
                    quote.value
                } else {
                    U256::zero()
                }
            )
            .chain_id(self.chain_id);

        debug!(
            from_token = ?quote.from_token,
            to_token = ?quote.to_token,
            from_amount = %quote.from_amount,
            to_amount = %quote.to_amount,
            native_token = ?self.wallet_manager.native_token.address,
            tx_value = %tx.value.unwrap_or(U256::zero()),
            tx = ?tx,
            "Transaction value calculation"
        );

        // Estimate gas using the provider
        let gas_estimate = self.wallet_manager.signer.provider().estimate_gas(&tx.clone().into(), None).await?;
        let gas_price_decimal = self.u256_to_decimal(self.wallet_manager.signer.provider().get_gas_price().await?, 0)?;
        let gas_price_with_buf = gas_price_decimal * self.max_fee_per_gas_buffer;
        let gas_price = self.decimal_to_u256(gas_price_with_buf, 0)?;

        // Set tx gas limit and gas price
        let tx = tx.gas(gas_estimate).gas_price(gas_price);
        
        debug!(
            to = ?quote.to_contract,
            value = %tx.value.unwrap_or(U256::zero()),
            gas_price = %tx.gas_price.unwrap_or(U256::zero()),
            gas_limit = %tx.gas.unwrap_or(U256::zero()),
            "Transaction built"
        );
        
        Ok(tx)
    }

    /// Simulate the transaction to ensure it will succeed
    #[instrument(skip(self, tx))]
    async fn simulate_transaction(&self, tx: &TransactionRequest, native_balance: Decimal) -> Result<()> {
        debug!("Simulating transaction");

        // Get gas limits from transaction request
        let gas_dec = self.u256_to_decimal(tx.gas.unwrap_or(U256::zero()), 0)?;
        let gas_price_dec = self.u256_to_decimal(tx.gas_price.unwrap_or(U256::zero()), 18)?;
        let gas_cost_limit = gas_dec * gas_price_dec;

        if native_balance < gas_cost_limit {
            return Err(eyre::eyre!(
                "Insufficient native balance for gas: have {}, need {}",
                native_balance,
                gas_cost_limit
            ));
        }

        debug!(
            gas_limit = %gas_dec,
            gas_price = %gas_price_dec,
            gas_cost_limit = %gas_cost_limit,
            native_balance = %native_balance,
            "Simulating transaction with gas limits"
        );


        // Convert TransactionRequest to TypedTransaction
        let typed_tx: TypedTransaction = tx.clone().into();
        
        // Use eth_call to simulate the transaction
        match self.wallet_manager.signer.provider().call(&typed_tx, None).await {
            Ok(_) => {
                debug!("Transaction simulation successful");
                Ok(())
            }
            Err(e) => {
                warn!(error = ?e, tx = ?typed_tx, "Transaction simulation failed");
                Err(eyre::eyre!("Transaction simulation failed: {}", e))
            }
        }
    }

    /// Execute the transaction
    #[instrument(skip(self, tx))]
    async fn execute_transaction(&self, tx: TransactionRequest) -> Result<(TxHash, TransactionReceipt)> {
        debug!("Executing transaction");
        
        // Send the transaction
        let pending_tx = self.wallet_manager.signer.send_transaction(tx, None).await?;
        let tx_hash = pending_tx.tx_hash();
        
        debug!(
            tx_hash = %tx_hash,
            "Transaction sent, waiting for confirmation"
        );
        
        // Wait for confirmation
        let receipt = match pending_tx.await? {
            Some(receipt) => {
                if receipt.status == Some(U64::from(1)) {
                    receipt
                } else {
                    return Err(eyre::eyre!("Transaction failed: {:?}", receipt));
                }
            }
            None => {
                return Err(eyre::eyre!("Transaction receipt not found"));
            }
        };
        
        Ok((tx_hash, receipt))
    }

    /// Helper to convert Decimal to U256
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

    /// Execute ETH/WETH wrap or unwrap operation
    #[instrument(skip(self, swap_request, is_wrap, swap_log_string))]
    async fn execute_eth_weth_swap(
        &self,
        swap_request: &SwapRequest,
        is_wrap: bool,
        swap_log_string: &str,
    ) -> Result<()> {
        let weth_address = Address::from_str(WNT_ADDRESS).unwrap();

        // Get initial balances
        let initial_native_balance = self.wallet_manager.get_native_balance().await?;
        let initial_weth_balance = self.wallet_manager.get_token_balance(weth_address).await?;

        info!(
            initial_eth_balance = %initial_native_balance,
            initial_weth_balance = %initial_weth_balance,
            "{} ETH/WETH Operation Initiated",
            swap_log_string
        );

        // Validate we have sufficient balance
        if is_wrap {
            if initial_native_balance < swap_request.amount {
                return Err(eyre::eyre!(
                    "Insufficient ETH balance for wrap: need {} but have {}",
                    swap_request.amount,
                    initial_native_balance
                ));
            }
        } else {
            if initial_weth_balance < swap_request.amount {
                return Err(eyre::eyre!(
                    "Insufficient WETH balance for unwrap: need {} but have {}",
                    swap_request.amount,
                    initial_weth_balance
                ));
            }
        }
        debug!("{} ETH/WETH Operation Validated", swap_log_string);

        // Build the transaction
        let tx = self.build_weth_transaction(weth_address, is_wrap, swap_request.amount).await?;
        debug!(transaction = ?tx, "{} ETH/WETH Operation Transaction Built", swap_log_string);

        // Simulate the transaction
        self.simulate_transaction(&tx, initial_native_balance).await?;
        debug!("{} ETH/WETH Operation Simulation Successful", swap_log_string);

        // Execute the transaction
        let (tx_hash, receipt) = self.execute_transaction(tx).await?;
        let gas_used = self.u256_to_decimal(receipt.gas_used.unwrap_or(U256::zero()), 0)?;
        let gas_price = self.u256_to_decimal(receipt.effective_gas_price.unwrap_or(U256::zero()), 18)?;
        info!(
            tx_hash = ?tx_hash,
            block_number = ?receipt.block_number.unwrap_or(U64::zero()),
            gas_used = ?gas_used,
            gas_price = ?gas_price,
            gas_cost = ?gas_used * gas_price,
            "{} ETH/WETH Operation Executed Successfully",
            swap_log_string,
        );

        // Get final balances
        let final_native_balance = self.wallet_manager.get_native_balance().await?;
        let final_weth_balance = self.wallet_manager.get_token_balance(weth_address).await?;

        info!(
            final_eth_balance = %final_native_balance,
            final_weth_balance = %final_weth_balance,
            "{} ETH/WETH Operation Completed",
            swap_log_string
        );

        Ok(())
    }

    /// Check if a swap is between ETH and WETH
    fn is_eth_weth_swap(
        &self,
        from_token: Address,
        to_token: Address,
        weth_address: Address,
    ) -> Option<bool> {
        if from_token == self.wallet_manager.native_token.address && to_token == weth_address {
            Some(true) // wrap ETH to WETH
        } else if from_token == weth_address && to_token == self.wallet_manager.native_token.address {
            Some(false) // unwrap WETH to ETH
        } else {
            None // not an ETH/WETH swap
        }
    }

    /// Build transaction for wrapping or unwrapping WETH
    #[instrument(skip(self))]
    async fn build_weth_transaction(&self, weth_address: Address, is_wrap: bool, amount: Decimal) -> Result<TransactionRequest> {
        let tx = if is_wrap {
            WETH9::new(weth_address, self.wallet_manager.signer.clone())
                .deposit()
                .value(self.decimal_to_u256(amount, 18).unwrap())
        } else {
            WETH9::new(weth_address, self.wallet_manager.signer.clone())
                .withdraw(self.decimal_to_u256(amount, 18).unwrap())
        };

        let mut tx_request: TransactionRequest = tx.tx.clone().into();
        tx_request = tx_request.from(self.wallet_manager.address).chain_id(self.chain_id);

        // Estimate gas using the provider
        let gas_estimate = self.wallet_manager.signer.provider().estimate_gas(&tx_request.clone().into(), None).await?;
        let gas_price_decimal = self.u256_to_decimal(self.wallet_manager.signer.provider().get_gas_price().await?, 0)?;
        let gas_price_with_buf = gas_price_decimal * self.max_fee_per_gas_buffer;
        let gas_price = self.decimal_to_u256(gas_price_with_buf, 0)?;

        // Set tx gas limit and gas price
        tx_request = tx_request.gas(gas_estimate).gas_price(gas_price);
        
        debug!(
            to = ?weth_address,
            value = %tx_request.value.unwrap_or(U256::zero()),
            gas_price = %tx_request.gas_price.unwrap_or(U256::zero()),
            gas_limit = %tx_request.gas.unwrap_or(U256::zero()),
            "Transaction built"
        );
        
        Ok(tx_request)
    }
}