use sqlx::PgPool;
use std::collections::HashMap;
use ethers::types::Address;
use std::sync::Arc;
use tokio::sync::RwLock;

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
    pub async fn init(config: &Config) -> Result<Self, sqlx::Error> {
        let pool = connection::create_pool(config).await?;

        // Ensure schema is initialized (creates tables if needed)
        schema::init_schema(&pool).await?;

        // Load ID maps
        let token_id_map = tokens_queries::get_token_id_map(&pool).await?;
        let market_id_map = markets_queries::get_market_id_map(&pool).await?;

        Ok(Self {
            pool,
            token_id_map,
            market_id_map,
        })
    }

    /// Add all tokens if not already present, and update `token_id_map`
    pub async fn sync_tokens<I>(&mut self, tokens_iter: I) -> Result<(), sqlx::Error>
    where
        I: IntoIterator<Item = Arc<RwLock<AssetToken>>>,
    {
        for token_arc in tokens_iter {
            let token = token_arc.read().await;
            if self.token_id_map.contains_key(&token.address) {
                continue;
            }

            let new_token = NewTokenModel::from(&*token);

            let id = tokens_queries::insert_token(&self.pool, &new_token).await?;
            self.token_id_map.insert(token.address, id);
        }
        Ok(())
    }

    /// Add all markets if not already present, and update `market_id_map`
    pub async fn sync_markets<'a, I>(&mut self, markets_iter: I) -> Result<(), sqlx::Error>
    where
        I: IntoIterator<Item = &'a Market>,
    {
        for market in markets_iter {
            if self.market_id_map.contains_key(&market.market_token) {
                continue;
            }

            // Use async reads to extract token addresses for NewMarketModel
            let new_market = NewMarketModel::from_async(market, &self.token_id_map).await;

            let id = markets_queries::insert_market(&self.pool, &new_market).await?;
            self.market_id_map.insert(market.market_token, id);
        }
        Ok(())
    }

    pub async fn prepare_token_prices<I>(&self, tokens_iter: I) -> Vec<NewTokenPriceModel>
    where
        I: IntoIterator<Item = Arc<RwLock<AssetToken>>>,
    {
        let mut token_prices = Vec::new();
        for token_arc in tokens_iter {
            let token = token_arc.read().await;
            let new_token_price = NewTokenPriceModel::from(&*token, &self.token_id_map);
            token_prices.push(new_token_price);
        }
        token_prices
    }

    pub async fn insert_token_prices(&self, token_prices: Vec<NewTokenPriceModel>) -> Result<(), sqlx::Error> {
        for new_token_price in token_prices {
            token_prices_queries::insert_token_price(&self.pool, &new_token_price).await?;
        }
        Ok(())
    }

    pub fn prepare_market_states<'a, I>(&self, markets_iter: I) -> Vec<NewMarketStateModel>
    where
        I: IntoIterator<Item = &'a Market>,
    {
        let mut market_states = Vec::new();
        for market in markets_iter {
            let new_market_state = NewMarketStateModel::from(market, &self.market_id_map);
            market_states.push(new_market_state);
        }
        market_states
    }

    pub async fn insert_market_states(&self, market_states: Vec<NewMarketStateModel>) -> Result<(), sqlx::Error> {
        for new_market_state in market_states {
            market_states_queries::insert_market_state(&self.pool, &new_market_state).await?;
        }
        Ok(())
    }
}