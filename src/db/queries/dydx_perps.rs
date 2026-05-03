use crate::db::models::dydx_perps::{DydxPerpModel, NewDydxPerpModel};
use sqlx::{Error, PgPool, Row};
use std::collections::HashMap;

pub async fn insert_dydx_perp(pool: &PgPool, perp: &NewDydxPerpModel) -> Result<i32, sqlx::Error> {
    let row = sqlx::query!(
        r#"
        INSERT INTO dydx_perps (token_id, ticker)
        VALUES ($1, $2)
        ON CONFLICT (ticker) DO UPDATE SET token_id = EXCLUDED.token_id
        RETURNING id
        "#,
        perp.token_id,
        perp.ticker,
    )
    .fetch_one(pool)
    .await?;

    Ok(row.id)
}

pub async fn get_dydx_perp_id_map(pool: &PgPool) -> Result<HashMap<String, i32>, Error> {
    let rows = sqlx::query!("SELECT id, ticker FROM dydx_perps")
        .fetch_all(pool)
        .await?;

    let map = rows
        .into_iter()
        .map(|row| (row.ticker, row.id))
        .collect();
    Ok(map)
}

/// Resolve a dYdX perp ID by ticker.
pub async fn get_dydx_perp_id_by_ticker(pool: &PgPool, ticker: &str) -> Result<Option<i32>, Error> {
    let row = sqlx::query(
        r#"
        SELECT id
        FROM dydx_perps
        WHERE ticker = $1
        LIMIT 1
        "#,
    )
    .bind(ticker)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|row| row.get(0)))
}

pub async fn get_all_dydx_perps(pool: &PgPool) -> Result<Vec<DydxPerpModel>, Error> {
    sqlx::query_as!(
        DydxPerpModel,
        "SELECT id, token_id, ticker FROM dydx_perps"
    )
    .fetch_all(pool)
    .await
}
