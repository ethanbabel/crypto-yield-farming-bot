use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::FromRow;

#[derive(Debug, FromRow)]
pub struct PortfolioSnapshotModel {
    pub id: i32,
    pub strategy_run_id: Option<i32>,
    pub timestamp: DateTime<Utc>,
    pub total_value_usd: Option<Decimal>,
    pub arbitrum_value_usd: Option<Decimal>,
    pub market_value_usd: Option<Decimal>,
    pub asset_value_usd: Option<Decimal>,
    pub native_balance: Option<Decimal>,
    pub native_value_usd: Option<Decimal>,
    pub dydx_main_usdc: Option<Decimal>,
    pub dydx_subaccount_equity: Option<Decimal>,
    pub dydx_free_collateral: Option<Decimal>,
    pub pnl_usd: Option<Decimal>,
}

#[derive(Debug, Clone)]
pub struct NewPortfolioSnapshotModel {
    pub strategy_run_id: Option<i32>,
    pub timestamp: DateTime<Utc>,
    pub total_value_usd: Decimal,
    pub arbitrum_value_usd: Decimal,
    pub market_value_usd: Decimal,
    pub asset_value_usd: Decimal,
    pub native_balance: Decimal,
    pub native_value_usd: Decimal,
    pub dydx_main_usdc: Decimal,
    pub dydx_subaccount_equity: Decimal,
    pub dydx_free_collateral: Decimal,
    pub pnl_usd: Decimal,
}
