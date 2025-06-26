use sqlx::PgPool;
use std::collections::HashMap;
use ethers::types::Address;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, debug, instrument};

use super::connection;
use super::schema;
use super::queries::{
    tokens as tokens_queries,
    markets as markets_queries,
    token_prices as token_prices_queries,
    market_states as market_states_queries,
};
use super::models::{
    tokens::{NewTokenModel, TokenModel},
    markets::{NewMarketModel, MarketModel},
    token_prices::{NewTokenPriceModel, TokenPriceModel},
    market_states::{NewMarketStateModel, MarketStateModel},
};
use crate::config::Config;
use crate::token::token::AssetToken;
use crate::market::market::Market;

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
}