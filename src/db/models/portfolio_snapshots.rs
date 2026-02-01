use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::FromRow;

#[derive(Debug, FromRow)]
pub struct PortfolioSnapshotModel {
    pub id: i32,
    pub timestamp: DateTime<Utc>,
    pub total_value_usd: Option<Decimal>,
    pub market_value_usd: Option<Decimal>,
    pub asset_value_usd: Option<Decimal>,
    pub hedge_value_usd: Option<Decimal>,
    pub pnl_usd: Option<Decimal>,
}

#[derive(Debug, Clone)]
pub struct NewPortfolioSnapshotModel {
    pub timestamp: DateTime<Utc>,
    pub total_value_usd: Decimal,
    pub market_value_usd: Decimal,
    pub asset_value_usd: Decimal,
    pub hedge_value_usd: Decimal,
    pub pnl_usd: Decimal,
}
