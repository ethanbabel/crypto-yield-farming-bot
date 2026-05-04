use chrono::{DateTime, Utc};
use sqlx::FromRow;

#[derive(Debug, Clone, FromRow)]
pub struct ExecutionControlEventModel {
    pub id: i32,
    pub timestamp: DateTime<Utc>,
    pub event_type: String,
    pub mode: String,
    pub target_stable_symbol: Option<String>,
    pub min_strategy_run_id_to_execute: Option<i32>,
    pub updated_by: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NewExecutionControlEventModel {
    pub timestamp: DateTime<Utc>,
    pub event_type: String,
    pub mode: String,
    pub target_stable_symbol: Option<String>,
    pub min_strategy_run_id_to_execute: Option<i32>,
    pub updated_by: Option<String>,
    pub reason: Option<String>,
}
