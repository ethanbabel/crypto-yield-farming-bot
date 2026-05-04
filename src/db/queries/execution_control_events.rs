use sqlx::PgPool;

use crate::db::models::execution_control_events::{
    ExecutionControlEventModel, NewExecutionControlEventModel,
};

pub async fn insert_execution_control_event(
    pool: &PgPool,
    event: &NewExecutionControlEventModel,
) -> Result<i32, sqlx::Error> {
    let row = sqlx::query!(
        r#"
        INSERT INTO execution_control_events (
            timestamp,
            event_type,
            mode,
            target_stable_symbol,
            min_strategy_run_id_to_execute,
            updated_by,
            reason
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id
        "#,
        event.timestamp,
        event.event_type,
        event.mode,
        event.target_stable_symbol,
        event.min_strategy_run_id_to_execute,
        event.updated_by,
        event.reason,
    )
    .fetch_one(pool)
    .await?;

    Ok(row.id)
}

pub async fn get_recent_execution_control_events(
    pool: &PgPool,
    limit: i64,
) -> Result<Vec<ExecutionControlEventModel>, sqlx::Error> {
    sqlx::query_as!(
        ExecutionControlEventModel,
        r#"
        SELECT
            id,
            timestamp,
            event_type,
            mode,
            target_stable_symbol,
            min_strategy_run_id_to_execute,
            updated_by,
            reason
        FROM execution_control_events
        ORDER BY timestamp DESC
        LIMIT $1
        "#,
        limit
    )
    .fetch_all(pool)
    .await
}
