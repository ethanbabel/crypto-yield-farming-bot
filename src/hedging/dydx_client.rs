use eyre::Result;
use std::sync::Arc;
use std::collections::HashMap;
use tracing::{instrument, debug, info};
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use ethers::prelude::*;
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

const ARBITRUM_CHAIN_ID: &str = "42161";
const DYDX_CHAIN_ID: &str = "dydx-mainnet-1";
const ARBITRUM_USDC_DENOM: &str = "0xaf88d065e77c8cC2239327C5EDb3A432268e5831";
const DYDX_USDC_DENOM: &str = "ibc/8E27BA2D5493AF5636760E354E46004562C46AB7EC0CC4C1CA14E9E20E2545B5";
const USDC_DECIMALS: u8 = 6;

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
        // Get USDC balances
        let initial_arbitrum_usdc_balance = self.get_arbitrum_usdc_balance().await?;
        let initial_dydx_usdc_balance = self.get_dydx_usdc_balance().await?;

        // Sanity check request
        let log_string = self.get_deposit_log_string(
            initial_arbitrum_usdc_balance,
            amount_in,
            amount_out,
        ).await?;

        info!(
            initial_arbitrum_usdc_balance = ?initial_arbitrum_usdc_balance,
            initial_dydx_usdc_balance = ?initial_dydx_usdc_balance,
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
        if amount > initial_arbitrum_usdc_balance {
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

    // async fn execute_skip_go_transfer(&self, txs: Vec<skip_go::SkipGoTx>) -> Result<()> {
    //     for tx in txs {
    //         match tx {
    //             skip_go::SkipGoTx::CosmosTx(cosmos_tx) => {
    //                 debug!("Executing Cosmos Tx: {:#?}", cosmos_tx);
    //             }
    //             skip_go::SkipGoTx::EvmTx(evm_tx) => {
    //                 debug!("Executing EVM Tx: {:#?}", evm_tx);
    //             }
    //             skip_go::SkipGoTx::SvmTx(svm_tx) => {
    //                 return Err(eyre::eyre!("No support for SVM transactions: {:#?}", svm_tx));
    //             }
    //         }
    //     }
    // }
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