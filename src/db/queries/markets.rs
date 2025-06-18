use sqlx::{PgPool, Error};
use std::collections::HashMap;
use ethers::types::Address;
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