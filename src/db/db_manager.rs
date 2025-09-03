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
};
use super::models::{
    tokens::{TokenModel, NewTokenModel, RawTokenModel},
    markets::{MarketModel, NewMarketModel, RawMarketModel},
    token_prices::{TokenPriceModel, NewTokenPriceModel, RawTokenPriceModel},
    market_states::{MarketStateModel, NewMarketStateModel, RawMarketStateModel},
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

    /// Internal method to refresh ID maps from the database
    #[instrument(skip(self))]
    pub async fn refresh_id_maps(&mut self) -> Result<(), sqlx::Error> {
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

        for (address, market_id) in &self.market_id_map {
            let history = market_states_queries::get_market_state_history_in_range(
                &self.pool, *market_id, start, end
            ).await?;

            // Skip markets with no historical data
            if history.is_empty() {
                continue;
            }

            // Get index token data
            let (index_token_address, index_token_symbol, index_token_timestamps, index_prices) = match self.get_index_token_prices_for_market(
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
                fees_usd,
                index_token_address,
                index_token_symbol,
                index_prices,
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

    /// Get index token prices for a specific market within a time range
    pub async fn get_index_token_prices_for_market(
        &self,
        market_id: i32,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<(Address, String, Vec<DateTime<Utc>>, Vec<Decimal>), sqlx::Error> {
        // Get the index token ID for this market
        let index_token_id = markets_queries::get_market_index_token_id(&self.pool, market_id)
            .await?
            .ok_or_else(|| sqlx::Error::RowNotFound)?;

        // Get the index token address and symbol
        let index_token = tokens_queries::get_token_by_id(&self.pool, index_token_id)
            .await?
            .ok_or_else(|| sqlx::Error::RowNotFound)?;
        let index_token_address = Address::from_str(&index_token.address)
            .map_err(|_| sqlx::Error::Decode("Invalid token address".into()))?;

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

        Ok((index_token_address, index_token.symbol, timestamps, prices))
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
}