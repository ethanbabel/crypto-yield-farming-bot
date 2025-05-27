use std::collections::HashMap;
use std::fs;
use std::str::FromStr;

use futures::future::join_all;

use crate::oracle::Oracle;
use crate::constants::{GMX_API_PRICES_ENDPOINT, GMX_PRICE_DECIMALS};
use crate::config::Config;
use crate::gmx_structs::PriceProps;

use ethers::types::{Address, U256};
use eyre::Result;
use serde_json::Value;
use reqwest::Client;

#[derive(Debug, Clone)]
pub struct AssetToken {
    pub symbol: String,
    pub address: Address,
    pub decimals: u8,
    pub is_synthetic: bool,
    pub oracle: Option<Oracle>,
    pub last_min_price: Option<U256>,
    pub last_max_price: Option<U256>,
    pub last_min_price_usd: Option<f64>,
    pub last_max_price_usd: Option<f64>,
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
}

impl AssetTokenRegistry {
    pub fn new() -> Self {
        Self {
            asset_tokens: HashMap::new(),
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

    pub fn load_from_file(&mut self, path: &str) -> Result<()> {
        let file_content = fs::read_to_string(path)?;
        let json_data: Value = serde_json::from_str(&file_content)?;

        let tokens = json_data.get("tokens").ok_or_else(|| eyre::eyre!("Missing 'tokens' field"))?;

        for token in tokens.as_array().unwrap_or(&vec![]) {
            let symbol = token["symbol"].as_str().expect("Token must have a symbol").to_string();
            let address = Address::from_str(token["address"].as_str().unwrap_or(""))?;
            let decimals = token["decimals"].as_u64().unwrap_or(18) as u8;
            let is_synthetic = token.get("synthetic").map_or(false, |v| v.as_bool().unwrap_or(false));

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
                decimals,
                is_synthetic,
                oracle,
                last_min_price: None,
                last_max_price: None,
                last_min_price_usd: None,
                last_max_price_usd: None,
            };

            self.asset_tokens.insert(address, asset_token);
        }

        Ok(())
    }

    pub async fn update_all_gmx_prices(&mut self) -> Result<()> {
        let client = Client::new();
        let res = client.get(GMX_API_PRICES_ENDPOINT).send().await?;
        let prices: Vec<Value> = res.json().await?;

        for entry in prices.iter() {
            if let Some(address_str) = entry["tokenAddress"].as_str() {
                if let Ok(address) = Address::from_str(address_str) {
                    if let Some(token) = self.asset_tokens.get_mut(&address) {
                        let min_raw = entry["minPrice"].as_str().unwrap_or("0");
                        let max_raw = entry["maxPrice"].as_str().unwrap_or("0");

                        // Get raw prices as U256
                        let min_price: U256 = min_raw.parse::<U256>().unwrap_or(U256::zero());
                        let max_price: U256 = max_raw.parse::<U256>().unwrap_or(U256::zero());

                        // Store raw prices
                        token.last_min_price = Some(min_price);
                        token.last_max_price = Some(max_price);
                        
                        // Convert raw prices to f64 and adjust for GMX price decimals
                        let min_price_usd: f64 = min_raw.parse::<f64>().unwrap_or(0.0) / 10f64.powi(GMX_PRICE_DECIMALS as i32);
                        let max_price_usd: f64 = max_raw.parse::<f64>().unwrap_or(0.0) / 10f64.powi(GMX_PRICE_DECIMALS as i32);

                        // Adjust using token decimals
                        let adjustment = 10f64.powi(token.decimals as i32);
                        token.last_min_price_usd = Some(min_price_usd * adjustment);
                        token.last_max_price_usd = Some(max_price_usd * adjustment);
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
}
