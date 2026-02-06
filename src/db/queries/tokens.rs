use ethers::types::Address;
use sqlx::{Error, PgPool, Row};
use std::collections::HashMap;

use crate::db::models::tokens::{TokenModel, NewTokenModel};

/// Fetch a token by its database ID
pub async fn get_token_by_id(pool: &PgPool, id: i32) -> Result<Option<TokenModel>, Error> {
    sqlx::query_as!(
        TokenModel,
        "SELECT id, address, symbol, decimals FROM tokens WHERE id = $1",
        id
    )
    .fetch_optional(pool)
    .await
}

/// Fetch a token by its address string
pub async fn get_token_by_address(pool: &PgPool, address: &str) -> Result<Option<TokenModel>, Error> {
    sqlx::query_as!(
        TokenModel,
        "SELECT id, address, symbol, decimals FROM tokens WHERE address = $1",
        address
    )
    .fetch_optional(pool)
    .await
}

/// Insert a single token into the database if not already present
pub async fn insert_token(pool: &PgPool, token: &NewTokenModel) -> Result<i32, sqlx::Error> {
    let row = sqlx::query!(
        r#"
        INSERT INTO tokens (address, symbol, decimals)
        VALUES ($1, $2, $3)
        ON CONFLICT (address) DO UPDATE SET address = EXCLUDED.address
        RETURNING id
        "#,
        token.address,
        token.symbol,
        token.decimals
    )
    .fetch_one(pool)
    .await?;

    Ok(row.id)
}

/// Load a map from token address to token ID
pub async fn get_token_id_map(pool: &PgPool) -> Result<HashMap<Address, i32>, Error> {
    let rows = sqlx::query!(
        "SELECT id, address FROM tokens"
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

/// Resolve a token ID by address using case-insensitive matching.
pub async fn get_token_id_by_address_insensitive(
    pool: &PgPool,
    address: &str,
) -> Result<Option<i32>, Error> {
    let row = sqlx::query(
        r#"
        SELECT id
        FROM tokens
        WHERE lower(address) = lower($1)
        LIMIT 1
        "#,
    )
    .bind(address)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|row| row.get(0)))
}

/// Load a map from token symbol to token ID
pub async fn get_token_symbol_map(pool: &PgPool) -> Result<HashMap<String, i32>, Error> {
    let rows = sqlx::query!(
        "SELECT id, symbol FROM tokens"
    )
    .fetch_all(pool)
    .await?;

    let map = rows.into_iter().map(|row| (row.symbol, row.id)).collect();
    Ok(map)
}

/// Resolve a token ID by symbol.
pub async fn get_token_id_by_symbol(pool: &PgPool, symbol: &str) -> Result<Option<i32>, Error> {
    let row = sqlx::query(
        r#"
        SELECT id
        FROM tokens
        WHERE symbol = $1
        LIMIT 1
        "#,
    )
    .bind(symbol)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|row| row.get(0)))
}

/// Fetch all tokens from the database
pub async fn get_all_tokens(pool: &PgPool) -> Result<Vec<TokenModel>, Error> {
    sqlx::query_as!(
        TokenModel,
        "SELECT id, address, symbol, decimals FROM tokens"
    )
    .fetch_all(pool)
    .await
}
