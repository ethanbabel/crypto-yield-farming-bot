use sqlx::PgPool;

use crate::db::models::position_snapshots::{NewPositionSnapshotModel, PositionSnapshotModel};

pub async fn insert_position_snapshot(
    pool: &PgPool,
    position: &NewPositionSnapshotModel,
) -> Result<i32, sqlx::Error> {
    let row = sqlx::query!(
        r#"
        INSERT INTO position_snapshots (
            portfolio_snapshot_id, position_type, market_id, token_id, symbol, size, usd_value
        )
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        RETURNING id
        "#,
        position.portfolio_snapshot_id,
        position.position_type,
        position.market_id,
        position.token_id,
        position.symbol,
        position.size,
        position.usd_value,
    )
    .fetch_one(pool)
    .await?;

    Ok(row.id)
}

pub async fn get_positions_for_snapshot(
    pool: &PgPool,
    snapshot_id: i32,
) -> Result<Vec<PositionSnapshotModel>, sqlx::Error> {
    sqlx::query_as!(
        PositionSnapshotModel,
        r#"
        SELECT id, portfolio_snapshot_id, position_type, market_id, token_id, symbol, size, usd_value
        FROM position_snapshots
        WHERE portfolio_snapshot_id = $1
        ORDER BY id ASC
        "#,
        snapshot_id
    )
    .fetch_all(pool)
    .await
}
