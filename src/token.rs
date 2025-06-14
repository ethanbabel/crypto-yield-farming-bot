use std::collections::HashMap;
use std::fs;
use std::str::FromStr;
use std::time::Instant;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use futures::future::join_all;
use ethers::types::{Address, U256};
use ethers::utils;
use eyre::{Result, eyre};
use serde_json::Value;
use reqwest::Client;
use tracing::{instrument, info, warn};

use crate::oracle::Oracle;
use crate::constants::{GMX_API_PRICES_ENDPOINT, GMX_SUPPORTED_TOKENS_ENDPOINT, GMX_DECIMALS};
use crate::config::Config;
use crate::gmx::reader_utils::PriceProps;

#[derive(Debug, Clone)]
pub struct AssetToken {
    pub symbol: String,
    pub address: Address,   // If netowrk mode=test this is testnet address, if network mode=prod this is mainnet address
    pub mainnet_address: Option<Address>, // If network mode=test this is the mainnet address, if network mode=prod this is None
    pub decimals: u8,
    pub is_synthetic: bool,
    pub oracle: Option<Oracle>,
    pub last_min_price: Option<U256>,
    pub last_max_price: Option<U256>,
    pub last_min_price_usd: Option<Decimal>,
    pub last_max_price_usd: Option<Decimal>,
    pub last_mid_price_usd: Option<Decimal>, 
    pub updated_at: Option<Instant>, // Timestamp of last price update
}

impl AssetToken {
    pub fn price_props(&self) -> Option<PriceProps> {
        Some(PriceProps {
            min: self.last_min_price?,
            max: self.last_max_price?,
        })
    }
}

#[derive(Debug)]
pub struct AssetTokenRegistry {
    asset_tokens: HashMap<Address, AssetToken>,
    network_mode: String, // "prod" or "test"
}

impl AssetTokenRegistry {
    pub fn new(config: &Config) -> Self {
        Self {
            asset_tokens: HashMap::new(),
            network_mode: config.network_mode.clone(),
        }
    }

    pub fn get_asset_token(&self, address: &Address) -> Option<&AssetToken> {
        self.asset_tokens.get(address)
    }

    pub fn num_asset_tokens(&self) -> usize {
        self.asset_tokens.len()
    }

    pub fn asset_tokens(&self) -> impl Iterator<Item = &AssetToken> {
        self.asset_tokens.values()
    }

    #[instrument(skip(self), fields(on_close = true))]
    pub fn load_from_file(&mut self) -> Result<()> {
        let path = match self.network_mode.as_str() {
            "test" => "data/testnet_asset_token_data.json".to_string(),
            "prod" => "data/asset_token_data.json".to_string(),
            _ => panic!("Invalid NETWORK_MODE"),
        };
        info!(file = %path, "Loading asset tokens from file");
        let file_content = fs::read_to_string(&path)?;
        let json_data: Value = serde_json::from_str(&file_content)?;
        let tokens = json_data.get("tokens").ok_or_else(|| eyre!("Tokens not found in JSON data"))?;

        for token in tokens.as_array().unwrap_or(&vec![]) {
            let symbol = token["symbol"].as_str().expect("Token must have a symbol").to_string();
            let decimals = token["decimals"].as_u64().expect("Token must have decimals") as u8;
            let is_synthetic = token.get("synthetic").map_or(false, |v| v.as_bool().unwrap_or(false));
            
            // Handle both mainnet and testnet addresses
            let address = if self.network_mode == "prod" {
                Address::from_str(token["address"].as_str().expect("Token must have an address"))?
            } else {
                Address::from_str(token["testnetAddress"].as_str().expect("Token must have a testnet address"))?
            };
            let mainnet_address = if self.network_mode == "test" {
                Some(Address::from_str(token["mainnetAddress"].as_str().expect("Token must have a mainnet address"))?)
            } else {
                None
            };

            let oracle = if let Some(addresses) = token.get("oracleAddresses") {
                let feed_addresses: Vec<Address> = addresses
                    .as_array()
                    .unwrap_or(&vec![])
                    .iter()
                    .filter_map(|v| Address::from_str(v.as_str().unwrap_or("")).ok())
                    .collect();
                Some(Oracle::new_composite(feed_addresses))
            } else if let Some(single) = token.get("oracleAddress") {
                Some(Oracle::new_single(Address::from_str(single.as_str().unwrap_or(""))?))
            } else {
                None
            };

            let asset_token = AssetToken {
                symbol,
                address,
                mainnet_address,
                decimals,
                is_synthetic,
                oracle,
                last_min_price: None,
                last_max_price: None,
                last_min_price_usd: None,
                last_max_price_usd: None,
                last_mid_price_usd: None,
                updated_at: None,
            };
            self.asset_tokens.insert(address, asset_token);
        }
        info!("Loaded asset tokens from file");
        Ok(())
    }

    #[instrument(skip(self), fields(on_close = true))]
    pub async fn update_tracked_tokens(&mut self) -> Result<()> {
        // If the network mode is test, we don't update tracked tokens
        if self.network_mode == "test" {
            return Ok(());
        }

        let client = Client::new();
        let res = client
            .get(GMX_SUPPORTED_TOKENS_ENDPOINT)
            .send()
            .await?
            .error_for_status()?;
        let res_json: Value = res.json().await?;
        let tokens_arr = res_json["tokens"].as_array().ok_or_else(|| 
            eyre!("Error parsing the 'tokens' field from API response")
        )?;

        let mut new_tokens = Vec::new();
        for token in tokens_arr {
            let address = Address::from_str(token["address"].as_str().expect("Token must have an address"))?;

            // If the token isn't already in the registry, add it and add it to new tokens
            if !self.asset_tokens.contains_key(&address) {
                let symbol = token["symbol"].as_str().expect("Token must have a symbol").to_string();
                let decimals = token["decimals"].as_u64().expect("Token must have decimals") as u8;
                let is_synthetic = token.get("synthetic").map_or(false, |v| v.as_bool().unwrap_or(false));

                let new_token = AssetToken {
                    symbol,
                    address,
                    mainnet_address: None, // Mainnet address is none in test mode
                    decimals,
                    is_synthetic,
                    oracle: None, 
                    last_min_price: None,
                    last_max_price: None,
                    last_min_price_usd: None,
                    last_max_price_usd: None,
                    last_mid_price_usd: None,
                    updated_at: None,
                };
                self.asset_tokens.insert(address, new_token.clone());
                new_tokens.push(new_token);
            }
        }

        // If no new tokens were found, return early
        if new_tokens.is_empty() {
            tracing::info!("No new supported tokens found.");
            return Ok(());
        }

        // Write the new tokens to json file
        let path = "data/asset_token_data.json".to_string();
        let existing_file_content = fs::read_to_string(&path)?;
        let mut existing_json_data: Value = serde_json::from_str(&existing_file_content)?;
        let tokens_arr: &mut Vec<Value> = existing_json_data["tokens"].as_array_mut().ok_or_else(|| 
            eyre!("Error parsing the 'tokens' field from existing JSON data")
        )?;
        for token in &new_tokens {
            let new_token_json = serde_json::json!({
                "symbol": token.symbol,
                "address": utils::to_checksum(&token.address, None),
                "decimals": token.decimals,
                "synthetic": token.is_synthetic,
            });
            tokens_arr.push(new_token_json);
        }
        fs::write(&path, serde_json::to_string_pretty(&existing_json_data)?)?;
        tracing::info!("Added {} new tokens to the registry and updated the token data file. New tokens: {}", new_tokens.len(), 
            new_tokens.iter().map(|t| t.symbol.clone()).collect::<Vec<String>>().join(", "));
        Ok(())
    }                  

    #[instrument(skip(self), fields(on_close = true))]
    pub async fn update_all_gmx_prices(&mut self) -> Result<()> {
        let client = Client::new();
        let res = client
            .get(GMX_API_PRICES_ENDPOINT)
            .send()
            .await?
            .error_for_status()?;
        let prices: Vec<Value> = res.json().await?;

        for entry in prices.iter() {
            if let Some(address_str) = entry["tokenAddress"].as_str() {
                if let Ok(address) = Address::from_str(address_str) {
                    if self.network_mode == "prod" {
                        if let Some(token) = self.asset_tokens.get_mut(&address) {
                            let min_raw = entry["minPrice"].as_str().unwrap_or("0");
                            let max_raw = entry["maxPrice"].as_str().unwrap_or("0");

                            let min_price: U256 = U256::from_dec_str(min_raw).unwrap_or(U256::zero());
                            let max_price: U256 = U256::from_dec_str(max_raw).unwrap_or(U256::zero());

                            token.last_min_price = Some(min_price);
                            token.last_max_price = Some(max_price);
                            let min_price_usd: Decimal = Decimal::from_str(
                                &utils::format_units(min_price, (GMX_DECIMALS - token.decimals) as usize).unwrap_or_else(|_| "0".to_string())
                            ).unwrap_or(Decimal::ZERO);
                            let max_price_usd: Decimal = Decimal::from_str(
                                &utils::format_units(max_price, (GMX_DECIMALS - token.decimals) as usize).unwrap_or_else(|_| "0".to_string())
                            ).unwrap_or(Decimal::ZERO);
                            let adjustment = Decimal::from(10u64).powu(token.decimals as u64);
                            token.last_min_price_usd = Some(min_price_usd * adjustment);
                            token.last_max_price_usd = Some(max_price_usd * adjustment);
                            token.last_mid_price_usd = Some((min_price_usd + max_price_usd) / Decimal::from(2));
                            token.updated_at = Some(Instant::now());
                        }
                    } else {
                        // In test mode, update all tokens whose mainnet_address matches
                        for token in self.asset_tokens.values_mut().filter(|token| token.mainnet_address == Some(address)) {
                            let min_raw = entry["minPrice"].as_str().unwrap_or("0");
                            let max_raw = entry["maxPrice"].as_str().unwrap_or("0");

                            let min_price: U256 = U256::from_dec_str(min_raw).unwrap_or(U256::zero());
                            let max_price: U256 = U256::from_dec_str(max_raw).unwrap_or(U256::zero());

                            token.last_min_price = Some(min_price);
                            token.last_max_price = Some(max_price);
                            let min_price_usd: Decimal = Decimal::from_str(
                                &utils::format_units(min_price, GMX_DECIMALS as usize).unwrap_or_else(|_| "0".to_string())
                            ).unwrap_or(Decimal::ZERO);
                            let max_price_usd: Decimal = Decimal::from_str(
                                &utils::format_units(max_price, GMX_DECIMALS as usize).unwrap_or_else(|_| "0".to_string())
                            ).unwrap_or(Decimal::ZERO);
                            let adjustment = Decimal::from(10u64).powu(token.decimals as u64);
                            token.last_min_price_usd = Some(min_price_usd * adjustment);
                            token.last_max_price_usd = Some(max_price_usd * adjustment);
                            token.last_mid_price_usd = Some((min_price_usd + max_price_usd) / Decimal::from(2));
                            token.updated_at = Some(Instant::now());
                        }
                    }
                }
            }
        }
        Ok(())
    }

    pub async fn update_all_oracle_prices(&mut self, config: &Config) -> Result<()> {
        let mut tasks = Vec::new();

        for token in self.asset_tokens.values_mut() {
            if let Some(oracle) = &mut token.oracle {
                tasks.push(oracle.fetch_price(config));
            }
        }

        // Run all updates in parallel
        let results = join_all(tasks).await;

        for result in results {
            if let Err(e) = result {
                tracing::warn!("Failed to update oracle price: {}", e);
            }
        }

        Ok(())
    }

    /// Returns true if all asset tokens have both min and max prices set
    pub fn all_prices_fetched(&self) -> bool {
        self.asset_tokens.values().all(|token| token.last_min_price.is_some() && token.last_max_price.is_some())
    }
}
