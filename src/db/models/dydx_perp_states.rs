use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;

#[derive(Debug, FromRow)]
pub struct DydxPerpStateModel {
    pub id: i32,
    pub dydx_perp_id: i32,
    pub timestamp: DateTime<Utc>,
    pub funding_rate: Option<Decimal>,
    pub initial_margin_fraction: Option<Decimal>,
    pub maintenance_margin_fraction: Option<Decimal>,
    pub oracle_price: Option<Decimal>,
    pub open_interest: Option<Decimal>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RawDydxPerpStateModel {
    pub ticker: String,
    pub timestamp: DateTime<Utc>,
    pub funding_rate: Option<Decimal>,
    pub initial_margin_fraction: Option<Decimal>,
    pub maintenance_margin_fraction: Option<Decimal>,
    pub oracle_price: Option<Decimal>,
    pub open_interest: Option<Decimal>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NewDydxPerpStateModel {
    pub dydx_perp_id: i32,
    pub timestamp: DateTime<Utc>,
    pub funding_rate: Option<Decimal>,
    pub initial_margin_fraction: Option<Decimal>,
    pub maintenance_margin_fraction: Option<Decimal>,
    pub oracle_price: Option<Decimal>,
    pub open_interest: Option<Decimal>,
}
