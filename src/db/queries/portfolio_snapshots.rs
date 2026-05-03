use sqlx::{PgPool, Row};

use crate::db::models::portfolio_snapshots::{NewPortfolioSnapshotModel, PortfolioSnapshotModel};

pub async fn insert_portfolio_snapshot(
    pool: &PgPool,
    snapshot: &NewPortfolioSnapshotModel,
) -> Result<i32, sqlx::Error> {
    let row = sqlx::query(
        r#"
        INSERT INTO portfolio_snapshots (
            strategy_run_id,
            timestamp,
            total_value_usd,
            arbitrum_value_usd,
            market_value_usd,
            asset_value_usd,
            native_balance,
            native_value_usd,
            dydx_main_usdc,
            dydx_subaccount_equity,
            dydx_free_collateral,
            pnl_usd
        )
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
        RETURNING id
        "#,
    )
    .bind(snapshot.strategy_run_id)
    .bind(snapshot.timestamp)
    .bind(snapshot.total_value_usd)
    .bind(snapshot.arbitrum_value_usd)
    .bind(snapshot.market_value_usd)
    .bind(snapshot.asset_value_usd)
    .bind(snapshot.native_balance)
    .bind(snapshot.native_value_usd)
    .bind(snapshot.dydx_main_usdc)
    .bind(snapshot.dydx_subaccount_equity)
    .bind(snapshot.dydx_free_collateral)
    .bind(snapshot.pnl_usd)
    .fetch_one(pool)
    .await?;

    Ok(row.get("id"))
}

pub async fn get_latest_portfolio_snapshot(
    pool: &PgPool,
) -> Result<Option<PortfolioSnapshotModel>, sqlx::Error> {
    sqlx::query_as::<_, PortfolioSnapshotModel>(
        r#"
        SELECT
            id,
            strategy_run_id,
            timestamp,
            total_value_usd,
            arbitrum_value_usd,
            market_value_usd,
            asset_value_usd,
            native_balance,
            native_value_usd,
            dydx_main_usdc,
            dydx_subaccount_equity,
            dydx_free_collateral,
            pnl_usd
        FROM portfolio_snapshots
        ORDER BY timestamp DESC
        LIMIT 1
        "#,
    )
    .fetch_optional(pool)
    .await
}
