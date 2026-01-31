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

    sqlx::query(
        r#"
        CREATE INDEX IF NOT EXISTS idx_portfolio_snapshots_timestamp
        ON portfolio_snapshots(timestamp);
        "#
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE INDEX IF NOT EXISTS idx_strategy_runs_timestamp
        ON strategy_runs(timestamp);
        "#
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE INDEX IF NOT EXISTS idx_strategy_targets_run
        ON strategy_targets(strategy_run_id);
        "#
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE INDEX IF NOT EXISTS idx_trades_timestamp
        ON trades(timestamp);
        "#
    )
    .execute(pool)
    .await?;


    Ok(())
}
