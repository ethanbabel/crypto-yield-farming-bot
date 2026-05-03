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
    pool.execute(include_str!("strategy_runs.sql")).await?;
    pool.execute(include_str!("strategy_targets.sql")).await?;
    pool.execute(include_str!("trades.sql")).await?;
    pool.execute(include_str!("portfolio_snapshots.sql")).await?;
    pool.execute(include_str!("dydx_perps.sql")).await?;
    pool.execute(include_str!("dydx_perp_states.sql")).await?;
    pool.execute(include_str!("position_snapshots.sql")).await?;

    Ok(())
}
