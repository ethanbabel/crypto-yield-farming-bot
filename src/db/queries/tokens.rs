use sqlx::{PgPool, Error};
use std::collections::HashMap;
use ethers::types::Address;

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

/// Fetch all tokens from the database
pub async fn get_all_tokens(pool: &PgPool) -> Result<Vec<TokenModel>, Error> {
    sqlx::query_as!(
        TokenModel,
        "SELECT id, address, symbol, decimals FROM tokens"
    )
    .fetch_all(pool)
    .await
}
