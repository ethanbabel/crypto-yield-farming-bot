use rust_decimal::Decimal;
use chrono::{DateTime, Utc};
use ethers::types::Address;
use std::collections::HashMap;
use sqlx::FromRow;

use crate::token::AssetToken;

#[derive(Debug, FromRow)]
pub struct TokenPriceModel {
    pub id: i32,
    pub token_id: i32,
    pub timestamp: chrono::DateTime<Utc>,
    pub min_price: Decimal,
    pub max_price: Decimal,
    pub mid_price: Decimal,
}

#[derive(Debug)]
pub struct NewTokenPriceModel {
    pub token_id: i32,
    pub timestamp: chrono::DateTime<Utc>,
    pub min_price: Decimal,
    pub max_price: Decimal,
    pub mid_price: Decimal,
}

impl NewTokenPriceModel {
    pub fn from(token: &AssetToken, token_id_map: &HashMap<Address, i32>) -> Self {
        let token_id = token_id_map[&token.address];
        let timestamp: DateTime<Utc> = token.updated_at.unwrap().into();

        Self {
            token_id,
            timestamp,
            min_price: token.last_min_price_usd.unwrap(),
            max_price: token.last_max_price_usd.unwrap(),
            mid_price: token.last_mid_price_usd.unwrap(),
        }
    }
}