use sqlx::PgPool;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use ethers::types::Address;
use rust_decimal::Decimal;

use crate::db::models::market_states::{NewMarketStateModel, MarketStateModel};

/// Insert a single market state record
pub async fn insert_market_state(
    pool: &PgPool,
    new_state: &NewMarketStateModel,
) -> Result<(), sqlx::Error> {
    sqlx::query!(
        r#"
        INSERT INTO market_states (
            market_id,
            timestamp,
            borrowing_factor_long,
            borrowing_factor_short,
            pnl_long,
            pnl_short,
            pnl_net,
            gm_price_min,
            gm_price_max,
            gm_price_mid,
            pool_long_amount,
            pool_short_amount,
            pool_impact_amount,
            pool_long_token_usd,
            pool_short_token_usd,
            pool_impact_token_usd,
            open_interest_long,
            open_interest_short,
            open_interest_long_amount,
            open_interest_short_amount,
            open_interest_long_via_tokens,
            open_interest_short_via_tokens,
            utilization,
            swap_volume,
            trading_volume,
            fees_position,
            fees_liquidation,
            fees_swap,
            fees_borrowing,
            fees_total
        )
        VALUES (
            $1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, 
            $14, $15, $16, $17, $18, $19, $20, $21, $22, $23, $24, 
            $25, $26, $27, $28, $29, $30
        )
        "#,
        new_state.market_id,
        new_state.timestamp,
        new_state.borrowing_factor_long,
        new_state.borrowing_factor_short,
        new_state.pnl_long,
        new_state.pnl_short,
        new_state.pnl_net,
        new_state.gm_price_min,
        new_state.gm_price_max,
        new_state.gm_price_mid,
        new_state.pool_long_amount,
        new_state.pool_short_amount,
        new_state.pool_impact_amount,
        new_state.pool_long_token_usd,
        new_state.pool_short_token_usd,
        new_state.pool_impact_token_usd,
        new_state.open_interest_long,
        new_state.open_interest_short,
        new_state.open_interest_long_amount,
        new_state.open_interest_short_amount,
        new_state.open_interest_long_via_tokens,
        new_state.open_interest_short_via_tokens,
        new_state.utilization,
        new_state.swap_volume,
        new_state.trading_volume,
        new_state.fees_position,
        new_state.fees_liquidation,
        new_state.fees_swap,
        new_state.fees_borrowing,
        new_state.fees_total
    )
    .execute(pool)
    .await?;

    Ok(())
}

/// Fetch market state for a specific market at a specific timestamp (exact match)
pub async fn get_market_state_at_timestamp(
    pool: &PgPool,
    market_id: i32,
    timestamp: DateTime<Utc>,
) -> Result<Option<MarketStateModel>, sqlx::Error> {
    sqlx::query_as!(
        MarketStateModel,
        r#"
        SELECT 
            id, market_id, timestamp, borrowing_factor_long, borrowing_factor_short, pnl_long,
            pnl_short, pnl_net, gm_price_min, gm_price_max, gm_price_mid, pool_long_amount,
            pool_short_amount, pool_impact_amount, pool_long_token_usd, pool_short_token_usd, 
            pool_impact_token_usd, open_interest_long, open_interest_short, open_interest_long_amount, 
            open_interest_short_amount, open_interest_long_via_tokens, open_interest_short_via_tokens, 
            utilization, swap_volume, trading_volume, fees_position, fees_liquidation, fees_swap, 
            fees_borrowing, fees_total
        FROM market_states
        WHERE market_id = $1 AND timestamp = $2
        "#,
        market_id,
        timestamp
    )
    .fetch_optional(pool)
    .await
}

/// Fetch the first market state after a given timestamp
pub async fn get_market_state_after_timestamp(
    pool: &PgPool,
    market_id: i32,
    timestamp: DateTime<Utc>,
) -> Result<Option<MarketStateModel>, sqlx::Error> {
    sqlx::query_as!(
        MarketStateModel,
        r#"
        SELECT 
            id, market_id, timestamp, borrowing_factor_long, borrowing_factor_short, pnl_long,
            pnl_short, pnl_net, gm_price_min, gm_price_max, gm_price_mid, pool_long_amount,
            pool_short_amount, pool_impact_amount, pool_long_token_usd, pool_short_token_usd, 
            pool_impact_token_usd, open_interest_long, open_interest_short, open_interest_long_amount, 
            open_interest_short_amount, open_interest_long_via_tokens, open_interest_short_via_tokens, 
            utilization, swap_volume, trading_volume, fees_position, fees_liquidation, fees_swap, 
            fees_borrowing, fees_total
        FROM market_states
        WHERE market_id = $1 AND timestamp > $2
        ORDER BY timestamp ASC
        LIMIT 1
        "#,
        market_id,
        timestamp
    )
    .fetch_optional(pool)
    .await
}

/// Fetch all market states for a market over a time range
pub async fn get_market_state_history_in_range(
    pool: &PgPool,
    market_id: i32,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<Vec<MarketStateModel>, sqlx::Error> {
    sqlx::query_as!(
        MarketStateModel,
        r#"
        SELECT 
            id, market_id, timestamp, borrowing_factor_long, borrowing_factor_short, pnl_long,
            pnl_short, pnl_net, gm_price_min, gm_price_max, gm_price_mid, pool_long_amount,
            pool_short_amount, pool_impact_amount, pool_long_token_usd, pool_short_token_usd, 
            pool_impact_token_usd, open_interest_long, open_interest_short, open_interest_long_amount, 
            open_interest_short_amount, open_interest_long_via_tokens, open_interest_short_via_tokens, 
            utilization, swap_volume, trading_volume, fees_position, fees_liquidation, fees_swap, 
            fees_borrowing, fees_total
        FROM market_states
        WHERE market_id = $1 AND timestamp >= $2 AND timestamp <= $3
        ORDER BY timestamp ASC
        "#,
        market_id,
        start,
        end
    )
    .fetch_all(pool)
    .await
}

/// Fetch all market states across all markets in a time range
pub async fn get_all_market_states_in_range(
    pool: &PgPool,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> Result<Vec<MarketStateModel>, sqlx::Error> {
    sqlx::query_as!(
        MarketStateModel,
        r#"
        SELECT 
            id, market_id, timestamp, borrowing_factor_long, borrowing_factor_short, pnl_long,
            pnl_short, pnl_net, gm_price_min, gm_price_max, gm_price_mid, pool_long_amount,
            pool_short_amount, pool_impact_amount, pool_long_token_usd, pool_short_token_usd, 
            pool_impact_token_usd, open_interest_long, open_interest_short, open_interest_long_amount, 
            open_interest_short_amount, open_interest_long_via_tokens, open_interest_short_via_tokens, 
            utilization, swap_volume, trading_volume, fees_position, fees_liquidation, fees_swap, 
            fees_borrowing, fees_total
        FROM market_states
        WHERE timestamp >= $1 AND timestamp <= $2
        ORDER BY market_id, timestamp
        "#,
        start,
        end
    )
    .fetch_all(pool)
    .await
}

/// Get market display names by joining markets and tokens tables
pub async fn get_market_display_names(pool: &PgPool) -> Result<HashMap<Address, String>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT 
            m.address as market_address,
            it.symbol as index_token_symbol,
            lt.symbol as long_token_symbol,
            st.symbol as short_token_symbol
        FROM markets m
        JOIN tokens it ON m.index_token_id = it.id
        JOIN tokens lt ON m.long_token_id = lt.id
        JOIN tokens st ON m.short_token_id = st.id
        "#
    )
    .fetch_all(pool)
    .await?;

    let display_names = rows.into_iter()
        .filter_map(|row| {
            let addr = row.market_address.parse::<Address>().ok()?;
            let display_name = format!(
                "{}/USD [{} - {}]", 
                row.index_token_symbol, 
                row.long_token_symbol, 
                row.short_token_symbol
            );
            Some((addr, display_name))
        })
        .collect();

    Ok(display_names)
}

/// Fetch latest market state for a specific market
pub async fn get_latest_market_state_for_market(pool: &PgPool, market_id: i32) -> Result<Option<MarketStateModel>, sqlx::Error> {
    sqlx::query_as!(
        MarketStateModel,
        r#"
        SELECT 
            id, market_id, timestamp, borrowing_factor_long, borrowing_factor_short, pnl_long,
            pnl_short, pnl_net, gm_price_min, gm_price_max, gm_price_mid, pool_long_amount,
            pool_short_amount, pool_impact_amount, pool_long_token_usd, pool_short_token_usd, 
            pool_impact_token_usd, open_interest_long, open_interest_short, open_interest_long_amount, 
            open_interest_short_amount, open_interest_long_via_tokens, open_interest_short_via_tokens, 
            utilization, swap_volume, trading_volume, fees_position, fees_liquidation, fees_swap, 
            fees_borrowing, fees_total
        FROM market_states 
        WHERE market_id = $1
        ORDER BY timestamp DESC
        LIMIT 1
        "#,
        market_id
    )
    .fetch_optional(pool)
    .await
}

/// Fetch latest market state for all markets
pub async fn get_latest_market_states_for_all_markets(pool: &PgPool) -> Result<Vec<MarketStateModel>, sqlx::Error> {
    sqlx::query_as!(
        MarketStateModel,
        r#"
        SELECT DISTINCT ON (market_id)
            id, market_id, timestamp, borrowing_factor_long, borrowing_factor_short, pnl_long,
            pnl_short, pnl_net, gm_price_min, gm_price_max, gm_price_mid, pool_long_amount,
            pool_short_amount, pool_impact_amount, pool_long_token_usd, pool_short_token_usd, 
            pool_impact_token_usd, open_interest_long, open_interest_short, open_interest_long_amount, 
            open_interest_short_amount, open_interest_long_via_tokens, open_interest_short_via_tokens, 
            utilization, swap_volume, trading_volume, fees_position, fees_liquidation, fees_swap, 
            fees_borrowing, fees_total
        FROM market_states
        ORDER BY market_id, timestamp DESC
        "#
    )
    .fetch_all(pool)
    .await
}

/// Fetch all market tokens
pub async fn get_all_market_tokens(pool: &PgPool) -> Result<Vec<(String, String, Decimal, String, String, String)>, sqlx::Error> {
    let rows = sqlx::query!(
        r#"
        SELECT DISTINCT ON (ms.market_id)
            m.address AS market_address,
            it.symbol AS index_token_symbol,
            lt.symbol AS long_token_symbol,
            st.symbol AS short_token_symbol,
            ms.gm_price_mid AS last_mid_price_usd,
            it.address AS index_token_address,
            lt.address AS long_token_address,
            st.address AS short_token_address
        FROM market_states ms
        JOIN markets m ON ms.market_id = m.id
        JOIN tokens it ON m.index_token_id = it.id
        JOIN tokens lt ON m.long_token_id = lt.id
        JOIN tokens st ON m.short_token_id = st.id
        ORDER BY ms.market_id, ms.timestamp DESC
        "#
    )
    .fetch_all(pool)
    .await?;

    let market_tokens = rows.into_iter()
        .filter_map(|row| {
            if let Some(last_mid_price_usd) = row.last_mid_price_usd {
                Some((
                    row.market_address,
                    format!("GM_{}/USD_[{}-{}]", row.index_token_symbol, row.long_token_symbol, row.short_token_symbol),
                    last_mid_price_usd,
                    row.index_token_address,
                    row.long_token_address,
                    row.short_token_address,
                ))
            } else {
                None
            }
        })
        .collect();
    Ok(market_tokens)
}
