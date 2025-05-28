use std::collections::HashMap;
use std::fs;
use std::str::FromStr;
use std::time::Instant;

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
    pub address: Address,   // If mode=prod this is mainnet address, if mode=test this is testnet address
    pub mainnet_address: Option<Address>, // If mode=prod this is the mainnet address, if mode=test this is None
    pub decimals: u8,
    pub is_synthetic: bool,
    pub oracle: Option<Oracle>,
    pub last_min_price: Option<U256>,
    pub last_max_price: Option<U256>,
    pub last_min_price_usd: Option<f64>,
    pub last_max_price_usd: Option<f64>,
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
    mode: String, // "prod" or "test"
}

impl AssetTokenRegistry {
    pub fn new(config: &Config) -> Self {
        Self {
            asset_tokens: HashMap::new(),
            mode: config.mode.clone(),
        }
    }

    pub fn get_asset_token(&self, address: &Address) -> Option<&AssetToken> {
        self.asset_tokens.get(address)
    }

    // Internal helper function for mode=test to get token by mainnet address
    fn get_asset_token_by_mainnet_address(&mut self, mainnet_address: &Address) -> Option<&mut AssetToken> {
        self.asset_tokens.values_mut().find(|token| token.mainnet_address.as_ref() == Some(mainnet_address))
    }

    pub fn num_asset_tokens(&self) -> usize {
        self.asset_tokens.len()
    }

    pub fn asset_tokens(&self) -> impl Iterator<Item = &AssetToken> {
        self.asset_tokens.values()
    }

    pub fn load_from_file(&mut self) -> Result<()> {
        // Set token data file path based on mode
        let path = match self.mode.as_str() {
            "test" => "tokens/testnet_asset_token_data.json".to_string(),
            "prod" => "tokens/asset_token_data.json".to_string(),
            _ => panic!("Invalid MODE"),
        };
        let file_content = fs::read_to_string(path)?;
        let json_data: Value = serde_json::from_str(&file_content)?;

        let tokens = json_data.get("tokens").ok_or_else(|| eyre::eyre!("Missing 'tokens' field"))?;

        for token in tokens.as_array().unwrap_or(&vec![]) {
            let symbol = token["symbol"].as_str().expect("Token must have a symbol").to_string();
            let decimals = token["decimals"].as_u64().expect("Token must have decimals") as u8;
            let is_synthetic = token.get("synthetic").map_or(false, |v| v.as_bool().unwrap_or(false));
            
            // Handle both mainnet and testnet addresses
            let address = if self.mode == "prod" {
                Address::from_str(token["address"].as_str().expect("Token must have an address"))?
            } else {
                Address::from_str(token["testnetAddress"].as_str().expect("Token must have a testnet address"))?
            };
            let mainnet_address = if self.mode == "test" {
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
                updated_at: None,
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
                    let asset_token = if self.mode == "prod" { 
                        self.asset_tokens.get_mut(&address)
                    } else {
                        // In test mode, we need to find the token by its mainnet address
                        self.get_asset_token_by_mainnet_address(&address)
                    };
                    if let Some(token) = asset_token {
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

                        // Update the timestamp of the last price update
                        token.updated_at = Some(Instant::now());
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
