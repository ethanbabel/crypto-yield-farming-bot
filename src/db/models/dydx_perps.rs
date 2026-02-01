use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, FromRow)]
pub struct DydxPerpModel {
    pub id: i32,
    pub token_id: i32,
    pub ticker: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RawDydxPerpModel {
    pub token_symbol: String,
    pub ticker: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NewDydxPerpModel {
    pub token_id: i32,
    pub ticker: String,
}
