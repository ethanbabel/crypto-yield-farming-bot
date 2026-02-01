use sqlx::PgPool;

use crate::db::models::dydx_perp_states::{DydxPerpStateModel, NewDydxPerpStateModel};

pub async fn insert_dydx_perp_state(pool: &PgPool, state: &NewDydxPerpStateModel) -> Result<i32, sqlx::Error> {
    let row = sqlx::query!(
        r#"
        INSERT INTO dydx_perp_states (
            dydx_perp_id, timestamp, funding_rate, initial_margin_fraction,
            maintenance_margin_fraction, oracle_price, open_interest
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id
        "#,
        state.dydx_perp_id,
        state.timestamp,
        state.funding_rate,
        state.initial_margin_fraction,
        state.maintenance_margin_fraction,
        state.oracle_price,
        state.open_interest,
    )
    .fetch_one(pool)
    .await?;

    Ok(row.id)
}

pub async fn get_dydx_perp_states_in_range(
    pool: &PgPool,
    dydx_perp_id: i32,
    start: chrono::DateTime<chrono::Utc>,
    end: chrono::DateTime<chrono::Utc>,
) -> Result<Vec<DydxPerpStateModel>, sqlx::Error> {
    sqlx::query_as!(
        DydxPerpStateModel,
        r#"
        SELECT id, dydx_perp_id, timestamp, funding_rate, initial_margin_fraction,
               maintenance_margin_fraction, oracle_price, open_interest
        FROM dydx_perp_states
        WHERE dydx_perp_id = $1 AND timestamp >= $2 AND timestamp <= $3
        ORDER BY timestamp ASC
        "#,
        dydx_perp_id,
        start,
        end
    )
    .fetch_all(pool)
    .await
}
