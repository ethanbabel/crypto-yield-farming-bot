use std::collections::HashMap;
use std::fmt;
use std::time::Instant;
use futures::stream::{self, StreamExt};
use ethers::types::{Address, H160, I256};
use eyre::Result;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use tracing::{instrument, info, warn, error};

use crate::config::Config;
use crate::constants::GMX_DECIMALS;
use crate::token::{AssetToken, AssetTokenRegistry};
use crate::gmx::gmx_reader_structs::{MarketPrices, MarketProps, MarketInfo, MarketPoolValueInfoProps};
use crate::gmx::gmx_reader;
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
            self.market_info = Some(gmx_reader::get_market_info(config, self.market_token, prices.clone()).await?);
            // Fetch pool info for deposit and withdrawal
            let deposit_min = gmx_reader::get_market_token_price(config, self.market_props(), prices.clone(), gmx_reader::PnlFactorType::Deposit, false).await?;
            let deposit_max = gmx_reader::get_market_token_price(config, self.market_props(), prices.clone(), gmx_reader::PnlFactorType::Deposit, true).await?;
            let withdrawal_min = gmx_reader::get_market_token_price(config, self.market_props(), prices.clone(), gmx_reader::PnlFactorType::Withdrawal, false).await?;
            let withdrawal_max = gmx_reader::get_market_token_price(config, self.market_props(), prices.clone(), gmx_reader::PnlFactorType::Withdrawal, true).await?;

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
            eyre::bail!("Fetch market prices before fetching market data for market {}", self);
        }
    }

    pub fn calculate_apr(&mut self) {
        if let (Some(market_info), Some(pool_info_deposit_min)) = (&self.market_info, &self.pool_info_deposit_min) {
            self.current_apr = return_calculation_utils::calculate_apr(market_info, pool_info_deposit_min);
        } else {
            tracing::warn!("Market data not fully available for APR calculation for market {}, continuing...", self);
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
    #[instrument]
    pub fn new() -> Self {
        Self { markets: HashMap::new() }
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
                market_info: None,
                pool_info_deposit_min: None,
                pool_info_deposit_max: None,
                pool_info_withdrawal_min: None,
                pool_info_withdrawal_max: None,
                has_supply: true,
                current_apr: None,
                gm_token_price_min: None,
                gm_token_price_max: None,
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
        let market_props_list = gmx_reader::get_markets(config).await?;
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
        let market_props_list = gmx_reader::get_markets(config).await?;
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
        for market in self.all_markets() {
            info!(market = %market, "Market info");
        }
    }

    // Prints all markets that have supply
    pub fn print_relevant_markets(&self) {
        for market in self.relevant_markets() {
            info!(market = %market, "Relevant market info");
        }
    }

    #[instrument(skip(self, config), fields(on_close = true))]
    pub async fn update_all_market_data(&mut self, config: &Config) -> Result<()> {

        stream::iter(self.markets.values_mut())
            .for_each_concurrent(1, |market| {  // Limit concurrency to 1 for now, can be adjusted later with a paid alchemy plan
                let config = config.clone();
                async move {
                    if let Err(e) = market.fetch_market_data(&config).await {
                        error!(market = %market.market_token, "Failed to fetch market data: {}", e);
                    } 
                }
            })
            .await;
        info!("All market data updated");
        Ok(())
    }

    #[instrument(skip(self), fields(on_close = false))]
    pub fn calculate_all_aprs(&mut self) {
        for market in self.markets.values_mut() {
            market.calculate_apr();
        }
        info!("All APRs calculated");
    }

    pub fn print_markets_by_apr_desc(&self) {
        let mut markets: Vec<&Market> = self.markets.values().collect();
        markets.sort_by(|a, b| {
            let a_apr = a.current_apr.unwrap_or(Decimal::ZERO);
            let b_apr = b.current_apr.unwrap_or(Decimal::ZERO);
            b_apr.partial_cmp(&a_apr).unwrap_or(std::cmp::Ordering::Equal)
        });
        let output = markets
            .iter()
            .filter(|m| m.has_supply)
            .map(|m| format!("{}", m))
            .collect::<Vec<_>>()
            .join("\n");
        info!("Markets by APR (desc):\n{}", output);
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