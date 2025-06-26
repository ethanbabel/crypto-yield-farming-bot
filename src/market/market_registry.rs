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
use crate::token::{token::AssetToken, token_registry::AssetTokenRegistry};
use crate::gmx::{
    reader_utils::{MarketProps},
    reader,
    event_listener_utils::MarketFees,
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
        let market_props_list = reader::get_markets(config).await?;
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
    ) -> eyre::Result<()> {
        debug!("Repopulating market registry");
        asset_token_registry.update_tracked_tokens().await?;
        
        let market_props_list = reader::get_markets(config).await?;
        let mut new_markets_count = 0;
        
        for props in &market_props_list {
            let initial_count = self.markets.len();
            self.insert_market_if_possible(props, asset_token_registry, true);
            if self.markets.len() > initial_count {
                new_markets_count += 1;
            }
        }
        
        debug!(
            total_markets = self.markets.len(),
            new_markets = new_markets_count,
            "Market registry repopulation completed"
        );
        Ok(())
    }

    #[instrument(skip(self), ret)]
    pub fn get_market(&self, market_token: &Address) -> Option<&Market> {
        self.markets.get(market_token)
    }

    // Returns the number of markets in the registry
    #[instrument(skip(self), ret)]
    pub fn num_markets(&self) -> usize {
        self.markets.len()
    }

    // Returns the number of relevant markets (those with supply)
    #[instrument(skip(self), ret)]
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

    #[instrument(skip(self, config, fee_map), fields(on_close = true))]
    pub async fn update_all_market_data(&mut self, config: Arc<Config>, fee_map: &HashMap<Address, MarketFees>) -> Result<()> {
        let market_count = self.markets.len();
        debug!(market_count = market_count, "Starting market data update");
        
        stream::iter(self.markets.values_mut())
            .for_each_concurrent(1, |market| {  // Limit concurrency to 1 for now, can be adjusted later with a paid alchemy plan
                let config = Arc::clone(&config);
                async move {
                    let fallback_fees = MarketFees::new();
                    let market_fees = fee_map.get(&market.market_token).unwrap_or(&fallback_fees);
                    if let Err(e) = market.fetch_market_data(&config, market_fees).await {
                        error!(
                            market = %market.market_token,
                            error = %e,
                            "Failed to fetch market data"
                        );
                    } else {
                        debug!(
                            market = %market.market_token,
                            "Market data updated successfully"
                        );
                    }
                }
            })
            .await;
        
        info!(market_count = market_count, "Market data update completed");
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
}