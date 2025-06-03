use std::collections::HashMap;
use std::fmt;
use std::time::Instant;
use futures::stream::{self, StreamExt};
use ethers::types::{Address, H160, I256};
use eyre::{eyre, Result};
use rust_decimal::Decimal;
use rust_decimal::prelude::*;

use crate::config::Config;
use crate::constants::GMX_DECIMALS;
use crate::token::{AssetToken, AssetTokenRegistry};
use crate::gmx_structs::{MarketPrices, MarketProps, MarketInfo, MarketPoolValueInfoProps};
use crate::gmx;
use crate::return_calculation_utils;

#[derive(Debug, Clone)]
pub struct Market {
    pub market_token: Address,
    pub index_token: AssetToken,
    pub long_token: AssetToken,
    pub short_token: AssetToken,
    market_info: Option<MarketInfo>,
    pool_info_deposit_min: Option<MarketPoolValueInfoProps>,
    pool_info_deposit_max: Option<MarketPoolValueInfoProps>,
    pool_info_withdrawal_min: Option<MarketPoolValueInfoProps>,
    pool_info_withdrawal_max: Option<MarketPoolValueInfoProps>,
    pub has_supply : bool, // Indicates if the market has supply (won't for swap or deprecated markets), default is true
    pub gm_token_price_min: Option<I256>, 
    pub gm_token_price_max: Option<I256>,
    pub current_apr: Option<Decimal>, 
    
    // Timestamp of the last market data (market_info + pool_info) update, set time of least recent update between market_info and all pool_info's
    pub updated_at: Option<Instant>,  
}

impl fmt::Display for Market {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}/USD [{} - {}],  APR: {},  GM Token Price Range: [${}, ${}]",
            self.index_token.symbol,
            self.long_token.symbol,
            self.short_token.symbol,
            self.current_apr.map_or("N/A".to_string(), |v| format!("{:.2}%", v * dec!(100.0))),
            self.gm_token_price_min.map_or("N/A".to_string(), |v| format!("{:.5}", i256_to_decimal_scaled(v))),
            self.gm_token_price_max.map_or("N/A".to_string(), |v| format!("{:.5}", i256_to_decimal_scaled(v))),
        )
    }
}

impl Market {
    /// Construct a MarketProps struct from the latest price data
    pub fn market_props(&self) -> MarketProps {
        MarketProps {
            market_token: self.market_token,
            index_token: self.index_token.address,
            long_token: self.long_token.address,
            short_token: self.short_token.address,
        }
    }

    /// Construct a MarketPrices struct from the latest price data
    pub fn market_prices(&self) -> Option<MarketPrices> {
        Some(MarketPrices {
            index_token_price: self.index_token.price_props()?,
            long_token_price: self.long_token.price_props()?,
            short_token_price: self.short_token.price_props()?,
        })
    }

    /// Fetch market info and pool info for the market
    pub async fn fetch_market_data(&mut self, config: &Config) -> Result<()> {
        let market_prices = self.market_prices();
        if let Some(prices) = market_prices {
            // Fetch market info
            self.market_info = Some(gmx::get_market_info(config, self.market_token, prices.clone()).await?);
            // Fetch pool info for deposit and withdrawal
            let deposit_min = gmx::get_market_token_price(config, self.market_props(), prices.clone(), gmx::PnlFactorType::Deposit, false).await?;
            let deposit_max = gmx::get_market_token_price(config, self.market_props(), prices.clone(), gmx::PnlFactorType::Deposit, true).await?;
            let withdrawal_min = gmx::get_market_token_price(config, self.market_props(), prices.clone(), gmx::PnlFactorType::Withdrawal, false).await?;
            let withdrawal_max = gmx::get_market_token_price(config, self.market_props(), prices.clone(), gmx::PnlFactorType::Withdrawal, true).await?;

            // Update pool info and gm token prices
            self.pool_info_deposit_min = Some(deposit_min.1);
            self.pool_info_deposit_max = Some(deposit_max.1);
            self.pool_info_withdrawal_min = Some(withdrawal_min.1);
            self.pool_info_withdrawal_max = Some(withdrawal_max.1);
            
            self.gm_token_price_min = Some(deposit_min.0);
            self.gm_token_price_max = Some(deposit_max.0);

            // If pool_supply is zero, set has_supply to false
            self.has_supply = self.pool_info_deposit_min.as_ref().map_or(true, |pool_info| !pool_info.pool_value.is_zero());

            // Update the timestamp of the last update
            self.updated_at = Some(Instant::now());

            Ok(())
        } else {
            Err(eyre!("Fetch market prices before fetching market data for market {}", self))
        }
    }

    pub fn calculate_apr(&mut self) {
        if let (Some(market_info), Some(pool_info_deposit_min)) = (&self.market_info, &self.pool_info_deposit_min) {
            self.current_apr = return_calculation_utils::calculate_apr(market_info, pool_info_deposit_min);
        } else {
            tracing::warn!("Market data not fully available for APR calculation for market {}", self.market_token);
        }
    }
}

fn i256_to_decimal_scaled(val: I256) -> Decimal {
    let formatted = ethers::utils::format_units(val, GMX_DECIMALS as usize).unwrap_or_else(|_| "0".to_string());
    Decimal::from_str(&formatted).unwrap_or(Decimal::ZERO)
}

pub struct MarketRegistry {
    markets: HashMap<Address, Market>,
}

impl MarketRegistry {
    pub fn new() -> Self {
        Self {
            markets: HashMap::new(),
        }
    }

    /// Populate the registry using MarketProps and a reference to the TokenRegistry
    pub fn populate(
        &mut self,
        market_props_list: &Vec<MarketProps>,
        asset_token_registry: &AssetTokenRegistry,
    ) {
        for props in market_props_list {
            let index = asset_token_registry.get_asset_token(&props.index_token);
            let long = asset_token_registry.get_asset_token(&props.long_token);
            let short = asset_token_registry.get_asset_token(&props.short_token);

            if let (Some(index), Some(long), Some(short)) = (index, long, short) {
                let market = Market {
                    market_token: props.market_token,
                    index_token: index.clone(),
                    long_token: long.clone(),
                    short_token: short.clone(),
                    market_info: None,
                    pool_info_deposit_min: None,
                    pool_info_deposit_max: None,
                    pool_info_withdrawal_min: None,
                    pool_info_withdrawal_max: None,
                    has_supply: true, // Default to true, can be updated later if needed
                    current_apr: None,
                    gm_token_price_min: None,
                    gm_token_price_max: None,
                    updated_at: None,
                };
                self.markets.insert(props.market_token, market);
            } else {
                // Index token address is zero for swap markets, safe to ignore for this bot
                if props.index_token != H160::zero() {
                    tracing::warn!(
                        "Missing tokens for market {:?}: index {:?}, long {:?}, short {:?}",
                        props.market_token, props.index_token, props.long_token, props.short_token
                    );
                }
            }
        }
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
        for market in self.all_markets() {
            println!("{}", market);
        }
    }

    // Prints all markets that have supply
    pub fn print_relevant_markets(&self) {
        for market in self.relevant_markets() {
            println!("{}", market);
        }
    }

    pub async fn update_all_market_data(&mut self, config: &Config) -> Result<()> {
        let config = config.clone();

        stream::iter(self.markets.values_mut())
            .for_each_concurrent(1, |market| {  // Limit concurrency to 1 for now, can be adjusted later with a paid alchemy plan
                let config = config.clone();
                async move {
                    if let Err(err) = market.fetch_market_data(&config).await {
                        tracing::warn!("Failed to fetch market data: {:?}", err);
                    }
                }
            })
            .await;

        Ok(())
    }

    pub fn calculate_all_aprs(&mut self) {
        for market in self.markets.values_mut() {
            market.calculate_apr();
        }
    }

    pub fn print_markets_by_apr_desc(&self) {
        let mut markets: Vec<&Market> = self.markets.values().collect();
        markets.sort_by(|a, b| {
            let a_apr = a.current_apr.unwrap_or(Decimal::ZERO);
            let b_apr = b.current_apr.unwrap_or(Decimal::ZERO);
            b_apr.partial_cmp(&a_apr).unwrap_or(std::cmp::Ordering::Equal)
        });
        for market in markets {
           if market.has_supply {
                println!("{}", market);
            } 
        }
    }

    pub fn top_markets_by_apr(&self, count: usize) -> Vec<&Market> {
        let mut markets: Vec<&Market> = self.markets.values().collect();
        markets.sort_by(|a, b| {
            let a_apr = a.current_apr.unwrap_or(Decimal::ZERO);
            let b_apr = b.current_apr.unwrap_or(Decimal::ZERO);
            b_apr.partial_cmp(&a_apr).unwrap_or(std::cmp::Ordering::Equal)
        });
        markets.into_iter().take(count).collect()
    }
}