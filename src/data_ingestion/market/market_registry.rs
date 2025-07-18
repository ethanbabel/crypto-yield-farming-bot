use std::collections::HashMap;
use std::fs;
use futures::future::join_all;
use futures::stream::{self, StreamExt};
use ethers::types::{Address, H160};
use ethers::utils::to_checksum;
use eyre::Result;
use tracing::{instrument, info, warn, error, debug};
use serde_json::json;
use std::sync::Arc;

use crate::config::Config;
use crate::data_ingestion::token::{token::AssetToken, token_registry::AssetTokenRegistry};
use crate::gmx::{
    reader_utils::MarketProps,
    reader,
    event_listener_utils::MarketFees,
    multicall::fetch_all_market_data_batch,
};
use super::market::Market;
use super::market_utils;

pub struct MarketRegistry {
    markets: HashMap<Address, Market>,
    network_mode: String, 
}

impl MarketRegistry {
    #[instrument(skip(config), fields(network_mode = %config.network_mode))]
    pub fn new(config: &Config) -> Self {
        Self { 
            markets: HashMap::new(), 
            network_mode: config.network_mode.clone() 
        }
    }

    /// Insert a market into the registry if it is valid 
    #[instrument(skip(self, asset_token_registry), fields(
        market_token = %props.market_token,
        index_token = %props.index_token,
        long_token = %props.long_token,
        short_token = %props.short_token
    ))]
    fn insert_market_if_possible(
        &mut self,
        props: &MarketProps,
        asset_token_registry: &AssetTokenRegistry,
        only_if_absent: bool,   // If true, only insert if the market is not already present
    ) {
        if only_if_absent && self.markets.contains_key(&props.market_token) {
            debug!("Market already exists, skipping");
            return;
        }
        let index = asset_token_registry.get_asset_token(&props.index_token);
        let long = asset_token_registry.get_asset_token(&props.long_token);
        let short = asset_token_registry.get_asset_token(&props.short_token);
        if let (Some(index), Some(long), Some(short)) = (index, long, short) {
            let market = Market {
                market_token: props.market_token,
                index_token: index,
                long_token: long,
                short_token: short,
                borrowing_factor_per_second: None,
                has_supply: true, // Default to true
                pnl: None,
                token_pool: None,
                gm_token_price: None,
                open_interest: None,
                current_utilization: None,
                volume:  market_utils::Volume::new(),
                cumulative_fees:  market_utils::CumulativeFees::new(),
                updated_at: None,
            };
            self.markets.insert(props.market_token, market);
            debug!("Market inserted successfully");
        } else {
            if props.index_token != H160::zero() {
                warn!(
                    market_token = %props.market_token,
                    index_token = %props.index_token,
                    long_token = %props.long_token,
                    short_token = %props.short_token,
                    "Missing tokens for market, skipping insertion"
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
        debug!("Fetching market props from GMX");
        let market_props_list = self.fetch_markets_with_retry(config).await?;
        info!(count = market_props_list.len(), "Fetched market props from GMX");
        
        let mut inserted_count = 0;
        for props in &market_props_list {
            let initial_count = self.markets.len();
            self.insert_market_if_possible(props, asset_token_registry, false);
            if self.markets.len() > initial_count {
                inserted_count += 1;
            }
        }
        
        info!(
            total_markets = self.markets.len(),
            inserted_count = inserted_count,
            "Market registry population completed"
        );
        Ok(())
    }

    /// Repopulate the registry by updating tracked tokens and adding any new markets
    #[instrument(skip(self, config, asset_token_registry), fields(on_close = true))]
    pub async fn repopulate(
        &mut self,
        config: &Config,
        asset_token_registry: &mut AssetTokenRegistry, 
    ) -> eyre::Result<(Vec<AssetToken>, Vec<Address>)> {
        debug!("Repopulating market registry");
        let new_tokens = asset_token_registry.update_tracked_tokens().await?;
        
        let market_props_list = self.fetch_markets_with_retry(config).await?;
        let mut new_market_addresses = Vec::new();
        
        for props in &market_props_list {
            let initial_count = self.markets.len();
            let market_token = props.market_token;
            
            self.insert_market_if_possible(props, asset_token_registry, true);
            if self.markets.len() > initial_count {
                new_market_addresses.push(market_token);
                debug!(market_token = %market_token, "Added new tracked market");
            }
        }
        
        info!(
            total_markets = self.markets.len(),
            new_markets = new_market_addresses.len(),
            new_tokens = new_tokens.len(),
            "Market registry repopulation completed"
        );
        Ok((new_tokens, new_market_addresses))
    }

    #[instrument(skip(self, market_token))]
    pub fn get_market(&self, market_token: &Address) -> Option<&Market> {
        self.markets.get(market_token)
    }

    // Returns the number of markets in the registry
    #[instrument(skip(self))]
    pub fn num_markets(&self) -> usize {
        self.markets.len()
    }

    // Returns the number of relevant markets (those with supply)
    #[instrument(skip(self))]
    pub fn num_relevant_markets(&self) -> usize {
        self.relevant_markets().count()
    }

    // Returns an iterator over all markets in the registry
    #[instrument(skip(self))]
    pub fn all_markets(&self) -> impl Iterator<Item = &Market> {
        self.markets.values()
    }

    // Returns an iterator over all markets that have supply (i.e., are not swap or deprecated markets)
    #[instrument(skip(self))]
    pub fn relevant_markets(&self) -> impl Iterator<Item = &Market> {
        self.markets.values().filter(|m| m.has_supply)
    }

    // Prints all markets in the registry
    #[instrument(skip(self))]
    pub fn print_all_markets(&self) {
        let market_count = self.all_markets().count();
        let output = self.all_markets()
            .map(|m| format!("{}", m))
            .collect::<Vec<_>>()
            .join("\n");
        info!(market_count = market_count, "All markets:\n{}", output);
    }

    // Prints all markets that have supply
    #[instrument(skip(self))]
    pub fn print_relevant_markets(&self) {
        let relevant_count = self.relevant_markets().count();
        let output =  self.relevant_markets()
            .map(|m| format!("{}", m))
            .collect::<Vec<_>>()
            .join("\n");
        info!(relevant_count = relevant_count, "Relevant markets:\n{}", output);
    }

    /// Zero out tracked fields for all markets (for each data collection cycle).
    pub fn zero_all_tracked_fields(&mut self) {
        for market in self.markets.values_mut() {
            market.zero_out_tracked_fields();
        }
    }

    /// Update all market data using batch multicall 
    #[instrument(skip(self, config, fee_map), fields(on_close = true))]
    pub async fn update_all_market_data(&mut self, config: Arc<Config>, fee_map: &HashMap<Address, MarketFees>) -> Result<()> {
        let market_count = self.markets.len();
        debug!(market_count = market_count, "Starting batch market data update");
        
        // Collect market props and prices for all markets that have prices available
        let mut markets_with_prices = Vec::new();
        for market in self.markets.values() {
            let market_props = market.market_props().await;
            if let Some(market_prices) = market.market_prices().await {
                markets_with_prices.push((market_props, market_prices));
            } else {
                warn!(
                    market = %market.market_token,
                    "Market prices not available, will fall back to individual fetch"
                );
            }
        }
        
        debug!(
            total_markets = market_count,
            batch_markets = markets_with_prices.len(),
            "Prepared markets for batch fetch"
        );
        
        // Fetch batch data if we have markets with prices
        let batch_data = if !markets_with_prices.is_empty() {
            match fetch_all_market_data_batch(&config, &markets_with_prices).await {
                Ok(data) => Some(data),
                Err(e) => {
                    error!("Failed to fetch batch market data: {}, falling back to individual fetches", e);
                    None
                }
            }
        } else {
            None
        };
        
        // Update all markets (with fallback to individual fetches)
        stream::iter(self.markets.values_mut())
            .for_each_concurrent(1, |market| {
                let config = Arc::clone(&config);
                let batch_data_ref = batch_data.as_ref();
                async move {
                    let fallback_fees = MarketFees::new();
                    let market_fees = fee_map.get(&market.market_token).unwrap_or(&fallback_fees);
                    
                    if let Err(e) = market.update_from_batch_data_with_fallback(&config, batch_data_ref, market_fees).await {
                        error!(
                            market = %market,
                            error = ?e,
                            "Failed to update market data"
                        );
                    } else {
                        debug!(
                            market = %market,
                            "Market data updated successfully"
                        );
                    }
                }
            })
            .await;
        
        info!(
            market_count = market_count,
            batch_count = batch_data.as_ref().map(|d| d.market_infos.len()).unwrap_or(0),
            "Batch market data update completed"
        );
        Ok(())
    }

    #[instrument(skip(self), fields(on_close = true))]
    pub async fn save_markets_to_file(&self) -> eyre::Result<()> {
        debug!("Saving markets to file");
        let markets_json = join_all(self.all_markets().map(|market| async {
            let index_token = market.index_token.read().await;
            let long_token = market.long_token.read().await;
            let short_token = market.short_token.read().await;
            let description = format!(
                "{}/USD [{} - {}]",
                index_token.symbol,
                long_token.symbol,
                short_token.symbol
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
                "indexToken": token_json(&index_token),
                "longToken": token_json(&long_token),
                "shortToken": token_json(&short_token)
            })
        })).await;
        let output = json!({ "markets": markets_json });

        let path = match self.network_mode.as_str() {
            "prod" => "data/markets_data.json",
            "test" => "data/testnet_markets_data.json",
            _ => eyre::bail!("Unknown network mode: {}", self.network_mode),
        };
        fs::write(path, serde_json::to_string_pretty(&output)?)?;
        info!(
            path = path,
            market_count = markets_json.len(),
            "Markets saved to file"
        );
        Ok(())
    }

    /// Helper method to fetch markets with retry logic and backoff
    async fn fetch_markets_with_retry(&self, config: &Config) -> eyre::Result<Vec<MarketProps>> {
        const MAX_RETRIES: u32 = 3;
        let mut last_error = None;

        for attempt in 1..=MAX_RETRIES {
            match self.try_fetch_markets(config).await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    last_error = Some(e);
                    if attempt < MAX_RETRIES {
                        let delay_ms = attempt * 500; // Linear backoff: 500ms, 1000ms
                        warn!(
                            attempt = attempt,
                            delay_ms = delay_ms,
                            error = %last_error.as_ref().unwrap(),
                            "Failed to fetch markets from GMX, retrying after delay"
                        );
                        tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms as u64)).await;
                    }
                }
            }
        }

        error!(
            attempts = MAX_RETRIES,
            error = %last_error.as_ref().unwrap(),
            "Failed to fetch markets from GMX after all retries"
        );
        Err(last_error.unwrap())
    }

    /// Internal method that performs the actual markets fetch
    async fn try_fetch_markets(&self, config: &Config) -> eyre::Result<Vec<MarketProps>> {
        reader::get_markets(config).await
    }
}