use eyre::Result;
use std::sync::Arc;
use std::collections::HashMap;
use tracing::{instrument, debug, info};
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use ethers::prelude::*;
use ethers::types::{
    TransactionRequest, Bytes, TxHash, TransactionReceipt, 
    transaction::eip2718::TypedTransaction
};
use futures::stream::{self, StreamExt};
use dydx::{
    config::ClientConfig,
    node::{
        NodeClient,
        // Account,
    },
    indexer::{
        IndexerClient,
        types::{Ticker, PerpetualMarket, Denom},
    },
};
use cosmrs::{
    crypto::secp256k1,
    // AccountId,
};
use hex;

use crate::config;
use crate::wallet::WalletManager;
use super::hedge_utils;
use super::skip_go;

const MAX_FEE_PER_GAS_BUFFER: f64 = 1.05; // 5% above the current gas price

const ARBITRUM_CHAIN_ID: &str = "42161";
const DYDX_CHAIN_ID: &str = "dydx-mainnet-1";
const ARBITRUM_USDC_DENOM: &str = "0xaf88d065e77c8cC2239327C5EDb3A432268e5831";
const DYDX_USDC_DENOM: &str = "ibc/8E27BA2D5493AF5636760E354E46004562C46AB7EC0CC4C1CA14E9E20E2545B5";
const USDC_DECIMALS: u8 = 6;

// ERC20 ABI for token approvals
abigen!(
    IERC20Approve,
    r#"[
        function approve(address spender, uint256 amount) external returns (bool)
        function allowance(address owner, address spender) external view returns (uint256)
    ]"#
);

pub struct DydxClient {
    config: Arc<config::Config>,
    wallet_manager: Arc<WalletManager>,
    node_client: NodeClient,
    indexer_client: IndexerClient,
    dydx_address: String,
}

impl DydxClient {
    pub async fn new(cfg: Arc<config::Config>, wallet_manager: Arc<WalletManager>) -> Result<Self> {
        // Initialize crypto provider
        config::init_crypto_provider();

        let config = ClientConfig::from_file("src/hedging/dydx_mainnet.toml")
            .await
            .map_err(|e| eyre::eyre!("Failed to load dYdX config: {}", e))?;
        let node_client = NodeClient::connect(config.node)
            .await
            .map_err(|e| eyre::eyre!("Failed to connect to dYdX node: {}", e))?;
        let indexer_client = IndexerClient::new(config.indexer);

        // Manually derive dydx account from private key
        let address = derive_cosmos_address_from_key(&cfg, "dydx")?;

        // let account = node_client.get_account(&address.to_string().into()).await
        //     .map_err(|e| eyre::eyre!("Failed to fetch dYdX account for address {}: {}", address, e))?;
        // info!(account = ?account, "dYdX account fetched successfully");
        
        Ok(Self {
            config: cfg,
            wallet_manager,
            node_client,
            indexer_client,
            dydx_address: address,
        })
    }

    #[instrument(skip(self))]
    pub async fn get_perpetual_markets(&self) -> Result<HashMap<Ticker, PerpetualMarket>> {
        let markets = self.indexer_client.markets().get_perpetual_markets(None).await
            .map_err(|e| eyre::eyre!("Failed to fetch perpetual markets: {}", e))?;
        Ok(markets)
    }

    pub async fn get_perpetual_market(&self, long_token: &str) -> Result<Option<PerpetualMarket>> {
        if hedge_utils::STABLE_COINS.contains(&long_token) {
            return Ok(None);
        }
        let ticker = hedge_utils::get_dydx_perp_ticker(long_token);
        match self.indexer_client.markets().get_perpetual_market(&ticker.into()).await {
            Ok(market) => Ok(Some(market)),
            Err(e) => {
                if e.to_string().contains("400 Bad Request") {
                    Ok(None)
                } else {
                    Err(eyre::eyre!("Failed to fetch perpetual market for {}: {}", long_token, e))
                }
            }
        }
    }

    #[instrument(skip(self))]
    pub async fn get_token_perp_map(&self) -> Result<HashMap<String, Option<PerpetualMarket>>> {
        let token_symbols: Vec<String> = self.wallet_manager.asset_tokens.values()
            .map(|token| token.symbol.clone())
            .collect();

        let results: Vec<_> = stream::iter(token_symbols)
            .map(|token_symbol| async move {
                let market = self.get_perpetual_market(&token_symbol).await;
                (token_symbol, market)
            })
            .buffer_unordered(10)
            .collect()
            .await;

        let mut token_perp_map = HashMap::new();
        for (token_symbol, market_result) in results {
            match market_result {
                Ok(market) => {
                    token_perp_map.insert(token_symbol, market);
                }
                Err(e) => {
                    return Err(eyre::eyre!("Error fetching market for {}: {}", token_symbol, e));
                }
            }
        }
        Ok(token_perp_map)
    }

    #[instrument(skip(self))]
    pub async fn dydx_deposit(
        &mut self, 
        amount_in: Option<Decimal>, 
        amount_out: Option<Decimal>, 
        go_fast: bool,
        slippage_tolerance_percent: Option<Decimal>,
    ) -> Result<()> {
        // Get USDC + ETH balances
        let initial_arbitrum_usdc_balance = self.get_arbitrum_usdc_balance().await?;
        let initial_dydx_usdc_balance = self.get_dydx_usdc_balance().await?;
        let initial_arbitrum_eth_balance = self.wallet_manager.get_native_balance().await?;

        // Sanity check request
        let log_string = self.get_deposit_log_string(
            initial_arbitrum_usdc_balance,
            amount_in,
            amount_out,
        ).await?;

        info!(
            initial_arbitrum_usdc_balance = ?initial_arbitrum_usdc_balance,
            initial_dydx_usdc_balance = ?initial_dydx_usdc_balance,
            initial_arbitrum_eth_balance = ?initial_arbitrum_eth_balance,
            "{} Deposit Initiated", 
            log_string
        );

        // Get SkipGo route and msgs
        let (amount, msgs) = self.skip_go_get_route_and_msgs(
            amount_in,
            amount_out,
            go_fast,
            slippage_tolerance_percent,
            ARBITRUM_USDC_DENOM,
            ARBITRUM_CHAIN_ID,
            DYDX_USDC_DENOM,
            DYDX_CHAIN_ID,
        ).await?;

        // Validate requested transfer amount and estimated fees against balances
        let mut combined_gas_fees = Decimal::ZERO;
        for tx in &msgs.txs {
            if let skip_go::SkipGoTx::EvmTx(evm_tx) = tx {
                let gas_fee_decimal = u256_to_decimal(
                    U256::from_dec_str(&evm_tx.evm_tx.value).unwrap(),
                    USDC_DECIMALS,
                )?;
                combined_gas_fees += gas_fee_decimal;
            }
        }
        if amount > initial_arbitrum_usdc_balance || combined_gas_fees > initial_arbitrum_eth_balance {
            return Err(eyre::eyre!(
                "Insufficient Arbitrum USDC balance for deposit: have {}, need {}",
                initial_arbitrum_usdc_balance,
                amount
            ));
        } else {
            info!(
                initial_arbitrum_usdc_balance = ?initial_arbitrum_usdc_balance,
                deposit_amount_including_fees = ?amount,
                "{} Deposit Validated", 
                log_string
            );
        }

        // Execute SkipGo transfer
        self.execute_skip_go_transfer(msgs.txs, initial_arbitrum_eth_balance, &log_string).await?;

        // Final balances
        let final_arbitrum_usdc_balance = self.get_arbitrum_usdc_balance().await?;
        let final_dydx_usdc_balance = self.get_dydx_usdc_balance().await?;
        let final_arbitrum_eth_balance = self.wallet_manager.get_native_balance().await?;
        let arbitrum_usdc_diff = final_arbitrum_usdc_balance - initial_arbitrum_usdc_balance;
        let dydx_usdc_diff = final_dydx_usdc_balance - initial_dydx_usdc_balance;
        let arbitrum_eth_diff = final_arbitrum_eth_balance - initial_arbitrum_eth_balance;
        info!(
            final_arbitrum_usdc_balance = ?final_arbitrum_usdc_balance,
            final_dydx_usdc_balance = ?final_dydx_usdc_balance,
            final_arbitrum_eth_balance = ?final_arbitrum_eth_balance,
            "{} Deposit Executed Successfully \n Arbitrum USDC Change: {} | dYdX USDC Change: {} | Arbitrum ETH Change: {}", 
            log_string, 
            arbitrum_usdc_diff,
            dydx_usdc_diff,
            arbitrum_eth_diff
        );

        Ok(())
    }

    #[instrument(skip(self))]
    pub async fn dydx_withdrawal(
        &mut self, 
        amount_in: Option<Decimal>, 
        amount_out: Option<Decimal>, 
        go_fast: bool,
        slippage_tolerance_percent: Option<Decimal>,
    ) -> Result<()> {
        // Get USDC balances
        let initial_arbitrum_usdc_balance = self.get_arbitrum_usdc_balance().await?;
        let initial_dydx_usdc_balance = self.get_dydx_usdc_balance().await?;

        // Sanity check request
        let log_string = self.get_withdrawal_log_string(
            initial_dydx_usdc_balance,
            amount_in,
            amount_out,
        ).await?;

        info!(
            initial_arbitrum_usdc_balance = ?initial_arbitrum_usdc_balance,
            initial_dydx_usdc_balance = ?initial_dydx_usdc_balance,
            "{} Withdrawal Initiated", 
            log_string
        );

        let (amount, msgs) = self.skip_go_get_route_and_msgs(
            amount_in,
            amount_out,
            go_fast,
            slippage_tolerance_percent,
            DYDX_USDC_DENOM,
            DYDX_CHAIN_ID,
            ARBITRUM_USDC_DENOM,
            ARBITRUM_CHAIN_ID,
        ).await?;

        // Validate requested transfer amount and estimated fees against balances
        if amount > initial_dydx_usdc_balance {
            return Err(eyre::eyre!(
                "Insufficient dYdX USDC balance for withdrawal: have {}, need {}",
                initial_dydx_usdc_balance,
                amount
            ));
        } else {
            info!(
                initial_dydx_usdc_balance = ?initial_dydx_usdc_balance,
                withdrawal_amount_including_fees = ?amount,
                "{} Withdrawal Validated", 
                log_string
            );
        }

        Ok(())
    }

    async fn get_arbitrum_usdc_balance(&self) -> Result<Decimal> {
        let balance = self.wallet_manager.get_token_balance(
            Address::from_str(ARBITRUM_USDC_DENOM)?
        ).await?;
        Ok(balance)
    }

    async fn get_dydx_usdc_balance(&mut self) -> Result<Decimal> {
        let balance = self.node_client.get_account_balance(
            &self.dydx_address.to_string().into(),
            &Denom::Usdc
        ).await
        .map_err(|e| eyre::eyre!("Failed to fetch dYdX USDC balance: {}", e))?;
        let balance_decimal = Decimal::from_str(&balance.amount)?;
        Ok(balance_decimal)
    }

    async fn get_deposit_log_string(
        &self,
        arbitrum_usdc_balance: Decimal,
        amount_in: Option<Decimal>, 
        amount_out: Option<Decimal>
    ) -> Result<String> {
        match (amount_in, amount_out) {
            (Some(_), Some(_)) => Err(eyre::eyre!("Specify only one of amount_in or amount_out")),
            (Some(amount), None) => {
                if amount > arbitrum_usdc_balance {
                    return Err(eyre::eyre!("Insufficient USDC balance for deposit: have {}, need {}", arbitrum_usdc_balance, amount));
                }
                let log_string = format!("DYDX DEPOSIT REQUEST | Deposit {} (amount in) USDC |", amount);
                Ok(log_string)
            }
            (None, Some(amount)) => {
                if amount > arbitrum_usdc_balance {
                    return Err(eyre::eyre!("Insufficient USDC balance for deposit: have {}, need {}", arbitrum_usdc_balance, amount));
                }
                let log_string = format!("DYDX DEPOSIT REQUEST | Deposit {} (amount out) USDC |", amount);
                Ok(log_string)
            }
            (None, None) => Err(eyre::eyre!("Must specify one of amount_in or amount_out")),
        }
    }

    async fn get_withdrawal_log_string(
        &self,
        dydx_usdc_balance: Decimal,
        amount_in: Option<Decimal>, 
        amount_out: Option<Decimal>
    ) -> Result<String> {
        match (amount_in, amount_out) {
            (Some(_), Some(_)) => Err(eyre::eyre!("Specify only one of amount_in or amount_out")),
            (Some(amount), None) => {
                if amount > dydx_usdc_balance {
                    return Err(eyre::eyre!("Insufficient USDC balance for withdrawal: have {}, need {}", dydx_usdc_balance, amount));
                }
                let log_string = format!("DYDX WITHDRAWAL REQUEST | Withdraw {} (amount in) USDC |", amount);
                Ok(log_string)
            }
            (None, Some(amount)) => {
                if amount > dydx_usdc_balance {
                    return Err(eyre::eyre!("Insufficient USDC balance for withdrawal: have {}, need {}", dydx_usdc_balance, amount));
                }
                let log_string = format!("DYDX WITHDRAWAL REQUEST | Withdraw {} (amount out) USDC |", amount);
                Ok(log_string)
            }
            (None, None) => Err(eyre::eyre!("Must specify one of amount_in or amount_out")),
        }
    }

    async fn skip_go_get_route_and_msgs(
        &self,
        amount_in: Option<Decimal>,
        amount_out: Option<Decimal>,
        go_fast: bool,
        slippage_tolerance_percent: Option<Decimal>,
        source_asset_denom: &str,
        source_asset_chain_id: &str,
        dest_asset_denom: &str,
        dest_asset_chain_id: &str,
    ) -> Result<(Decimal, skip_go::SkipGoGetMsgsResponse)> {
        let amount_in: Option<String> = amount_in.map(|d| {
            let amount_u256 = decimal_to_u256(d, USDC_DECIMALS).unwrap();
            amount_u256.to_string()
        });
        let amount_out: Option<String> = amount_out.map(|d| {
            let amount_u256 = decimal_to_u256(d, USDC_DECIMALS).unwrap();
            amount_u256.to_string()
        });

        let route_request = skip_go::SkipGoGetRouteRequest {
            source_asset_denom: source_asset_denom.to_string(),
            source_asset_chain_id: source_asset_chain_id.to_string(),
            dest_asset_denom: dest_asset_denom.to_string(),
            dest_asset_chain_id: dest_asset_chain_id.to_string(),
            amount_in: amount_in.clone(),
            amount_out: amount_out.clone(),
            go_fast: Some(go_fast),
            ..Default::default()
        };
        debug!("SkipGo Route Request: {:#?}", route_request);
        let route = skip_go::get_route(route_request).await?;
        debug!("SkipGo Route Response: {:#?}", route);

        let true_amount_in = route["amount_in"].as_str().unwrap();
        let true_amount_out = route["amount_out"].as_str().unwrap();
        let required_chain_addresses = route["required_chain_addresses"].clone();
        let operations = route["operations"].clone();

        let mut address_list = Vec::<String>::new();
        for chain in required_chain_addresses.as_array().unwrap() {
            let chain_str = chain.as_str().unwrap();
            if !chain_str.chars().all(|c| c.is_numeric()) {
                let chain_prefix = chain_str.find('-').map(|idx| &chain_str[..idx]).unwrap_or(chain_str);
                let address = derive_cosmos_address_from_key(&self.config, chain_prefix)?;
                address_list.push(address);
            } else {
                address_list.push(ethers::utils::to_checksum(&self.wallet_manager.address, None));
            }
        }

        let msg_request = skip_go::SkipGoGetMsgsRequest {
            source_asset_denom: source_asset_denom.to_string(),
            source_asset_chain_id: source_asset_chain_id.to_string(),
            dest_asset_denom: dest_asset_denom.to_string(),
            dest_asset_chain_id: dest_asset_chain_id.to_string(),
            amount_in: true_amount_in.to_string(),
            amount_out: true_amount_out.to_string(),
            address_list,
            operations,
            slippage_tolerance_percent: slippage_tolerance_percent.map(|d| d.to_string()),
            enable_gas_warnings: Some(true),
            ..Default::default()
        };
        debug!("SkipGo Msgs Request: {:#?}", msg_request);
        let msgs = skip_go::get_msgs(msg_request).await?;
        info!("SkipGo Msgs Response: {:#?}", msgs);

        let amount = u256_to_decimal(
            U256::from_dec_str(true_amount_in).unwrap(),
            USDC_DECIMALS,
        )?;

        Ok((amount, msgs))
    }

    async fn execute_skip_go_transfer(
        &self, 
        txs: Vec<skip_go::SkipGoTx>, 
        arbitrum_native_balance: Decimal,
        log_string: &String
    ) -> Result<()> {
        for tx in txs {
            match tx {
                skip_go::SkipGoTx::CosmosTx(cosmos_tx) => {
                    debug!("Executing Cosmos Tx: {:#?}", cosmos_tx);

                    // Not yet implemented
                    return Err(eyre::eyre!("No support for Cosmos transactions yet: {:#?}", cosmos_tx));

                }
                skip_go::SkipGoTx::EvmTx(evm_tx) => {
                    debug!("Executing EVM Tx: {:#?}", evm_tx);

                    for required_approval in &evm_tx.evm_tx.required_erc20_approvals {
                        self.approve_erc20(required_approval.clone()).await?;
                    }

                    let evm_transaction = self.build_evm_transaction(evm_tx).await?;
                    debug!(transaction = ?evm_transaction, "{} Transaction Built", log_string);

                    self.simulate_evm_transaction(&evm_transaction, arbitrum_native_balance).await?;
                    debug!(transaction = ?evm_transaction, "{} Transaction Simulated", log_string);

                    let (tx_hash, receipt) = self.execute_evm_transaction(evm_transaction).await?;
                    let gas_used = u256_to_decimal(receipt.gas_used.unwrap_or(U256::zero()), 0)?;
                    let gas_price = u256_to_decimal(receipt.effective_gas_price.unwrap_or(U256::zero()), 18)?;
                    let total_gas_cost = gas_used * gas_price;
                    info!(
                        tx_hash = ?tx_hash,
                        gas_used = ?gas_used,
                        gas_price = ?gas_price,
                        total_gas_cost = ?total_gas_cost,
                        total_gas_cost_usd = ?(total_gas_cost * self.wallet_manager.native_token.last_mid_price_usd),
                        "{} Transaction Executed Successfully", 
                        log_string
                    );
                }
                skip_go::SkipGoTx::SvmTx(svm_tx) => {
                    return Err(eyre::eyre!("No support for SVM transactions: {:#?}", svm_tx));
                }
            }
        }
        Ok(())
    }

    async fn approve_erc20(&self, approval: skip_go::EvmRequiredErc20Approval) -> Result<()> {
        let token_address = Address::from_str(&approval.token_contract)?;
        let spender_address = Address::from_str(&approval.spender)?;
        let required_allowance = U256::from_dec_str(&approval.amount)?;

        let erc20_contract = IERC20Approve::new(token_address, self.wallet_manager.signer.clone());

        // Check current allowance
        let current_allowance = erc20_contract
            .allowance(self.wallet_manager.address, spender_address)
            .call()
            .await?;
        if current_allowance >= required_allowance {
            info!(
                token = ?token_address,
                spender = ?spender_address,
                current_allowance = ?current_allowance,
                "Sufficient allowance already granted, no approval needed"
            );
            return Ok(());
        }

        // Send approval transaction
        let approve_tx = erc20_contract.approve(spender_address, required_allowance);
        let pending_tx = approve_tx.send().await?;
        let receipt = pending_tx.await?;

        match receipt {
            Some(receipt) => {
                if receipt.status == Some(U64::from(1)) {
                    info!(
                        token = ?token_address,
                        spender = ?spender_address,
                        amount = ?required_allowance,
                        tx_hash = ?receipt.transaction_hash,
                        "ERC20 approval successful"
                    );
                    Ok(())
                } else {
                    Err(eyre::eyre!(
                        "ERC20 approval transaction failed: tx_hash = {:?}",
                        receipt.transaction_hash
                    ))
                }
            }
            None => Err(eyre::eyre!("ERC20 approval receipt not found")),
        }
    }
    
    async fn build_evm_transaction(&self, evm_tx_wrapper: skip_go::TxsEvmTx) -> Result<TransactionRequest> {
        let to_address = Address::from_str(&evm_tx_wrapper.evm_tx.to)?;
        let data = Bytes::from_str(&evm_tx_wrapper.evm_tx.data)?;
        let value = U256::from_dec_str(&evm_tx_wrapper.evm_tx.value)?;

        let tx = TransactionRequest::new()
            .to(to_address)
            .from(self.wallet_manager.address)
            .data(data)
            .value(value);

        // Estimate gas using the provider
        let gas_estimate = self.wallet_manager.signer.provider().estimate_gas(&tx.clone().into(), None).await?;
        let gas_price_decimal = u256_to_decimal(self.wallet_manager.signer.provider().get_gas_price().await?, 0)?;
        let gas_price_with_buf = gas_price_decimal * Decimal::from_f64(MAX_FEE_PER_GAS_BUFFER).unwrap();
        let gas_price_u256 = decimal_to_u256(gas_price_with_buf, 0)?;

        // Set tx gas and gas price
        let tx = tx.gas(gas_estimate).gas_price(gas_price_u256);

        info!(
            to = ?to_address,
            value = ?value,
            gas = ?gas_estimate,
            gas_price = ?gas_price_u256,
            "EVM transaction built successfully"
        );

        Ok(tx)
    }

    async fn simulate_evm_transaction(&self, tx: &TransactionRequest, native_balance: Decimal) -> Result<()> {
        // Get gas limits from transaction request
        let gas_dec = u256_to_decimal(tx.gas.unwrap_or(U256::zero()), 0)?;
        let gas_price_dec = u256_to_decimal(tx.gas_price.unwrap_or(U256::zero()), 18)?;
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

        // Simulate the transaction using eth_call
        match self.wallet_manager.signer.provider().call(&typed_tx, None).await {
            Ok(_) => {
                info!("EVM transaction simulation successful");
                Ok(())
            }
            Err(e) => Err(eyre::eyre!("EVM transaction simulation failed: {}", e)),
        }
    }

    async fn execute_evm_transaction(&self, tx: TransactionRequest) -> Result<(TxHash, TransactionReceipt)> {
        let pending_tx = self.wallet_manager.signer.send_transaction(tx, None).await?;
        let tx_hash = pending_tx.tx_hash();

        debug!(
            tx_hash = ?tx_hash,
            "EVM transaction sent, awaiting confirmation"
        );

        // Wait for confirmation
        match pending_tx.await? {
            Some(receipt) => {
                if receipt.status == Some(U64::from(1)) {
                    debug!(
                        tx_hash = ?tx_hash,
                        "EVM transaction confirmed successfully"
                    );
                    Ok((tx_hash, receipt))
                } else {
                    Err(eyre::eyre!(
                        "EVM transaction failed: tx_hash = {:?}, receipt = {:#?}",
                        tx_hash,
                        receipt
                    ))
                }
            }
            None => Err(eyre::eyre!("EVM transaction receipt not found for tx_hash = {:?}", tx_hash)),
        }
    }
}

// ==================== Utility methods ====================

/// Helper to convert Decimal to U256
fn decimal_to_u256(value: Decimal, decimals: u8) -> Result<U256> {
    let value_str = value.to_string();
    let formatted = ethers::utils::parse_units(&value_str, decimals as usize)
        .map_err(|e| eyre::eyre!("Failed to parse decimal value: {}", e))?;
    
    match formatted {
        ethers::utils::ParseUnits::U256(u256_val) => Ok(u256_val),
        _ => Err(eyre::eyre!("Unexpected parse result type")),
    }
}

/// Helper to convert U256 to Decimal
pub fn u256_to_decimal(value: U256, decimals: u8) -> Result<Decimal> {
    let formatted = ethers::utils::format_units(value, decimals as usize)
        .map_err(|e| eyre::eyre!("Failed to format U256 value: {}", e))?;
    Decimal::from_str(&formatted).map_err(|e| eyre::eyre!("Failed to parse formatted value: {}", e))
}
    
/// Derive Cosmos address from private key for a given chain
fn derive_cosmos_address_from_key(config: &config::Config, chain_prefix: &str) -> Result<String> {
    let private_key_str = &config.wallet_private_key.clone()[2..]; // Remove "0x" prefix
    let private_key_bytes = hex::decode(&private_key_str)?;
    let signing_key = secp256k1::SigningKey::from_slice(&private_key_bytes)?;
    let public_key = signing_key.public_key();
    let address = public_key.account_id(chain_prefix)?;
    Ok(address.to_string())
}