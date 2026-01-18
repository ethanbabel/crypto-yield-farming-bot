use sqlx::{PgPool, Row};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use std::collections::HashMap;

use crate::db::models::token_prices::{NewTokenPriceModel, TokenPriceModel};

/// Insert a single token price record
pub async fn insert_token_price(
    pool: &PgPool,
    new_price: &NewTokenPriceModel,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO token_prices (token_id, timestamp, min_price, max_price, mid_price)
        VALUES ($1, $2, $3, $4, $5)
        "#,
        new_price.token_id,
        new_price.timestamp,
        new_price.min_price,
        new_price.max_price,
        new_price.mid_price,

    )
    .execute(pool)
    .await?;

    Ok(())
}

/// Fetch the token price for a specific token at a specific timestamp (exact match)
pub async fn get_token_price_at_timestamp(
    pool: &PgPool,
    token_id: i32,
    timestamp: DateTime<Utc>,
) -> Result<Option<TokenPriceModel>, sqlx::Error> {
    sqlx::query_as!(
        TokenPriceModel,
        r#"
        SELECT id, token_id, timestamp, min_price, max_price, mid_price
        FROM token_prices
        WHERE token_id = $1 AND timestamp = $2
        "#,
        token_id,
        timestamp
    )
    .fetch_optional(pool)
    .await
}

/// Fetch the first token price after a given timestamp
pub async fn get_token_price_after_timestamp(
    pool: &PgPool,
    token_id: i32,
    timestamp: DateTime<Utc>,
) -> Result<Option<TokenPriceModel>, sqlx::Error> {
    sqlx::query_as!(
        TokenPriceModel,
        r#"
        SELECT id, token_id, timestamp, min_price, max_price, mid_price
        FROM token_prices
        WHERE token_id = $1 AND timestamp > $2
        ORDER BY timestamp ASC
        LIMIT 1
        "#,
        token_id,
        timestamp
    )
    .fetch_optional(pool)
    .await
}

/// Fetch a token’s price history over a time range
pub async fn get_token_price_history_in_range(
    pool: &PgPool,
    token_id: i32,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<Vec<TokenPriceModel>, sqlx::Error> {
    sqlx::query_as!(
        TokenPriceModel,
        r#"
        SELECT id, token_id, timestamp, min_price, max_price, mid_price
        FROM token_prices
        WHERE token_id = $1
          AND timestamp >= $2
          AND timestamp <= $3
        ORDER BY timestamp ASC
        "#,
        token_id,
        start,
        end
    )
    .fetch_all(pool)
    .await
}

/// Fetch all token prices across all tokens in a time range
pub async fn get_all_token_prices_in_range(
    pool: &PgPool,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<HashMap<i32, Vec<TokenPriceModel>>, sqlx::Error> {
    // Use query() instead of query_as!() for binary protocol
    let rows = sqlx::query(
        r#"
        SELECT id, token_id, timestamp, min_price, max_price, mid_price
        FROM token_prices
        WHERE timestamp >= $1 AND timestamp <= $2
        ORDER BY token_id, timestamp
        "#
    )
    .bind(start)
    .bind(end)
    .fetch_all(pool)
    .await?;

    // Manual deserialization (faster than text protocol)
    let mut result = HashMap::new();
    for row in rows {
        let token_price = TokenPriceModel {
            id: row.get(0),
            token_id: row.get(1),
            timestamp: row.get(2),
            min_price: row.get(3),
            max_price: row.get(4),
            mid_price: row.get(5),
        };
        result.entry(token_price.token_id)
            .or_insert_with(Vec::new)
            .push(token_price);
    }
    Ok(result)
}

/// Fetch the latest token price for a specific token
pub async fn get_latest_token_price_for_token(pool: &PgPool, token_id: i32) -> Result<Option<TokenPriceModel>, sqlx::Error> {
    sqlx::query_as!(
        TokenPriceModel,
        r#"
        SELECT id, token_id, timestamp, min_price, max_price, mid_price
        FROM token_prices
        WHERE token_id = $1
        ORDER BY timestamp DESC
        LIMIT 1
        "#,
        token_id
    )
    .fetch_optional(pool)
    .await
}

/// Fetch the latest token prices for all tokens
pub async fn get_latest_token_prices_for_all_tokens(pool: &PgPool) -> Result<Vec<TokenPriceModel>, sqlx::Error> {
    sqlx::query_as!(
        TokenPriceModel,
        r#"
        SELECT DISTINCT ON (token_id) 
            id, token_id, timestamp, min_price, max_price, mid_price
        FROM token_prices
        ORDER BY token_id, timestamp DESC
        "#,
    )
    .fetch_all(pool)
    .await
}

/// Fetch all asset tokens
pub async fn get_all_asset_tokens(pool: &PgPool) -> Result<Vec<(String, String, u8, Decimal)>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT DISTINCT ON (tp.token_id)
            t.address, t.symbol, t.decimals, tp.mid_price
        FROM token_prices tp
        JOIN tokens t ON tp.token_id = t.id
        WHERE t.id IN (
            SELECT DISTINCT long_token_id FROM markets
            UNION
            SELECT DISTINCT short_token_id FROM markets
        )
        ORDER BY tp.token_id, tp.timestamp DESC
        "#,
    )
    .fetch_all(pool)
    .await?;
    let tokens = rows.into_iter()
        .map(|row| (row.address, row.symbol, row.decimals as u8, row.mid_price))
        .collect();
    Ok(tokens)
}

/// Fetch latest price props for a specific market
pub async fn get_latest_price_props_for_market(
    pool: &PgPool,
    market_id: i32,
) -> Result<Option<(Decimal, Decimal, Decimal, Decimal, Decimal, Decimal)>, sqlx::Error> {
    // Lateral joins to prevent explosive Cartesian products when joining
    let row = sqlx::query!(
        r#"
        SELECT 
            it.min_price AS index_min_price, it.max_price AS index_max_price,
            lt.min_price AS long_min_price, lt.max_price AS long_max_price,
            st.min_price AS short_min_price, st.max_price AS short_max_price
        FROM markets m
        LEFT JOIN LATERAL (
            SELECT min_price, max_price 
            FROM token_prices 
            WHERE token_id = m.index_token_id 
            ORDER BY timestamp DESC LIMIT 1
        ) it ON true
        LEFT JOIN LATERAL (
            SELECT min_price, max_price 
            FROM token_prices 
            WHERE token_id = m.long_token_id 
            ORDER BY timestamp DESC LIMIT 1
        ) lt ON true
        LEFT JOIN LATERAL (
            SELECT min_price, max_price 
            FROM token_prices 
            WHERE token_id = m.short_token_id 
            ORDER BY timestamp DESC LIMIT 1
        ) st ON true
        WHERE m.id = $1;
        "#,
        market_id
    )
    .fetch_optional(pool)
    .await?;

    if let Some(r) = row {
        Ok(Some((
            r.index_min_price,
            r.index_max_price,
            r.long_min_price,
            r.long_max_price,
            r.short_min_price,
            r.short_max_price,
        )))
    } else {
        Ok(None)
    }
}