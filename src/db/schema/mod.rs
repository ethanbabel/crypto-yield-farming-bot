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
    Ok(())
}