use sqlx::PgPool;

use crate::db::models::execution_control_state::{
    ExecutionControlStateModel, NewExecutionControlStateModel,
};

pub async fn get_execution_control_state(
    pool: &PgPool,
) -> Result<ExecutionControlStateModel, sqlx::Error> {
    sqlx::query_as::<_, ExecutionControlStateModel>(
        r#"
        SELECT
            singleton_id,
            mode,
            target_stable_symbol,
            min_strategy_run_id_to_execute,
            updated_at,
            updated_by,
            reason
        FROM execution_control_state
        WHERE singleton_id = TRUE
        "#,
    )
    .fetch_one(pool)
    .await
}

pub async fn upsert_execution_control_state(
    pool: &PgPool,
    state: &NewExecutionControlStateModel,
) -> Result<ExecutionControlStateModel, sqlx::Error> {
    sqlx::query_as::<_, ExecutionControlStateModel>(
        r#"
        INSERT INTO execution_control_state (
            singleton_id,
            mode,
            target_stable_symbol,
            min_strategy_run_id_to_execute,
            updated_at,
            updated_by,
            reason
        )
        VALUES (TRUE, $1, $2, $3, $4, $5, $6)
        ON CONFLICT (singleton_id) DO UPDATE
        SET
            mode = EXCLUDED.mode,
            target_stable_symbol = EXCLUDED.target_stable_symbol,
            min_strategy_run_id_to_execute = EXCLUDED.min_strategy_run_id_to_execute,
            updated_at = EXCLUDED.updated_at,
            updated_by = EXCLUDED.updated_by,
            reason = EXCLUDED.reason
        RETURNING
            singleton_id,
            mode,
            target_stable_symbol,
            min_strategy_run_id_to_execute,
            updated_at,
            updated_by,
            reason
        "#,
    )
    .bind(&state.mode)
    .bind(&state.target_stable_symbol)
    .bind(state.min_strategy_run_id_to_execute)
    .bind(state.updated_at)
    .bind(&state.updated_by)
    .bind(&state.reason)
    .fetch_one(pool)
    .await
}
