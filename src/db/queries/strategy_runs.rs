use sqlx::PgPool;

use crate::db::models::strategy_runs::{NewStrategyRunModel, StrategyRunModel};

pub async fn insert_strategy_run(pool: &PgPool, run: &NewStrategyRunModel) -> Result<i32, sqlx::Error> {
    let row = sqlx::query!(
        r#"
        INSERT INTO strategy_runs (timestamp, total_weight, expected_return_bps, volatility_bps, sharpe)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id
        "#,
        run.timestamp,
        run.total_weight,
        run.expected_return_bps,
        run.volatility_bps,
        run.sharpe,
    )
    .fetch_one(pool)
    .await?;

    Ok(row.id)
}

pub async fn get_latest_strategy_run(pool: &PgPool) -> Result<Option<StrategyRunModel>, sqlx::Error> {
    sqlx::query_as!(
        StrategyRunModel,
        r#"
        SELECT id, timestamp, total_weight, expected_return_bps, volatility_bps, sharpe
        FROM strategy_runs
        ORDER BY timestamp DESC
        LIMIT 1
        "#
    )
    .fetch_optional(pool)
    .await
}
