use sqlx::PgPool;
use std::collections::HashMap;
use ethers::types::Address;
use std::sync::Arc;
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
};
use super::models::{
    tokens::NewTokenModel,
    markets::NewMarketModel,
    token_prices::NewTokenPriceModel,
    market_states::NewMarketStateModel,
};
use crate::config::Config;
use crate::data_ingestion::token::token::AssetToken;
use crate::data_ingestion::market::market::Market;
use crate::strategy::types::MarketStateSlice;

pub struct DbManager {
    pub pool: PgPool,
    pub token_id_map: HashMap<Address, i32>,
    pub market_id_map: HashMap<Address, i32>,
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
        let market_id_map = markets_queries::get_market_id_map(&pool).await?;

        info!(
            token_count = token_id_map.len(),
            market_count = market_id_map.len(),
            "Database manager initialized with existing entities"
        );

        Ok(Self {
            pool,
            token_id_map,
            market_id_map,
        })
    }

    /// Add all tokens if not already present, and update `token_id_map`
    #[instrument(skip(self, tokens_iter), fields(on_close = true))]
    pub async fn sync_tokens<I>(&mut self, tokens_iter: I) -> Result<(), sqlx::Error>
    where
        I: IntoIterator<Item = Arc<RwLock<AssetToken>>>,
    {
        debug!("Syncing tokens with database");
        let mut synced_count = 0;
        let mut new_count = 0;
        
        for token_arc in tokens_iter {
            let token = token_arc.read().await;
            synced_count += 1;
            
            if self.token_id_map.contains_key(&token.address) {
                continue;
            }

            let new_token = NewTokenModel::from(&*token);
            let id = tokens_queries::insert_token(&self.pool, &new_token).await?;
            self.token_id_map.insert(token.address, id);
            new_count += 1;
            
            debug!(
                symbol = %token.symbol,
                address = %token.address,
                id = id,
                "New token added to database"
            );
        }
        
        info!(
            synced_count = synced_count,
            new_count = new_count,
            "Token sync completed"
        );
        Ok(())
    }

    /// Add all markets if not already present, and update `market_id_map`
    #[instrument(skip(self, markets_iter), fields(on_close = true))]
    pub async fn sync_markets<'a, I>(&mut self, markets_iter: I) -> Result<(), sqlx::Error>
    where
        I: IntoIterator<Item = &'a Market>,
    {
        debug!("Syncing markets with database");
        let mut synced_count = 0;
        let mut new_count = 0;
        
        for market in markets_iter {
            synced_count += 1;
            
            if self.market_id_map.contains_key(&market.market_token) {
                continue;
            }

            // Use async reads to extract token addresses for NewMarketModel
            let new_market = NewMarketModel::from_async(market, &self.token_id_map).await;
            let id = markets_queries::insert_market(&self.pool, &new_market).await?;
            self.market_id_map.insert(market.market_token, id);
            new_count += 1;
            
            debug!(
                market_token = %market.market_token,
                id = id,
                "New market added to database"
            );
        }
        
        info!(
            synced_count = synced_count,
            new_count = new_count,
            "Market sync completed"
        );
        Ok(())
    }

    /// Internal method to refresh ID maps from the database
    #[instrument(skip(self))]
    async fn refresh_id_maps(&mut self) -> Result<(), sqlx::Error> {
        debug!("Refreshing ID maps from database");
        
        let token_id_map = tokens_queries::get_token_id_map(&self.pool).await?;
        let market_id_map = markets_queries::get_market_id_map(&self.pool).await?;
        
        let token_count_diff = token_id_map.len() as i32 - self.token_id_map.len() as i32;
        let market_count_diff = market_id_map.len() as i32 - self.market_id_map.len() as i32;
        
        self.token_id_map = token_id_map;
        self.market_id_map = market_id_map;
        
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
    pub async fn prepare_token_prices<I>(&self, tokens_iter: I) -> Vec<NewTokenPriceModel>
    where
        I: IntoIterator<Item = Arc<RwLock<AssetToken>>>,
    {
        debug!("Preparing token price models");
        let mut token_prices = Vec::new();
        for token_arc in tokens_iter {
            let token = token_arc.read().await;
            let new_token_price = NewTokenPriceModel::from(&*token, &self.token_id_map);
            token_prices.push(new_token_price);
        }
        debug!(count = token_prices.len(), "Token price models prepared");
        token_prices
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
    pub fn prepare_market_states<'a, I>(&self, markets_iter: I) -> Vec<NewMarketStateModel>
    where
        I: IntoIterator<Item = &'a Market>,
    {
        debug!("Preparing market state models");
        let mut market_states = Vec::new();
        for market in markets_iter {
            let new_market_state = NewMarketStateModel::from(market, &self.market_id_map);
            market_states.push(new_market_state);
        }
        debug!(count = market_states.len(), "Market state models prepared");
        market_states
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
    pub fn prepare_new_tokens(&self, tokens: &[AssetToken]) -> Vec<NewTokenModel> {
        debug!("Preparing new token models");
        let new_tokens: Vec<NewTokenModel> = tokens
            .iter()
            .map(|token| NewTokenModel::from(token))
            .collect();
        debug!(count = new_tokens.len(), "New token models prepared");
        new_tokens
    }

    /// Prepare new market models from a list of Market objects
    #[instrument(skip(self, markets))]
    pub async fn prepare_new_markets(&self, markets: &[&Market]) -> Vec<NewMarketModel> {
        debug!("Preparing new market models");
        let mut new_markets = Vec::new();
        for market in markets {
            let new_market = NewMarketModel::from_async(market, &self.token_id_map).await;
            new_markets.push(new_market);
        }
        debug!(count = new_markets.len(), "New market models prepared");
        new_markets
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

        for (address, market_id) in &self.market_id_map {
            let history = market_states_queries::get_market_state_history_in_range(
                &self.pool, *market_id, start, end
            ).await?;

            // Skip markets with no historical data
            if history.is_empty() {
                continue;
            }

            // Get index token prices for this market
            let (index_token_timestamps, index_prices) = match self.get_index_token_prices_for_market(
                *market_id, start, end
            ).await {
                Ok(prices) => prices,
                Err(e) => {
                    tracing::warn!(
                        market_id = *market_id,
                        address = ?address,
                        error = ?e,
                        "Failed to get index token prices for market, skipping"
                    );
                    continue;
                }
            };

            // Get the proper display name or fallback to address
            let display_name = display_names.get(address).cloned().unwrap_or_else(|| format!("{}/USD [Unknown]", address));
            
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
            let last_index_price = match index_prices.last().cloned() {
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
                index_prices,
                index_token_timestamps,
                fees_usd,
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

    /// Get index token prices for a specific market within a time range
    pub async fn get_index_token_prices_for_market(
        &self,
        market_id: i32,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<(Vec<DateTime<Utc>>, Vec<Decimal>), sqlx::Error> {
        // Get the index token ID for this market
        let index_token_id = markets_queries::get_market_index_token_id(&self.pool, market_id)
            .await?
            .ok_or_else(|| sqlx::Error::RowNotFound)?;

        // Fetch the token price history
        let price_history = token_prices_queries::get_token_price_history_in_range(
            &self.pool, 
            index_token_id, 
            start, 
            end
        ).await?;

        // Extract timestamps and mid prices
        let timestamps = price_history.iter().map(|p| p.timestamp).collect();
        let prices = price_history.iter().map(|p| p.mid_price).collect();

        Ok((timestamps, prices))
    }
}