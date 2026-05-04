use chrono::{DateTime, Utc};
use sqlx::FromRow;

pub const EXECUTION_MODE_NORMAL: &str = "normal";
pub const EXECUTION_MODE_STABLE_ONLY: &str = "stable_only";

#[derive(Debug, Clone, FromRow)]
pub struct ExecutionControlStateModel {
    pub singleton_id: bool,
    pub mode: String,
    pub target_stable_symbol: Option<String>,
    pub min_strategy_run_id_to_execute: Option<i32>,
    pub updated_at: DateTime<Utc>,
    pub updated_by: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewExecutionControlStateModel {
    pub mode: String,
    pub target_stable_symbol: Option<String>,
    pub min_strategy_run_id_to_execute: Option<i32>,
    pub updated_at: DateTime<Utc>,
    pub updated_by: Option<String>,
    pub reason: Option<String>,
}
