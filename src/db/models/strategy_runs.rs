use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::FromRow;

#[derive(Debug, FromRow)]
pub struct StrategyRunModel {
    pub id: i32,
    pub timestamp: DateTime<Utc>,
    pub strategy_version: String,
    pub total_weight: Option<Decimal>,
    pub expected_return_bps: Option<Decimal>,
    pub volatility_bps: Option<Decimal>,
    pub sharpe: Option<Decimal>,
}

#[derive(Debug, Clone)]
pub struct NewStrategyRunModel {
    pub timestamp: DateTime<Utc>,
    pub strategy_version: String,
    pub total_weight: Decimal,
    pub expected_return_bps: Decimal,
    pub volatility_bps: Decimal,
    pub sharpe: Decimal,
}
