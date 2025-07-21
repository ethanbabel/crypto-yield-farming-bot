use sqlx::PgPool;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

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