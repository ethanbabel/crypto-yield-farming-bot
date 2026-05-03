use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::FromRow;

#[derive(Debug, FromRow)]
pub struct TradeModel {
    pub id: i32,
    pub timestamp: DateTime<Utc>,
    pub action_type: String,
    pub strategy_run_id: Option<i32>,
    pub market_id: Option<i32>,
    pub from_token_id: Option<i32>,
    pub to_token_id: Option<i32>,
    pub amount_in: Option<Decimal>,
    pub amount_out: Option<Decimal>,
    pub usd_value: Option<Decimal>,
    pub fee_usd: Option<Decimal>,
    pub tx_hash: Option<String>,
    pub status: String,
    pub details: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewTradeModel {
    pub timestamp: DateTime<Utc>,
    pub action_type: String,
    pub strategy_run_id: Option<i32>,
    pub market_id: Option<i32>,
    pub from_token_id: Option<i32>,
    pub to_token_id: Option<i32>,
    pub amount_in: Option<Decimal>,
    pub amount_out: Option<Decimal>,
    pub usd_value: Option<Decimal>,
    pub fee_usd: Option<Decimal>,
    pub tx_hash: Option<String>,
    pub status: String,
    pub details: Option<String>,
}
