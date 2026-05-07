use sqlx::PgPool;

use crate::db::models::execution_transfer_state::{
    ExecutionTransferStateModel, NewExecutionTransferStateModel,
};

pub async fn get_execution_transfer_state(
    pool: &PgPool,
) -> Result<ExecutionTransferStateModel, sqlx::Error> {
    sqlx::query_as::<_, ExecutionTransferStateModel>(
        r#"
        SELECT
            singleton_id,
            direction,
            tx_hash,
            chain_id,
            amount_usd,
            expected_time_to_complete_secs,
            initiated_at
        FROM execution_transfer_state
        WHERE singleton_id = TRUE
        "#,
    )
    .fetch_one(pool)
    .await
}

pub async fn upsert_execution_transfer_state(
    pool: &PgPool,
    state: &NewExecutionTransferStateModel,
) -> Result<ExecutionTransferStateModel, sqlx::Error> {
    sqlx::query_as::<_, ExecutionTransferStateModel>(
        r#"
        INSERT INTO execution_transfer_state (
            singleton_id,
            direction,
            tx_hash,
            chain_id,
            amount_usd,
            expected_time_to_complete_secs,
            initiated_at
        )
        VALUES (TRUE, $1, $2, $3, $4, $5, $6)
        ON CONFLICT (singleton_id) DO UPDATE
        SET
            direction = EXCLUDED.direction,
            tx_hash = EXCLUDED.tx_hash,
            chain_id = EXCLUDED.chain_id,
            amount_usd = EXCLUDED.amount_usd,
            expected_time_to_complete_secs = EXCLUDED.expected_time_to_complete_secs,
            initiated_at = EXCLUDED.initiated_at
        RETURNING
            singleton_id,
            direction,
            tx_hash,
            chain_id,
            amount_usd,
            expected_time_to_complete_secs,
            initiated_at
        "#,
    )
    .bind(&state.direction)
    .bind(&state.tx_hash)
    .bind(&state.chain_id)
    .bind(state.amount_usd)
    .bind(state.expected_time_to_complete_secs)
    .bind(state.initiated_at)
    .fetch_one(pool)
    .await
}
