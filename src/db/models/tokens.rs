use sqlx::FromRow;
use ethers::utils::to_checksum;
use serde::{Serialize, Deserialize};

use crate::data_ingestion::token::token::AssetToken;

#[derive(Debug, FromRow)]
pub struct TokenModel {
    pub id: i32,
    pub address: String,
    pub symbol: String,
    pub decimals: i32,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NewTokenModel {
    pub address: String,
    pub symbol: String,
    pub decimals: i32,
}

impl NewTokenModel {
    pub fn from(token: &AssetToken) -> Self {
        Self {
            address: to_checksum(&token.address, None),
            symbol: token.symbol.clone(),
            decimals: token.decimals as i32,
        }
    }
}