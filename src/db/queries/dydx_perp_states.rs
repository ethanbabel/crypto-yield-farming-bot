use chrono::{DateTime, Utc};
use sqlx::PgPool;
use sqlx::Row;
use std::collections::HashMap;

use crate::db::models::dydx_perp_states::{DydxPerpStateModel, NewDydxPerpStateModel};

fn rows_to_dydx_perp_state_map(
    rows: Vec<sqlx::postgres::PgRow>,
) -> HashMap<i32, Vec<DydxPerpStateModel>> {
    let mut result = HashMap::new();
    for row in rows {
        let state = DydxPerpStateModel {
            id: row.get(0),
            dydx_perp_id: row.get(1),
            timestamp: row.get(2),
            funding_rate: row.get(3),
            initial_margin_fraction: row.get(4),
            maintenance_margin_fraction: row.get(5),
            oracle_price: row.get(6),
            open_interest: row.get(7),
        };
        result
            .entry(state.dydx_perp_id)
            .or_insert_with(Vec::new)
            .push(state);
    }
    result
}

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

pub async fn get_dydx_perp_states_in_range_for_perps_exclusive_start(
    pool: &PgPool,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    perp_ids: &[i32],
) -> Result<HashMap<i32, Vec<DydxPerpStateModel>>, sqlx::Error> {
    if perp_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let rows = sqlx::query(
        r#"
        SELECT id, dydx_perp_id, timestamp, funding_rate, initial_margin_fraction,
               maintenance_margin_fraction, oracle_price, open_interest
        FROM dydx_perp_states
        WHERE dydx_perp_id = ANY($1)
          AND timestamp > $2
          AND timestamp <= $3
        ORDER BY dydx_perp_id ASC, timestamp ASC
        "#,
    )
    .bind(perp_ids)
    .bind(start)
    .bind(end)
    .fetch_all(pool)
    .await?;

    Ok(rows_to_dydx_perp_state_map(rows))
}

pub async fn get_latest_dydx_perp_states_at_or_before_for_perps(
    pool: &PgPool,
    timestamp: DateTime<Utc>,
    perp_ids: &[i32],
) -> Result<HashMap<i32, DydxPerpStateModel>, sqlx::Error> {
    if perp_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let rows = sqlx::query(
        r#"
        SELECT
            ps.id, ps.dydx_perp_id, ps.timestamp, ps.funding_rate, ps.initial_margin_fraction,
            ps.maintenance_margin_fraction, ps.oracle_price, ps.open_interest
        FROM unnest($1::int[]) AS ids(dydx_perp_id)
        JOIN LATERAL (
            SELECT
                id, dydx_perp_id, timestamp, funding_rate, initial_margin_fraction,
                maintenance_margin_fraction, oracle_price, open_interest
            FROM dydx_perp_states
            WHERE dydx_perp_id = ids.dydx_perp_id
              AND timestamp <= $2
            ORDER BY timestamp DESC
            LIMIT 1
        ) ps ON true
        ORDER BY ps.dydx_perp_id
        "#,
    )
    .bind(perp_ids)
    .bind(timestamp)
    .fetch_all(pool)
    .await?;

    let grouped = rows_to_dydx_perp_state_map(rows);
    let mut out = HashMap::new();
    for (perp_id, mut states) in grouped {
        if let Some(state) = states.pop() {
            out.insert(perp_id, state);
        }
    }

    Ok(out)
}
