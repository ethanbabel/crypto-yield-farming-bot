use sqlx::PgPool;

use crate::db::models::trades::{NewTradeModel, TradeModel};

pub async fn insert_trade(pool: &PgPool, trade: &NewTradeModel) -> Result<i32, sqlx::Error> {
    let row = sqlx::query!(
        r#"
        INSERT INTO trades (
            timestamp, action_type, strategy_run_id, market_id, from_token_id, to_token_id,
            amount_in, amount_out, usd_value, fee_usd, tx_hash, status, details
        )
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
        RETURNING id
        "#,
        trade.timestamp,
        trade.action_type,
        trade.strategy_run_id,
        trade.market_id,
        trade.from_token_id,
        trade.to_token_id,
        trade.amount_in,
        trade.amount_out,
        trade.usd_value,
        trade.fee_usd,
        trade.tx_hash,
        trade.status,
        trade.details,
    )
    .fetch_one(pool)
    .await?;

    Ok(row.id)
}

pub async fn get_recent_trades(pool: &PgPool, limit: i64) -> Result<Vec<TradeModel>, sqlx::Error> {
    sqlx::query_as!(
        TradeModel,
        r#"
        SELECT id, timestamp, action_type, strategy_run_id, market_id, from_token_id, to_token_id,
               amount_in, amount_out, usd_value, fee_usd, tx_hash, status, details
        FROM trades
        ORDER BY timestamp DESC
        LIMIT $1
        "#,
        limit
    )
    .fetch_all(pool)
    .await
}
