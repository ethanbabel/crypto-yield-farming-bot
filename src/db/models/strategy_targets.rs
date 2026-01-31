use rust_decimal::Decimal;
use sqlx::FromRow;

#[derive(Debug, FromRow)]
pub struct StrategyTargetModel {
    pub id: i32,
    pub strategy_run_id: i32,
    pub market_id: Option<i32>,
    pub target_weight: Decimal,
    pub expected_return_bps: Option<Decimal>,
    pub variance_bps: Option<Decimal>,
}

#[derive(Debug, Clone)]
pub struct NewStrategyTargetModel {
    pub strategy_run_id: i32,
    pub market_id: i32,
    pub target_weight: Decimal,
    pub expected_return_bps: Decimal,
    pub variance_bps: Decimal,
}
