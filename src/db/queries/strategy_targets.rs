use sqlx::PgPool;

use crate::db::models::strategy_targets::{NewStrategyTargetModel, StrategyTargetModel};

pub async fn insert_strategy_target(pool: &PgPool, target: &NewStrategyTargetModel) -> Result<i32, sqlx::Error> {
    let row = sqlx::query!(
        r#"
        INSERT INTO strategy_targets (strategy_run_id, market_id, target_weight, expected_return_bps, variance_bps)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id
        "#,
        target.strategy_run_id,
        target.market_id,
        target.target_weight,
        target.expected_return_bps,
        target.variance_bps,
    )
    .fetch_one(pool)
    .await?;

    Ok(row.id)
}

pub async fn get_targets_for_run(pool: &PgPool, run_id: i32) -> Result<Vec<StrategyTargetModel>, sqlx::Error> {
    sqlx::query_as!(
        StrategyTargetModel,
        r#"
        SELECT id, strategy_run_id, market_id, target_weight, expected_return_bps, variance_bps
        FROM strategy_targets
        WHERE strategy_run_id = $1
        "#,
        run_id
    )
    .fetch_all(pool)
    .await
}
