use sqlx::FromRow;
use ethers::types::Address;
use ethers::utils::to_checksum;
use std::collections::HashMap;
use serde::{Serialize, Deserialize};

use crate::data_ingestion::market::market::Market;

#[derive(Debug, FromRow)]
pub struct MarketModel {
    pub id: i32,
    pub address: String,
    pub index_token_id: i32,
    pub long_token_id: i32,
    pub short_token_id: i32,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RawMarketModel {
    pub address: String,
    pub index_token_address: String,
    pub long_token_address: String,
    pub short_token_address: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NewMarketModel {
    pub address: String,
    pub index_token_id: i32,
    pub long_token_id: i32,
    pub short_token_id: i32,
}

impl RawMarketModel {
    pub async fn from_async(market: &Market) -> Self {
        let index_token = market.index_token.read().await;
        let long_token = market.long_token.read().await;
        let short_token = market.short_token.read().await;
        Self {
            address: to_checksum(&market.market_token, None),
            index_token_address: to_checksum(&index_token.address, None),
            long_token_address: to_checksum(&long_token.address, None),
            short_token_address: to_checksum(&short_token.address, None),
        }
    }
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
            address: to_checksum(&market.market_token, None),
            index_token_id,
            long_token_id,
            short_token_id,
        }
    }
}