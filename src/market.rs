use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::time::Instant;
use futures::stream::{self, StreamExt};
use ethers::types::{Address, H160, I256, U256};
use ethers::utils::to_checksum;
use eyre::Result;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use tracing::{instrument, info, warn, error};
use serde_json::json;

use crate::config::Config;
use crate::token::{AssetToken, AssetTokenRegistry};
use crate::gmx::{
    reader_utils::{MarketPrices, MarketProps, MarketInfo, MarketPoolValueInfoProps},
    reader,
    datastore,
    event_listener_utils::MarketFees,
};
use crate::market_utils::{
    self,
    i256_to_decimal_scaled, 
    i256_to_decimal_scaled_decimals,
    u256_to_decimal_scaled,
    u256_to_decimal_scaled_decimals
};

#[derive(Debug, Clone)]
pub struct Market {
    // --- Market properties & data ---
    pub market_token: Address,
    pub index_token: AssetToken,
    pub long_token: AssetToken,
    pub short_token: AssetToken,
    pub borrowing_factor_per_second: Option<market_utils::BorrowingFactorPerSecond>,
    pub has_supply: bool, // Indicates if the market has supply (won't for swap or deprecated markets), default is true
    pub pnl: Option<market_utils::Pnl>,
    pub token_pool: Option<market_utils::TokenPool>,
    pub gm_token_price: Option<market_utils::GmTokenPrice>,
    pub open_interest: Option<market_utils::OpenInterest>,
    pub current_utilization: Option<Decimal>,
    pub volume: Option<market_utils::Volume>,
    pub cumulative_fees: Option<market_utils::CumulativeFees>,

    // Timestamp of the last market data update
    pub updated_at: Option<Instant>,  
}

impl fmt::Display for Market {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}/USD [{} - {}], Total Fees: {}",
            self.index_token.symbol,
            self.long_token.symbol,
            self.short_token.symbol,
            self.cumulative_fees.as_ref().map_or("N/A".to_string(), |u| format!("${:.5}", u.total_fees))
        )
    }
}

impl Market {
    // Construct a MarketProps struct from the latest price data
    pub fn market_props(&self) -> MarketProps {
        MarketProps {
            market_token: self.market_token,
            index_token: self.index_token.address,
            long_token: self.long_token.address,
            short_token: self.short_token.address,
        }
    }

    // Construct a MarketPrices struct from the latest price data
    pub fn market_prices(&self) -> Option<MarketPrices> {
        Some(MarketPrices {
            index_token_price: self.index_token.price_props()?,
            long_token_price: self.long_token.price_props()?,
            short_token_price: self.short_token.price_props()?,
        })
    }

    /// Returns a reference to the asset token (index, long, or short) matching the given address, if any.
    pub fn get_asset_token_by_address(&self, address: &Address) -> Option<&AssetToken> {
        if &self.index_token.address == address {
            Some(&self.index_token)
        } else if &self.long_token.address == address {
            Some(&self.long_token)
        } else if &self.short_token.address == address {
            Some(&self.short_token)
        } else {
            None
        }
    }

    // Fetch market info and pool info for the market
    pub async fn fetch_market_data(&mut self, config: &Config, market_fees: &MarketFees) -> Result<()> {
        let market_prices = self.market_prices();
        if let Some(prices) = market_prices {
            // Fetch market info
            let market_info = Some(reader::get_market_info(config, self.market_token, prices.clone()).await?);
            // Fetch pool info for deposit and withdrawal
            let (gm_price_min, pool_info_min) = reader::get_market_token_price(config, self.market_props(), prices.clone(), reader::PnlFactorType::Deposit, false).await?;
            let (gm_price_max, pool_info_max) = reader::get_market_token_price(config, self.market_props(), prices.clone(), reader::PnlFactorType::Deposit, true).await?;

            // Set borrowing factor per second
            self.borrowing_factor_per_second = Some(
                market_utils::BorrowingFactorPerSecond {
                    longs: u256_to_decimal_scaled(market_info.as_ref().map_or(U256::zero(), |m| m.borrowing_factor_per_second_for_longs)),
                    shorts: u256_to_decimal_scaled(market_info.as_ref().map_or(U256::zero(), |m| m.borrowing_factor_per_second_for_shorts)),
                }
            );

            // If pool_supply is zero, set has_supply to false
            self.has_supply =  pool_info_min.pool_value != I256::zero() || pool_info_max.pool_value != I256::zero();

            // Set GM token price
            self.gm_token_price = Some(
                market_utils::GmTokenPrice {
                    min: i256_to_decimal_scaled(gm_price_min),
                    max: i256_to_decimal_scaled(gm_price_max),
                    mid: i256_to_decimal_scaled((gm_price_min + gm_price_max) / I256::from(2)),
                }
            );

            // Set pnl
            self.pnl = Some(
                market_utils::Pnl {
                    long: i256_to_decimal_scaled(pool_info_min.long_pnl + pool_info_max.long_pnl) / Decimal::from(2),
                    short: i256_to_decimal_scaled(pool_info_min.short_pnl + pool_info_max.short_pnl) / Decimal::from(2),
                    net: i256_to_decimal_scaled(pool_info_min.net_pnl + pool_info_max.net_pnl) / Decimal::from(2),
                }
            );

            // Set token pool info
            self.token_pool = Some(
                market_utils::TokenPool {
                    long_token_amount: u256_to_decimal_scaled(pool_info_min.long_token_amount),
                    short_token_amount: u256_to_decimal_scaled(pool_info_min.short_token_amount),
                    long_token_usd: u256_to_decimal_scaled(pool_info_min.long_token_usd + pool_info_max.long_token_usd) / Decimal::from(2),
                    short_token_usd: u256_to_decimal_scaled(pool_info_min.short_token_usd + pool_info_max.short_token_usd) / Decimal::from(2),
                }
            );

            // Fetch long and short open interest
            let long_open_interest = datastore::get_open_interest(config, self.market_props(), true).await?;
            let short_open_interest = datastore::get_open_interest(config, self.market_props(), false).await?;

            // Fetch long and short open interest in tokens
            let long_open_interest_in_tokens = datastore::get_open_interest_in_tokens(config, self.market_props(), true).await?;
            let short_open_interest_in_tokens = datastore::get_open_interest_in_tokens(config, self.market_props(), false).await?;
            
            // Set open interest
            self.open_interest = Some(
                market_utils::OpenInterest {
                    long: u256_to_decimal_scaled(long_open_interest),
                    short: u256_to_decimal_scaled(short_open_interest),
                    long_via_tokens: u256_to_decimal_scaled(long_open_interest_in_tokens) * self.index_token.last_mid_price_usd.unwrap(),
                    short_via_tokens: u256_to_decimal_scaled(short_open_interest_in_tokens) * self.index_token.last_mid_price_usd.unwrap(),
                }
            );

            // Calculate utilization
            let total_open_interest = self.open_interest.as_ref().unwrap().long + self.open_interest.as_ref().unwrap().short;
            let total_pool_liquidity = self.token_pool.as_ref().unwrap().long_token_usd + self.token_pool.as_ref().unwrap().short_token_usd;
            if total_pool_liquidity > Decimal::ZERO {
                self.current_utilization = Some(total_open_interest / total_pool_liquidity);
            } 

            // Set volume and cumulative fees
            let mut total_swap_volume = Decimal::ZERO;
            for (token_address, volume) in &market_fees.swap_volume {
                if let Some(token) = self.get_asset_token_by_address(token_address) {
                    total_swap_volume += u256_to_decimal_scaled_decimals(*volume, token.decimals) * token.last_mid_price_usd.unwrap();
                } else {
                    warn!("Market {} has swap volume for unknown token {}", self.market_token, to_checksum(token_address, None));
                }
            }
            self.volume = Some(
                market_utils::Volume {
                    trading: u256_to_decimal_scaled(market_fees.trading_volume),
                    swap: total_swap_volume,
                }
            );
            
            let mut cumulative_fees = market_utils::CumulativeFees::new();
            let mut fee_types = [
                (&market_fees.position_fees, &mut cumulative_fees.position_fees),
                (&market_fees.liquidation_fees, &mut cumulative_fees.liquidation_fees),
                (&market_fees.swap_fees, &mut cumulative_fees.swap_fees),
                (&market_fees.borrowing_fees, &mut cumulative_fees.borrowing_fees),
            ];

            for (fee_map, field) in fee_types.iter_mut() {
                for (token_address, fee) in fee_map.iter() {
                    if let Some(token) = self.get_asset_token_by_address(token_address) {
                        tracing::info!("Market {} has fee for token {}: {}", to_checksum(&self.market_token, None), to_checksum(token_address, None), fee);
                        let fee_val = u256_to_decimal_scaled_decimals(*fee, token.decimals) * token.last_mid_price_usd.unwrap();
                        tracing::info!("Converting fee {} to decimal with decimals {}: {}, token price: {}", fee, token.decimals, fee_val, token.last_mid_price_usd.unwrap());
                        **field += fee_val;
                        cumulative_fees.total_fees += fee_val;
                    } else {
                        warn!("Market {} has fee for unknown token {}", self.market_token, to_checksum(token_address, None));
                    }
                }
            }
            self.cumulative_fees = Some(cumulative_fees);

            // Update the timestamp of the last update
            self.updated_at = Some(Instant::now());

            Ok(())
        } else {
            eyre::bail!("Fetch market prices before fetching market data for market {}", self);
        }
    }
}

pub struct MarketRegistry {
    markets: HashMap<Address, Market>,
    network_mode: String, 
}

impl MarketRegistry {
    #[instrument]
    pub fn new(config: &Config) -> Self {
        Self { 
            markets: HashMap::new(), 
            network_mode: config.network_mode.clone() 
        }
    }

    /// Insert a market into the registry if it is valid 
    fn insert_market_if_possible(
        &mut self,
        props: &MarketProps,
        asset_token_registry: &AssetTokenRegistry,
        only_if_absent: bool,   // If true, only insert if the market is not already present
    ) {
        if only_if_absent && self.markets.contains_key(&props.market_token) {
            return;
        }
        let index = asset_token_registry.get_asset_token(&props.index_token);
        let long = asset_token_registry.get_asset_token(&props.long_token);
        let short = asset_token_registry.get_asset_token(&props.short_token);
        if let (Some(index), Some(long), Some(short)) = (index, long, short) {
            let market = Market {
                market_token: props.market_token,
                index_token: index.clone(),
                long_token: long.clone(),
                short_token: short.clone(),
                borrowing_factor_per_second: None,
                has_supply: true, // Default to true
                pnl: None,
                token_pool: None,
                gm_token_price: None,
                open_interest: None,
                current_utilization: None,
                volume: None,
                cumulative_fees: None,
                updated_at: None,
            };
            self.markets.insert(props.market_token, market);
        } else {
            if props.index_token != H160::zero() {
                tracing::warn!(
                    "Missing tokens for market {:?}: index {:?}, long {:?}, short {:?}",
                    props.market_token, props.index_token, props.long_token, props.short_token
                );
            }
        }
    }

    /// Populate the registry with markets from GMX
    #[instrument(skip(self, config, asset_token_registry), fields(on_close = true))]
    pub async fn populate(
        &mut self,
        config: &Config,
        asset_token_registry: &AssetTokenRegistry,
    ) -> eyre::Result<()> {
        let market_props_list = reader::get_markets(config).await?;
        info!(count = market_props_list.len(), "Fetched market props from GMX");
        for props in &market_props_list {
            self.insert_market_if_possible(props, asset_token_registry, false);
        }
        Ok(())
    }

    /// Repopulate the registry by updating tracked tokens and adding any new markets
    #[instrument(skip(self, config, asset_token_registry), fields(on_close = true))]
    pub async fn repopulate(
        &mut self,
        config: &Config,
        asset_token_registry: &mut AssetTokenRegistry,
    ) -> eyre::Result<()> {
        asset_token_registry.update_tracked_tokens().await?;
        let market_props_list = reader::get_markets(config).await?;
        for props in &market_props_list {
            self.insert_market_if_possible(props, asset_token_registry, true);
        }
        Ok(())
    }

    pub fn get_market(&self, market_token: &Address) -> Option<&Market> {
        self.markets.get(market_token)
    }

    // Returns the number of markets in the registry
    pub fn num_markets(&self) -> usize {
        self.markets.len()
    }

    // Returns the number of relevant markets (those with supply)
    pub fn num_relevant_markets(&self) -> usize {
        self.relevant_markets().count()
    }

    // Returns an iterator over all markets in the registry
    pub fn all_markets(&self) -> impl Iterator<Item = &Market> {
        self.markets.values()
    }

    // Returns an iterator over all markets that have supply (i.e., are not swap or deprecated markets)
    pub fn relevant_markets(&self) -> impl Iterator<Item = &Market> {
        self.markets.values().filter(|m| m.has_supply)
    }

    // Prints all markets in the registry
    pub fn print_all_markets(&self) {
        let output = self.all_markets()
            .map(|m| format!("{}", m))
            .collect::<Vec<_>>()
            .join("\n");
        info!("All markets:\n{}", output);
    }

    // Prints all markets that have supply
    pub fn print_relevant_markets(&self) {
        let output =  self.relevant_markets()
            .map(|m| format!("{}", m))
            .collect::<Vec<_>>()
            .join("\n");
        info!("Relevant markets:\n{}", output);
    }

    #[instrument(skip(self, config), fields(on_close = true))]
    pub async fn update_all_market_data(&mut self, config: &Config, fee_map: &HashMap<Address, MarketFees>) -> Result<()> {

        stream::iter(self.markets.values_mut())
            .for_each_concurrent(1, |market| {  // Limit concurrency to 1 for now, can be adjusted later with a paid alchemy plan
                let config = config.clone();
                async move {
                    let fallback_fees = MarketFees::new();
                    let market_fees = fee_map.get(&market.market_token).unwrap_or(&fallback_fees);
                    if let Err(e) = market.fetch_market_data(&config, market_fees).await {
                        error!(market = %market.market_token, "Failed to fetch market data: {}", e);
                    } 
                }
            })
            .await;

        info!("All market data updated");
        Ok(())
    }

    pub fn save_markets_to_file(&self) -> eyre::Result<()> {
        let markets_json = self.all_markets().map(|market| {
            let description = format!(
                "{}/USD [{} - {}]",
                market.index_token.symbol,
                market.long_token.symbol,
                market.short_token.symbol
            );
            let market_address = to_checksum(&market.market_token, None);
            let token_json = |token: &AssetToken| json!({
                "symbol": token.symbol,
                "address": to_checksum(&token.address, None),
                "decimals": token.decimals
            });
            json!({
                "description": description,
                "marketAddress": market_address,
                "indexToken": token_json(&market.index_token),
                "longToken": token_json(&market.long_token),
                "shortToken": token_json(&market.short_token)
            })
        }).collect::<Vec<_>>();
        let output = json!({ "markets": markets_json });

        let path = match self.network_mode.as_str() {
            "prod" => "data/markets_data.json",
            "test" => "data/testnet_markets_data.json",
            _ => eyre::bail!("Unknown network mode: {}", self.network_mode),
        };

        fs::write(path, serde_json::to_string_pretty(&output)?)?;
        Ok(())
    }

    
}