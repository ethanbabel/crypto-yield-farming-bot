use sqlx::FromRow;
use ethers::types::Address;
use std::collections::HashMap;

use crate::market::Market;

#[derive(Debug, FromRow)]
pub struct MarketModel {
    pub id: i32,
    pub address: String,
    pub index_token_id: i32,
    pub long_token_id: i32,
    pub short_token_id: i32,
}

#[derive(Debug)]
pub struct NewMarketModel {
    pub address: String,
    pub index_token_id: i32,
    pub long_token_id: i32,
    pub short_token_id: i32,
}

impl NewMarketModel {
    pub async fn from_async(market: &Market, token_id_map: &HashMap<Address, i32>) -> Self {
        let index_token = market.index_token.read().await;
        let long_token = market.long_token.read().await;
        let short_token = market.short_token.read().await;
        let index_token_id = token_id_map[&index_token.address];
        let long_token_id = token_id_map[&long_token.address];
        let short_token_id = token_id_map[&short_token.address];
        Self {
            address: market.market_token.to_string(),
            index_token_id,
            long_token_id,
            short_token_id,
        }
    }
}