use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, FromRow)]
pub struct PositionSnapshotModel {
    pub id: i32,
    pub portfolio_snapshot_id: i32,
    pub position_type: String,
    pub market_id: Option<i32>,
    pub token_id: Option<i32>,
    pub symbol: Option<String>,
    pub size: Option<Decimal>,
    pub usd_value: Option<Decimal>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NewPositionSnapshotModel {
    pub portfolio_snapshot_id: i32,
    pub position_type: String,
    pub market_id: Option<i32>,
    pub token_id: Option<i32>,
    pub symbol: Option<String>,
    pub size: Option<Decimal>,
    pub usd_value: Option<Decimal>,
}
