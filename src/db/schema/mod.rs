use sqlx::{
    Executor,
    postgres::PgPool,
};
use eyre::Result;

pub async fn init_schema(pool: &PgPool) -> Result<(), sqlx::Error> {
    pool.execute(include_str!("tokens.sql")).await?;
    pool.execute(include_str!("markets.sql")).await?;
    pool.execute(include_str!("token_prices.sql")).await?;
    pool.execute(include_str!("market_states.sql")).await?;

    // Create indices on timestamp for performance
    sqlx::query(
        r#"
        CREATE INDEX IF NOT EXISTS idx_market_states_market_timestamp 
        ON market_states(market_id, timestamp);
        "#
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE INDEX IF NOT EXISTS idx_token_prices_token_timestamp 
        ON token_prices(token_id, timestamp);
        "#
    )
    .execute(pool)
    .await?;

    Ok(())
}