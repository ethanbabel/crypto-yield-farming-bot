use sqlx::PgPool;

use crate::db::models::portfolio_snapshots::{NewPortfolioSnapshotModel, PortfolioSnapshotModel};

pub async fn insert_portfolio_snapshot(
    pool: &PgPool,
    snapshot: &NewPortfolioSnapshotModel,
) -> Result<i32, sqlx::Error> {
    let row = sqlx::query!(
        r#"
        INSERT INTO portfolio_snapshots (
            timestamp, total_value_usd, market_value_usd, asset_value_usd, hedge_value_usd, pnl_usd
        )
        VALUES ($1,$2,$3,$4,$5,$6)
        RETURNING id
        "#,
        snapshot.timestamp,
        snapshot.total_value_usd,
        snapshot.market_value_usd,
        snapshot.asset_value_usd,
        snapshot.hedge_value_usd,
        snapshot.pnl_usd,
    )
    .fetch_one(pool)
    .await?;

    Ok(row.id)
}

pub async fn get_latest_portfolio_snapshot(
    pool: &PgPool,
) -> Result<Option<PortfolioSnapshotModel>, sqlx::Error> {
    sqlx::query_as!(
        PortfolioSnapshotModel,
        r#"
        SELECT id, timestamp, total_value_usd, market_value_usd, asset_value_usd, hedge_value_usd, pnl_usd
        FROM portfolio_snapshots
        ORDER BY timestamp DESC
        LIMIT 1
        "#
    )
    .fetch_optional(pool)
    .await
}
