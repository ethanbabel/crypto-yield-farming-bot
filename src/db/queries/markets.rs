use ethers::types::Address;
use sqlx::{Error, PgPool, Row};
use std::collections::HashMap;
use crate::db::models::markets::{MarketModel, NewMarketModel};

/// Fetch a market by its database ID
pub async fn get_market_by_id(pool: &PgPool, id: i32) -> Result<Option<MarketModel>, Error> {
    sqlx::query_as!(
        MarketModel,
        "SELECT id, address, index_token_id, long_token_id, short_token_id FROM markets WHERE id = $1",
        id
    )
    .fetch_optional(pool)
    .await
}

/// Fetch a market by its address string
pub async fn get_market_by_address(pool: &PgPool, address: &str) -> Result<Option<MarketModel>, Error> {
    sqlx::query_as!(
        MarketModel,
        "SELECT id, address, index_token_id, long_token_id, short_token_id FROM markets WHERE address = $1",
        address
    )
    .fetch_optional(pool)
    .await
}

/// Insert a single market into the database if not already present
pub async fn insert_market(pool: &PgPool, market: &NewMarketModel) -> Result<i32, sqlx::Error> {
    let row = sqlx::query!(
        r#"
        INSERT INTO markets (address, index_token_id, long_token_id, short_token_id)
        VALUES ($1, $2, $3, $4)
        ON CONFLICT (address) DO UPDATE SET address = EXCLUDED.address
        RETURNING id
        "#,
        market.address,
        market.index_token_id,
        market.long_token_id,
        market.short_token_id
    )
    .fetch_one(pool)
    .await?;

    Ok(row.id)
}

/// Load a map from market address to market ID
pub async fn get_market_id_map(pool: &PgPool) -> Result<HashMap<Address, i32>, Error> {
    let rows = sqlx::query!(
        "SELECT id, address FROM markets"
    )
    .fetch_all(pool)
    .await?;

    let map = rows.into_iter()
        .filter_map(|row| {
            let addr = row.address.parse::<Address>().ok()?;
            Some((addr, row.id))
        })
        .collect();

    Ok(map)
}

/// Resolve a market ID by address using case-insensitive matching.
pub async fn get_market_id_by_address_insensitive(
    pool: &PgPool,
    address: &str,
) -> Result<Option<i32>, Error> {
    let row = sqlx::query(
        r#"
        SELECT id
        FROM markets
        WHERE lower(address) = lower($1)
        LIMIT 1
        "#,
    )
    .bind(address)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|row| row.get(0)))
}

/// Get the index token ID for a market
pub async fn get_market_index_token_id(pool: &PgPool, market_id: i32) -> Result<Option<i32>, Error> {
    let row = sqlx::query!(
        "SELECT index_token_id FROM markets WHERE id = $1",
        market_id
    )
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| r.index_token_id))
}

/// Fetch all markets from the database
pub async fn get_all_markets(pool: &PgPool) -> Result<Vec<MarketModel>, Error> {
    sqlx::query_as!(
        MarketModel,
        "SELECT id, address, index_token_id, long_token_id, short_token_id FROM markets"
    )
    .fetch_all(pool)
    .await
}

/// Get mapping of market_id to index_token_id for all markets
pub async fn get_all_market_index_tokens(pool: &PgPool) -> Result<HashMap<i32, i32>, Error> {
    let rows = sqlx::query!(
        r#"
        SELECT id, index_token_id
        FROM markets
        "#
    )
    .fetch_all(pool)
    .await?;

    let map = rows.into_iter()
        .map(|row| (row.id, row.index_token_id))
        .collect();
    
    Ok(map)
}
