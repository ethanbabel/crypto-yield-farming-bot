use sqlx::PgPool;
use std::collections::HashMap;
use ethers::types::Address;
use std::sync::Arc;
use std::str::FromStr;
use tokio::sync::RwLock;
use tracing::{info, debug, instrument};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use super::connection;
use super::schema;
use super::queries::{
    tokens as tokens_queries,
    markets as markets_queries,
    token_prices as token_prices_queries,
    market_states as market_states_queries,
    strategy_runs as strategy_runs_queries,
    strategy_targets as strategy_targets_queries,
    trades as trades_queries,
    portfolio_snapshots as portfolio_snapshots_queries,
    dydx_perps as dydx_perps_queries,
    dydx_perp_states as dydx_perp_states_queries,
    position_snapshots as position_snapshots_queries,
};
use super::models::{
    tokens::{TokenModel, NewTokenModel, RawTokenModel},
    markets::{MarketModel, NewMarketModel, RawMarketModel},
    token_prices::{TokenPriceModel, NewTokenPriceModel, RawTokenPriceModel},
    market_states::{MarketStateModel, NewMarketStateModel, RawMarketStateModel},
    strategy_runs::{NewStrategyRunModel, StrategyRunModel},
    strategy_targets::NewStrategyTargetModel,
    trades::NewTradeModel,
    portfolio_snapshots::{NewPortfolioSnapshotModel, PortfolioSnapshotModel},
    dydx_perps::{NewDydxPerpModel, RawDydxPerpModel},
    dydx_perp_states::{NewDydxPerpStateModel, RawDydxPerpStateModel},
    position_snapshots::NewPositionSnapshotModel,
};
use crate::config::Config;
use crate::data_ingestion::token::token::AssetToken;
use crate::data_ingestion::market::market::Market;
use crate::strategy::types::MarketStateSlice;

pub struct DbManager {
    pub pool: PgPool,
    pub token_id_map: HashMap<Address, i32>,
    pub token_symbol_map: HashMap<String, i32>,
    pub market_id_map: HashMap<Address, i32>,
    pub dydx_perp_id_map: HashMap<String, i32>,
}

impl DbManager {
    /// Creates a new database connection and initializes the schema
    #[instrument(skip(config), fields(on_close = true))]
    pub async fn init(config: &Config) -> Result<Self, sqlx::Error> {
        debug!("Initializing database manager");
        let pool = connection::create_pool(config).await?;

        // Ensure schema is initialized (creates tables if needed)
        schema::init_schema(&pool).await?;
        debug!("Database schema initialized");

        // Load ID maps
        let token_id_map = tokens_queries::get_token_id_map(&pool).await?;
        let token_symbol_map = tokens_queries::get_token_symbol_map(&pool).await?;
        let market_id_map = markets_queries::get_market_id_map(&pool).await?;
        let dydx_perp_id_map = dydx_perps_queries::get_dydx_perp_id_map(&pool).await.unwrap_or_default();

        info!(
            token_count = token_id_map.len(),
            market_count = market_id_map.len(),
            "Database manager initialized with existing entities"
        );

        Ok(Self {
            pool,
            token_id_map,
            token_symbol_map,
            market_id_map,
            dydx_perp_id_map,
        })
    }

    /// Internal method to refresh ID maps from the database
    #[instrument(skip(self))]
    pub async fn refresh_id_maps(&mut self) -> Result<(), sqlx::Error> {
        debug!("Refreshing ID maps from database");
        
        let token_id_map = tokens_queries::get_token_id_map(&self.pool).await?;
        let token_symbol_map = tokens_queries::get_token_symbol_map(&self.pool).await?;
        let market_id_map = markets_queries::get_market_id_map(&self.pool).await?;
        let dydx_perp_id_map = dydx_perps_queries::get_dydx_perp_id_map(&self.pool).await.unwrap_or_default();
        
        let token_count_diff = token_id_map.len() as i32 - self.token_id_map.len() as i32;
        let market_count_diff = market_id_map.len() as i32 - self.market_id_map.len() as i32;
        
        self.token_id_map = token_id_map;
        self.token_symbol_map = token_symbol_map;
        self.market_id_map = market_id_map;
        self.dydx_perp_id_map = dydx_perp_id_map;
        
        debug!(
            token_count = self.token_id_map.len(),
            market_count = self.market_id_map.len(),
            token_diff = token_count_diff,
            market_diff = market_count_diff,
            "ID maps refreshed from database"
        );
        
        Ok(())
    }

    /// Prepare token price models from a list of AssetToken objects
    #[instrument(skip(self, tokens_iter))]
    pub async fn prepare_token_prices<I>(&mut self, tokens_iter: I) -> Result<(Vec<NewTokenPriceModel>, Vec<AssetToken>), sqlx::Error>
    where
        I: IntoIterator<Item = Arc<RwLock<AssetToken>>>,
    {
        debug!("Preparing token price models");
        self.refresh_id_maps().await?;

        let mut token_prices = Vec::new();
        let mut failed_tokens = Vec::new();
        let mut count = 0;
        for token_arc in tokens_iter {
            count += 1;
            let token = token_arc.read().await;
            if self.token_id_map.contains_key(&token.address) {
                let new_token_price = NewTokenPriceModel::from(&*token, &self.token_id_map);
                token_prices.push(new_token_price);
            } else {
                debug!(
                    symbol = %token.symbol,
                    address = %token.address,
                    "Token not found in ID map, skipping token price preparation"
                );
                failed_tokens.push(token.clone());
            }
        }
        debug!(
            num_requested = count,
            num_succeeded = token_prices.len(), 
            num_failed = failed_tokens.len(),
            "Token price models prepared"
        );
        Ok((token_prices, failed_tokens))
    }

    /// Insert a batch of token price models into the database
    #[instrument(skip(self, token_prices), fields(batch_size = token_prices.len(), on_close = true))]
    pub async fn insert_token_prices(&self, token_prices: Vec<NewTokenPriceModel>) -> Result<(), sqlx::Error> {
        if token_prices.is_empty() {
            debug!("No token prices to insert");
            return Ok(());
        }
        
        debug!(batch_size = token_prices.len(), "Inserting token prices");
        for new_token_price in token_prices {
            token_prices_queries::insert_token_price(&self.pool, &new_token_price).await?;
        }
        debug!("Token prices insertion completed");
        Ok(())
    }

    /// Prepare market state models from a list of Market objects
    #[instrument(skip(self, markets_iter))]
    pub async fn prepare_market_states<'a, I>(&mut self, markets_iter: I) -> Result<(Vec<NewMarketStateModel>, Vec<Market>), sqlx::Error>
    where
        I: IntoIterator<Item = &'a Market>,
    {
        debug!("Preparing market state models");
        self.refresh_id_maps().await?;

        let mut market_states = Vec::new();
        let mut failed_markets = Vec::new();
        let mut count = 0;
        for market in markets_iter {
            count += 1;
            if self.market_id_map.contains_key(&market.market_token) {
                let new_market_state = NewMarketStateModel::from(market, &self.market_id_map);
                market_states.push(new_market_state);
            } else {
                debug!(
                    market_token = %market.market_token,
                    "Market not found in ID map, skipping market state preparation"
                );
                failed_markets.push(market.clone());
            }
        }
        debug!(
            num_requested = count,
            num_succeeded = market_states.len(),
            num_failed = failed_markets.len(),
            "Market state models prepared"
        );
        Ok((market_states, failed_markets))
    }

    /// Insert a batch of market state models into the database
    #[instrument(skip(self, market_states), fields(batch_size = market_states.len(), on_close = true))]
    pub async fn insert_market_states(&self, market_states: Vec<NewMarketStateModel>) -> Result<(), sqlx::Error> {
        if market_states.is_empty() {
            debug!("No market states to insert");
            return Ok(());
        }
        
        debug!(batch_size = market_states.len(), "Inserting market states");
        for new_market_state in market_states {
            market_states_queries::insert_market_state(&self.pool, &new_market_state).await?;
        }
        debug!("Market states insertion completed");
        Ok(())
    }

    /// Prepare new token models from a list of AssetToken objects
    #[instrument(skip(self, tokens))]
    pub async fn prepare_new_tokens(&mut self, tokens: &[AssetToken]) -> Result<Vec<NewTokenModel>, sqlx::Error> {
        debug!("Preparing new token models");
        self.refresh_id_maps().await?;

        let new_tokens: Vec<NewTokenModel> = tokens
            .iter()
            .filter(|token| !self.token_id_map.contains_key(&token.address))
            .map(|token| NewTokenModel::from(token))
            .collect();
        debug!(
            num_requested = tokens.len(),
            num_added = new_tokens.len(), 
            num_skipped = tokens.len() - new_tokens.len(),
            "New token models prepared"
        );
        Ok(new_tokens)
    }

    /// Prepare new market models from a list of Market objects
    #[instrument(skip(self, markets))]
    pub async fn prepare_new_markets(&mut self, markets: &[&Market]) -> Result<(Vec<NewMarketModel>, Vec<Market>), sqlx::Error> {
        debug!("Preparing new market models");
        self.refresh_id_maps().await?;

        let mut new_markets = Vec::new();
        let mut failed_markets = Vec::new();
        for market in markets {
            if self.token_id_map.contains_key(&market.index_token.read().await.address) &&
               self.token_id_map.contains_key(&market.long_token.read().await.address) &&
               self.token_id_map.contains_key(&market.short_token.read().await.address) {     
                let new_market = NewMarketModel::from_async(market, &self.token_id_map).await;
                new_markets.push(new_market);
            } else {
                failed_markets.push((*market).clone());
            }
        }
        debug!(
            num_requested = markets.len(),
            num_succeeded = new_markets.len(),
            num_failed = failed_markets.len(),
            "New market models prepared"
        );
        Ok((new_markets, failed_markets))
    }

    /// Insert a batch of new token models into the database
    #[instrument(skip(self, tokens), fields(batch_size = tokens.len(), on_close = true))]
    pub async fn insert_tokens(&mut self, tokens: Vec<NewTokenModel>) -> Result<(), sqlx::Error> {
        if tokens.is_empty() {
            debug!("No tokens to insert");
            return Ok(());
        }
        
        // Refresh ID maps to ensure we have the latest state
        self.refresh_id_maps().await?;
        
        debug!(batch_size = tokens.len(), "Inserting tokens");
        let mut inserted_count = 0;
        let mut skipped_count = 0;
        
        for new_token in tokens {
            // Parse the address to check if it already exists
            if let Ok(address) = new_token.address.parse::<Address>() {
                if self.token_id_map.contains_key(&address) {
                    skipped_count += 1;
                    debug!(
                        symbol = %new_token.symbol,
                        address = %new_token.address,
                        "Token already exists in ID map, skipping insertion"
                    );
                    continue;
                }
                
                let id = tokens_queries::insert_token(&self.pool, &new_token).await?;
                self.token_id_map.insert(address, id);
                inserted_count += 1;
                debug!(
                    symbol = %new_token.symbol,
                    address = %new_token.address,
                    id = id,
                    "Token inserted in db and added to ID map"
                );
            } else {
                debug!(
                    symbol = %new_token.symbol,
                    address = %new_token.address,
                    "Failed to parse address, skipping token insertion"
                );
                skipped_count += 1;
            }
        }
        debug!(
            inserted = inserted_count,
            skipped = skipped_count,
            "Token insertion completed"
        );
        Ok(())
    }

    /// Insert a batch of new market models into the database
    #[instrument(skip(self, markets), fields(batch_size = markets.len(), on_close = true))]
    pub async fn insert_markets(&mut self, markets: Vec<NewMarketModel>) -> Result<(), sqlx::Error> {
        if markets.is_empty() {
            debug!("No markets to insert");
            return Ok(());
        }
        
        // Refresh ID maps to ensure we have the latest state
        self.refresh_id_maps().await?;
        
        debug!(batch_size = markets.len(), "Inserting markets");
        let mut inserted_count = 0;
        let mut skipped_count = 0;
        
        for new_market in markets {
            // Parse the address to check if it already exists
            if let Ok(address) = new_market.address.parse::<Address>() {
                if self.market_id_map.contains_key(&address) {
                    skipped_count += 1;
                    debug!(
                        market_address = %new_market.address,
                        "Market already exists in ID map, skipping insertion"
                    );
                    continue;
                }
                
                let id = markets_queries::insert_market(&self.pool, &new_market).await?;
                self.market_id_map.insert(address, id);
                inserted_count += 1;
                debug!(
                    market_address = %new_market.address,
                    id = id,
                    "Market inserted in db and added to ID map"
                );
            } else {
                debug!(
                    market_address = %new_market.address,
                    "Failed to parse address, skipping market insertion"
                );
                skipped_count += 1;
            }
        }
        debug!(
            inserted = inserted_count,
            skipped = skipped_count,
            "Market insertion completed"
        );
        Ok(())
    }

    /// Get display names for all markets by joining with token information
    #[instrument(skip(self), fields(on_close = true))]
    pub async fn get_market_display_names(&self) -> Result<HashMap<Address, String>, sqlx::Error> {
        debug!("Fetching market display names");
        let display_names = market_states_queries::get_market_display_names(&self.pool).await?;
        debug!(count = display_names.len(), "Market display names fetched");
        Ok(display_names)
    }

    /// Fetch full history for each market and construct MarketStateSlice objects
    pub async fn get_market_state_slices(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<MarketStateSlice>, sqlx::Error> {
        let mut slices = Vec::new();

        // Get display names for all markets upfront
        let display_names = self.get_market_display_names().await?;

        // Fetch all market states and token prices concurrently
        let (states_by_market, prices_by_token) = tokio::try_join!(
            market_states_queries::get_all_market_states_in_range(&self.pool, start, end),
            token_prices_queries::get_all_token_prices_in_range(&self.pool, start, end),
        )?;

        // Get market-to-index-token mapping
        let market_index_tokens = markets_queries::get_all_market_index_tokens(&self.pool).await?;

        for (address, market_id) in &self.market_id_map {
            let history = match states_by_market.get(market_id) {
                Some(h) if !h.is_empty() => h,
                _ => continue,
            };

            // Get index token ID for this market
            let index_token_id = match market_index_tokens.get(market_id) {
                Some(&id) => id,
                None => continue,
            };

            // Get index token prices
            let index_token_prices_objects = match prices_by_token.get(&index_token_id) {
                Some(prices) if !prices.is_empty() => prices,
                _ => continue,
            };

            // Get token info
            let index_token = match tokens_queries::get_token_by_id(&self.pool, index_token_id).await? {
                Some(token) => token,
                None => continue,
            };

            // --- INDEX TOKEN DATA ---
            let index_token_address = Address::from_str(&index_token.address)
                .map_err(|_| sqlx::Error::Decode("Invalid token address".into()))?;
            let index_token_symbol = index_token.symbol;
            let index_token_prices: Vec<Decimal> = index_token_prices_objects.iter().map(|p| p.mid_price).collect();
            let index_token_timestamps: Vec<DateTime<Utc>> = index_token_prices_objects.iter().map(|p| p.timestamp).collect();

            let display_name = display_names.get(address).cloned()
                .unwrap_or_else(|| format!("{}/USD [Unknown]", address));
            
            // --- HISTORICAL DATA ---
            let timestamps = history.iter().map(|x| x.timestamp).collect();
            let fees_usd = history.iter().map(|x| x.fees_total.unwrap_or_default()).collect();

            // --- CURRENT STATE ---
            let last_state = history.last().unwrap(); // Safe since is_empty() was checked above

            // PnL data
            let pnl_long = last_state.pnl_long.unwrap_or_default();
            let pnl_short = last_state.pnl_short.unwrap_or_default();
            let pnl_net = last_state.pnl_net.unwrap_or_default();

            // Open interest data
            let oi_long = last_state.open_interest_long.unwrap_or_default();
            let oi_short = last_state.open_interest_short.unwrap_or_default();
            let oi_long_via_tokens = last_state.open_interest_long_via_tokens.unwrap_or_default();
            let oi_short_via_tokens = last_state.open_interest_short_via_tokens.unwrap_or_default();
            let last_index_price = match index_token_prices.last().cloned() {
                Some(price) => price,
                None => {
                    tracing::warn!(
                        market_id = *market_id,
                        address = ?address,
                        "No index prices available for market, skipping"
                    );
                    continue;
                }
            };
            let oi_long_token_amount = oi_long_via_tokens / last_index_price;
            let oi_short_token_amount = oi_short_via_tokens / last_index_price;

            // Pool composition data
            let pool_long_collateral_usd = last_state.pool_long_token_usd.unwrap_or_default();
            let pool_short_collateral_usd = last_state.pool_short_token_usd.unwrap_or_default();
            let pool_long_collateral_token_amount = last_state.pool_long_amount.unwrap_or_default();
            let pool_short_collateral_token_amount = last_state.pool_short_amount.unwrap_or_default();
            let impact_pool_usd = last_state.pool_impact_token_usd.unwrap_or_default();
            let impact_pool_token_amount = last_state.pool_impact_amount.unwrap_or_default();

            slices.push(MarketStateSlice {
                market_address: *address,
                display_name,
                timestamps,
                fees_usd,
                index_token_address,
                index_token_symbol,
                index_prices: index_token_prices,
                index_token_timestamps,
                pnl_long,
                pnl_short,
                pnl_net,
                oi_long,
                oi_short,
                oi_long_via_tokens,
                oi_short_via_tokens,
                oi_long_token_amount,
                oi_short_token_amount,
                pool_long_collateral_usd,
                pool_short_collateral_usd,
                pool_long_collateral_token_amount,
                pool_short_collateral_token_amount,
                impact_pool_usd,
                impact_pool_token_amount,
            });
        }

        Ok(slices)
    }

    /// Fetch all tokens
    #[instrument(skip(self))]
    pub async fn get_all_tokens(&self) -> Result<Vec<TokenModel>, sqlx::Error> {
        let tokens = tokens_queries::get_all_tokens(&self.pool).await?;
        debug!(count = tokens.len(), "Fetched all tokens");
        Ok(tokens)
    }

    /// Fetch all markets
    pub async fn get_all_markets(&self) -> Result<Vec<MarketModel>, sqlx::Error> {
        let markets = markets_queries::get_all_markets(&self.pool).await?;
        debug!(count = markets.len(), "Fetched all markets");
        Ok(markets)
    }

    /// Fetch most recent token price for all tokens
    #[instrument(skip(self))]
    pub async fn get_latest_token_prices(&self) -> Result<Vec<TokenPriceModel>, sqlx::Error> {
        let tokens = token_prices_queries::get_latest_token_prices_for_all_tokens(&self.pool).await?;
        debug!(count = tokens.len(), "Fetched latest token prices");
        Ok(tokens)
    }

    /// Fetch most recent market state for all markets
    #[instrument(skip(self))]
    pub async fn get_latest_market_states(&self) -> Result<Vec<MarketStateModel>, sqlx::Error> {
        let states = market_states_queries::get_latest_market_states_for_all_markets(&self.pool).await?;
        debug!(count = states.len(), "Fetched latest market states");
        Ok(states)
    }

    /// Fetch all asset tokens
    #[instrument(skip(self))]
    pub async fn get_all_asset_tokens(&self) -> Result<Vec<(Address, String, u8, Decimal)>, sqlx::Error> {
        let tokens: Vec<(Address, String, u8, Decimal)> = token_prices_queries::get_all_asset_tokens(&self.pool)
            .await?
            .into_iter()
            .map(|row| {
                (
                    Address::from_str(&row.0).unwrap_or_default(),
                    row.1,
                    row.2 as u8,
                    row.3,
                )
            })
            .collect();
        debug!(count = tokens.len(), "Fetched all asset tokens");
        Ok(tokens)
    }

    /// Fetch all market tokens
    #[instrument(skip(self))]
    pub async fn get_all_market_tokens(&self) -> Result<Vec<(Address, String, Decimal, Address, Address, Address)>, sqlx::Error> {
        let market_tokens: Vec<(Address, String, Decimal, Address, Address, Address)> = market_states_queries::get_all_market_tokens(&self.pool)
            .await?
            .into_iter()
            .map(|row| {
                (
                    Address::from_str(&row.0).unwrap_or_default(),
                    row.1,
                    row.2,
                    Address::from_str(&row.3).unwrap_or_default(),
                    Address::from_str(&row.4).unwrap_or_default(),
                    Address::from_str(&row.5).unwrap_or_default(),
                )
            })
            .collect();
        debug!(count = market_tokens.len(), "Fetched all market tokens");
        Ok(market_tokens)
    }

    /// Fetch most recent price props for a particular market
    #[instrument(skip(self, market_address))]
    pub async fn get_latest_price_props_for_market(&self, market_address: Address) -> Result<Option<(Decimal, Decimal, Decimal, Decimal, Decimal, Decimal)>, sqlx::Error> {
        let market_id = self.market_id_map.get(&market_address)
            .ok_or_else(|| sqlx::Error::RowNotFound)?;
        let price_props: Option<(Decimal, Decimal, Decimal, Decimal, Decimal, Decimal)> = token_prices_queries::get_latest_price_props_for_market(
            &self.pool,
            *market_id,
        ).await?;
        debug!(
            market_address = %market_address,
            price_props = ?price_props,
            "Fetched latest price props for market"
        );
        Ok(price_props)
    }

    /// Convert raw token model to new token model
    #[instrument(skip(self, raw_token))]
    pub async fn convert_raw_token_to_new_token(&mut self, raw_token: RawTokenModel) -> Result<NewTokenModel, sqlx::Error> {
        // For tokens, conversion is direct since no foreign keys are involved
        Ok(NewTokenModel {
            address: raw_token.address,
            symbol: raw_token.symbol,
            decimals: raw_token.decimals,
        })
    }

    /// Convert raw market model to new market model
    #[instrument(skip(self, raw_market))]
    pub async fn convert_raw_market_to_new_market(&mut self, raw_market: RawMarketModel) -> Result<Option<NewMarketModel>, sqlx::Error> {
        self.refresh_id_maps().await?;

        // Parse addresses
        let index_token_address = raw_market.index_token_address.parse::<Address>()
            .map_err(|_| sqlx::Error::Decode("Invalid index token address".into()))?;
        let long_token_address = raw_market.long_token_address.parse::<Address>()
            .map_err(|_| sqlx::Error::Decode("Invalid long token address".into()))?;
        let short_token_address = raw_market.short_token_address.parse::<Address>()
            .map_err(|_| sqlx::Error::Decode("Invalid short token address".into()))?;

        // Check if all required token IDs exist
        if let (Some(&index_token_id), Some(&long_token_id), Some(&short_token_id)) = (
            self.token_id_map.get(&index_token_address),
            self.token_id_map.get(&long_token_address),
            self.token_id_map.get(&short_token_address)
        ) {
            Ok(Some(NewMarketModel {
                address: raw_market.address,
                index_token_id,
                long_token_id,
                short_token_id,
            }))
        } else {
            debug!(
                index_exists = self.token_id_map.contains_key(&index_token_address),
                long_exists = self.token_id_map.contains_key(&long_token_address),
                short_exists = self.token_id_map.contains_key(&short_token_address),
                "Cannot convert raw market - missing token IDs"
            );
            Ok(None)
        }
    }

    /// Convert raw token price model to new token price model
    #[instrument(skip(self, raw_token_price))]
    pub async fn convert_raw_token_price_to_new_token_price(&mut self, raw_token_price: RawTokenPriceModel) -> Result<Option<NewTokenPriceModel>, sqlx::Error> {
        self.refresh_id_maps().await?;

        // Parse token address
        let token_address = raw_token_price.token_address.parse::<Address>()
            .map_err(|_| sqlx::Error::Decode("Invalid token address".into()))?;

        // Check if token ID exists
        if let Some(&token_id) = self.token_id_map.get(&token_address) {
            Ok(Some(NewTokenPriceModel {
                token_id,
                timestamp: raw_token_price.timestamp,
                min_price: raw_token_price.min_price,
                max_price: raw_token_price.max_price,
                mid_price: raw_token_price.mid_price,
            }))
        } else {
            debug!(
                token_address = %token_address,
                "Cannot convert raw token price - missing token ID"
            );
            Ok(None)
        }
    }

    /// Convert raw market state model to new market state model
    #[instrument(skip(self, raw_market_state))]
    pub async fn convert_raw_market_state_to_new_market_state(&mut self, raw_market_state: RawMarketStateModel) -> Result<Option<NewMarketStateModel>, sqlx::Error> {
        self.refresh_id_maps().await?;

        // Parse market address
        let market_address = raw_market_state.market_address.parse::<Address>()
            .map_err(|_| sqlx::Error::Decode("Invalid market address".into()))?;

        // Check if market ID exists
        if let Some(&market_id) = self.market_id_map.get(&market_address) {
            Ok(Some(NewMarketStateModel {
                market_id,
                timestamp: raw_market_state.timestamp,
                borrowing_factor_long: raw_market_state.borrowing_factor_long,
                borrowing_factor_short: raw_market_state.borrowing_factor_short,
                pnl_long: raw_market_state.pnl_long,
                pnl_short: raw_market_state.pnl_short,
                pnl_net: raw_market_state.pnl_net,
                gm_price_min: raw_market_state.gm_price_min,
                gm_price_max: raw_market_state.gm_price_max,
                gm_price_mid: raw_market_state.gm_price_mid,
                pool_long_amount: raw_market_state.pool_long_amount,
                pool_short_amount: raw_market_state.pool_short_amount,
                pool_impact_amount: raw_market_state.pool_impact_amount,
                pool_long_token_usd: raw_market_state.pool_long_token_usd,
                pool_short_token_usd: raw_market_state.pool_short_token_usd,
                pool_impact_token_usd: raw_market_state.pool_impact_token_usd,
                open_interest_long: raw_market_state.open_interest_long,
                open_interest_short: raw_market_state.open_interest_short,
                open_interest_long_amount: raw_market_state.open_interest_long_amount,
                open_interest_short_amount: raw_market_state.open_interest_short_amount,
                open_interest_long_via_tokens: raw_market_state.open_interest_long_via_tokens,
                open_interest_short_via_tokens: raw_market_state.open_interest_short_via_tokens,
                utilization: raw_market_state.utilization,
                swap_volume: raw_market_state.swap_volume,
                trading_volume: raw_market_state.trading_volume,
                fees_position: raw_market_state.fees_position,
                fees_liquidation: raw_market_state.fees_liquidation,
                fees_swap: raw_market_state.fees_swap,
                fees_borrowing: raw_market_state.fees_borrowing,
                fees_total: raw_market_state.fees_total,
            }))
        } else {
            debug!(
                market_address = %market_address,
                "Cannot convert raw market state - missing market ID"
            );
            Ok(None)
        }
    }

    /// Convert raw dYdX perp model to new model
    #[instrument(skip(self, raw_perp))]
    pub async fn convert_raw_dydx_perp_to_new_dydx_perp(
        &mut self,
        raw_perp: RawDydxPerpModel,
    ) -> Result<Option<NewDydxPerpModel>, sqlx::Error> {
        self.refresh_id_maps().await?;
        if let Some(&token_id) = self.token_symbol_map.get(&raw_perp.token_symbol) {
            Ok(Some(NewDydxPerpModel {
                token_id,
                ticker: raw_perp.ticker,
            }))
        } else {
            Ok(None)
        }
    }

    /// Convert raw dYdX perp state model to new model
    #[instrument(skip(self, raw_state))]
    pub async fn convert_raw_dydx_perp_state_to_new_dydx_perp_state(
        &mut self,
        raw_state: RawDydxPerpStateModel,
    ) -> Result<Option<NewDydxPerpStateModel>, sqlx::Error> {
        self.refresh_id_maps().await?;
        if let Some(&perp_id) = self.dydx_perp_id_map.get(&raw_state.ticker) {
            Ok(Some(NewDydxPerpStateModel {
                dydx_perp_id: perp_id,
                timestamp: raw_state.timestamp,
                funding_rate: raw_state.funding_rate,
                initial_margin_fraction: raw_state.initial_margin_fraction,
                maintenance_margin_fraction: raw_state.maintenance_margin_fraction,
                oracle_price: raw_state.oracle_price,
                open_interest: raw_state.open_interest,
            }))
        } else {
            Ok(None)
        }
    }

    /// Insert a strategy run and return its ID
    #[instrument(skip(self, run))]
    pub async fn insert_strategy_run(&self, run: &NewStrategyRunModel) -> Result<i32, sqlx::Error> {
        strategy_runs_queries::insert_strategy_run(&self.pool, run).await
    }

    /// Insert a strategy target and return its ID
    #[instrument(skip(self, target))]
    pub async fn insert_strategy_target(&self, target: &NewStrategyTargetModel) -> Result<i32, sqlx::Error> {
        strategy_targets_queries::insert_strategy_target(&self.pool, target).await
    }

    /// Insert a trade record and return its ID
    #[instrument(skip(self, trade))]
    pub async fn insert_trade(&self, trade: &NewTradeModel) -> Result<i32, sqlx::Error> {
        trades_queries::insert_trade(&self.pool, trade).await
    }

    /// Insert a portfolio snapshot and return its ID
    #[instrument(skip(self, snapshot))]
    pub async fn insert_portfolio_snapshot(&self, snapshot: &NewPortfolioSnapshotModel) -> Result<i32, sqlx::Error> {
        portfolio_snapshots_queries::insert_portfolio_snapshot(&self.pool, snapshot).await
    }

    /// Insert position snapshot rows
    #[instrument(skip(self, positions))]
    pub async fn insert_position_snapshots(
        &self,
        positions: Vec<NewPositionSnapshotModel>,
    ) -> Result<(), sqlx::Error> {
        for position in positions {
            position_snapshots_queries::insert_position_snapshot(&self.pool, &position).await?;
        }
        Ok(())
    }

    /// Insert a dYdX perp and return its ID
    #[instrument(skip(self, perp))]
    pub async fn insert_dydx_perp(&self, perp: &NewDydxPerpModel) -> Result<i32, sqlx::Error> {
        dydx_perps_queries::insert_dydx_perp(&self.pool, perp).await
    }

    /// Insert a batch of dYdX perp states
    #[instrument(skip(self, states))]
    pub async fn insert_dydx_perp_states(
        &self,
        states: Vec<NewDydxPerpStateModel>,
    ) -> Result<(), sqlx::Error> {
        for state in states {
            dydx_perp_states_queries::insert_dydx_perp_state(&self.pool, &state).await?;
        }
        Ok(())
    }

    /// Fetch latest portfolio snapshot
    #[instrument(skip(self))]
    pub async fn get_latest_portfolio_snapshot(&self) -> Result<Option<PortfolioSnapshotModel>, sqlx::Error> {
        portfolio_snapshots_queries::get_latest_portfolio_snapshot(&self.pool).await
    }

    /// Fetch latest strategy run
    #[instrument(skip(self))]
    pub async fn get_latest_strategy_run(&self) -> Result<Option<StrategyRunModel>, sqlx::Error> {
        strategy_runs_queries::get_latest_strategy_run(&self.pool).await
    }

    /// Fetch strategy runs in a time range
    #[instrument(skip(self))]
    pub async fn get_strategy_runs_in_range(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<StrategyRunModel>, sqlx::Error> {
        strategy_runs_queries::get_strategy_runs_in_range(&self.pool, start, end).await
    }
}
