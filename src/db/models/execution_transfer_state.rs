use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use sqlx::FromRow;

pub const EXECUTION_TRANSFER_DIRECTION_TO_DYDX: &str = "arbitrum_to_dydx";
pub const EXECUTION_TRANSFER_DIRECTION_FROM_DYDX: &str = "dydx_to_arbitrum";

#[derive(Debug, Clone, FromRow)]
pub struct ExecutionTransferStateModel {
    pub singleton_id: bool,
    pub direction: Option<String>,
    pub tx_hash: Option<String>,
    pub chain_id: Option<String>,
    pub amount_usd: Option<Decimal>,
    pub source_balance_before: Option<Decimal>,
    pub destination_balance_before: Option<Decimal>,
    pub expected_time_to_complete_secs: Option<i64>,
    pub initiated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub struct NewExecutionTransferStateModel {
    pub direction: Option<String>,
    pub tx_hash: Option<String>,
    pub chain_id: Option<String>,
    pub amount_usd: Option<Decimal>,
    pub source_balance_before: Option<Decimal>,
    pub destination_balance_before: Option<Decimal>,
    pub expected_time_to_complete_secs: Option<i64>,
    pub initiated_at: Option<DateTime<Utc>>,
}
