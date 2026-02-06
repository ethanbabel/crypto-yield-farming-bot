use chrono::{DateTime, Utc};
use ethers::types::Address;
use rust_decimal::Decimal;
use sqlx::{PgPool, Row};
use std::collections::HashMap;

use crate::db::models::market_states::{MarketStateModel, NewMarketStateModel};

fn rows_to_market_state_map(
    rows: Vec<sqlx::postgres::PgRow>,
) -> HashMap<i32, Vec<MarketStateModel>> {
    let mut result = HashMap::new();
    for row in rows {
        let state = MarketStateModel {
            id: row.get(0),
            market_id: row.get(1),
            timestamp: row.get(2),
            borrowing_factor_long: row.get(3),
            borrowing_factor_short: row.get(4),
            pnl_long: row.get(5),
            pnl_short: row.get(6),
            pnl_net: row.get(7),
            gm_price_min: row.get(8),
            gm_price_max: row.get(9),
            gm_price_mid: row.get(10),
            pool_long_amount: row.get(11),
            pool_short_amount: row.get(12),
            pool_impact_amount: row.get(13),
            pool_long_token_usd: row.get(14),
            pool_short_token_usd: row.get(15),
            pool_impact_token_usd: row.get(16),
            open_interest_long: row.get(17),
            open_interest_short: row.get(18),
            open_interest_long_amount: row.get(19),
            open_interest_short_amount: row.get(20),
            open_interest_long_via_tokens: row.get(21),
            open_interest_short_via_tokens: row.get(22),
            utilization: row.get(23),
            swap_volume: row.get(24),
            trading_volume: row.get(25),
            fees_position: row.get(26),
            fees_liquidation: row.get(27),
            fees_swap: row.get(28),
            fees_borrowing: row.get(29),
            fees_total: row.get(30),
        };
        result
            .entry(state.market_id)
            .or_insert_with(Vec::new)
            .push(state);
    }
    result
}

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
) -> Result<HashMap<i32, Vec<MarketStateModel>>, sqlx::Error> {
    // Use query() instead of query_as!() for binary protocol
    let rows = sqlx::query(
        r#"
        SELECT
            id, market_id, timestamp,
            borrowing_factor_long, borrowing_factor_short,
            pnl_long, pnl_short, pnl_net,
            gm_price_min, gm_price_max, gm_price_mid,
            pool_long_amount, pool_short_amount, pool_impact_amount,
            pool_long_token_usd, pool_short_token_usd, pool_impact_token_usd,
            open_interest_long, open_interest_short,
            open_interest_long_amount, open_interest_short_amount,
            open_interest_long_via_tokens, open_interest_short_via_tokens,
            utilization, swap_volume, trading_volume,
            fees_position, fees_liquidation, fees_swap, fees_borrowing, fees_total
        FROM market_states
        WHERE timestamp >= $1 AND timestamp <= $2
        ORDER BY market_id, timestamp
        "#,
    )
    .bind(start)
    .bind(end)
    .fetch_all(pool)
    .await?;

    Ok(rows_to_market_state_map(rows))
}

/// Fetch market states across selected markets in a time range
pub async fn get_market_states_in_range_for_markets(
    pool: &PgPool,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    market_ids: &[i32],
) -> Result<HashMap<i32, Vec<MarketStateModel>>, sqlx::Error> {
    if market_ids.is_empty() {
        return Ok(HashMap::new());
    }

    let rows = sqlx::query(
        r#"
        SELECT
            id, market_id, timestamp,
            borrowing_factor_long, borrowing_factor_short,
            pnl_long, pnl_short, pnl_net,
            gm_price_min, gm_price_max, gm_price_mid,
            pool_long_amount, pool_short_amount, pool_impact_amount,
            pool_long_token_usd, pool_short_token_usd, pool_impact_token_usd,
            open_interest_long, open_interest_short,
            open_interest_long_amount, open_interest_short_amount,
            open_interest_long_via_tokens, open_interest_short_via_tokens,
            utilization, swap_volume, trading_volume,
            fees_position, fees_liquidation, fees_swap, fees_borrowing, fees_total
        FROM market_states
        WHERE market_id = ANY($1)
          AND timestamp >= $2
          AND timestamp <= $3
        ORDER BY market_id, timestamp
        "#,
    )
    .bind(market_ids)
    .bind(start)
    .bind(end)
    .fetch_all(pool)
    .await?;

    Ok(rows_to_market_state_map(rows))
}

/// Get market display names by joining markets and tokens tables
pub async fn get_market_display_names(
    pool: &PgPool,
) -> Result<HashMap<Address, String>, sqlx::Error> {
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

    let display_names = rows
        .into_iter()
        .filter_map(|row| {
            let addr = row.market_address.parse::<Address>().ok()?;
            let display_name = format!(
                "{}/USD [{} - {}]",
                row.index_token_symbol, row.long_token_symbol, row.short_token_symbol
            );
            Some((addr, display_name))
        })
        .collect();

    Ok(display_names)
}

/// Fetch latest market state for a specific market
pub async fn get_latest_market_state_for_market(
    pool: &PgPool,
    market_id: i32,
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
pub async fn get_latest_market_states_for_all_markets(
    pool: &PgPool,
) -> Result<Vec<MarketStateModel>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
        SELECT
            ms.id, ms.market_id, ms.timestamp, ms.borrowing_factor_long, ms.borrowing_factor_short,
            ms.pnl_long, ms.pnl_short, ms.pnl_net, ms.gm_price_min, ms.gm_price_max, ms.gm_price_mid,
            ms.pool_long_amount, ms.pool_short_amount, ms.pool_impact_amount,
            ms.pool_long_token_usd, ms.pool_short_token_usd, ms.pool_impact_token_usd,
            ms.open_interest_long, ms.open_interest_short, ms.open_interest_long_amount,
            ms.open_interest_short_amount, ms.open_interest_long_via_tokens, ms.open_interest_short_via_tokens,
            ms.utilization, ms.swap_volume, ms.trading_volume, ms.fees_position, ms.fees_liquidation,
            ms.fees_swap, ms.fees_borrowing, ms.fees_total
        FROM markets m
        JOIN LATERAL (
            SELECT
                id, market_id, timestamp, borrowing_factor_long, borrowing_factor_short,
                pnl_long, pnl_short, pnl_net, gm_price_min, gm_price_max, gm_price_mid,
                pool_long_amount, pool_short_amount, pool_impact_amount,
                pool_long_token_usd, pool_short_token_usd, pool_impact_token_usd,
                open_interest_long, open_interest_short, open_interest_long_amount,
                open_interest_short_amount, open_interest_long_via_tokens, open_interest_short_via_tokens,
                utilization, swap_volume, trading_volume, fees_position, fees_liquidation,
                fees_swap, fees_borrowing, fees_total
            FROM market_states
            WHERE market_id = m.id
            ORDER BY timestamp DESC
            LIMIT 1
        ) ms ON true
        ORDER BY ms.market_id
        "#
    )
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| MarketStateModel {
            id: row.get(0),
            market_id: row.get(1),
            timestamp: row.get(2),
            borrowing_factor_long: row.get(3),
            borrowing_factor_short: row.get(4),
            pnl_long: row.get(5),
            pnl_short: row.get(6),
            pnl_net: row.get(7),
            gm_price_min: row.get(8),
            gm_price_max: row.get(9),
            gm_price_mid: row.get(10),
            pool_long_amount: row.get(11),
            pool_short_amount: row.get(12),
            pool_impact_amount: row.get(13),
            pool_long_token_usd: row.get(14),
            pool_short_token_usd: row.get(15),
            pool_impact_token_usd: row.get(16),
            open_interest_long: row.get(17),
            open_interest_short: row.get(18),
            open_interest_long_amount: row.get(19),
            open_interest_short_amount: row.get(20),
            open_interest_long_via_tokens: row.get(21),
            open_interest_short_via_tokens: row.get(22),
            utilization: row.get(23),
            swap_volume: row.get(24),
            trading_volume: row.get(25),
            fees_position: row.get(26),
            fees_liquidation: row.get(27),
            fees_swap: row.get(28),
            fees_borrowing: row.get(29),
            fees_total: row.get(30),
        })
        .collect())
}

/// Fetch all market tokens
pub async fn get_all_market_tokens(
    pool: &PgPool,
) -> Result<Vec<(String, String, Decimal, String, String, String)>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
        SELECT
            m.address AS market_address,
            it.symbol AS index_token_symbol,
            lt.symbol AS long_token_symbol,
            st.symbol AS short_token_symbol,
            ms.gm_price_mid AS last_mid_price_usd,
            it.address AS index_token_address,
            lt.address AS long_token_address,
            st.address AS short_token_address
        FROM markets m
        JOIN tokens it ON m.index_token_id = it.id
        JOIN tokens lt ON m.long_token_id = lt.id
        JOIN tokens st ON m.short_token_id = st.id
        JOIN LATERAL (
            SELECT gm_price_mid
            FROM market_states
            WHERE market_id = m.id
            ORDER BY timestamp DESC
            LIMIT 1
        ) ms ON true
        ORDER BY m.id
        "#,
    )
    .fetch_all(pool)
    .await?;

    let market_tokens = rows
        .into_iter()
        .filter_map(|row| {
            let last_mid_price_usd: Option<Decimal> = row.get(4);
            last_mid_price_usd.map(|last_mid_price_usd| {
                let market_address: String = row.get(0);
                let index_token_symbol: String = row.get(1);
                let long_token_symbol: String = row.get(2);
                let short_token_symbol: String = row.get(3);
                let index_token_address: String = row.get(5);
                let long_token_address: String = row.get(6);
                let short_token_address: String = row.get(7);
                (
                    market_address,
                    format!(
                        "GM_{}/USD_[{}-{}]",
                        index_token_symbol, long_token_symbol, short_token_symbol
                    ),
                    last_mid_price_usd,
                    index_token_address,
                    long_token_address,
                    short_token_address,
                )
            })
        })
        .collect();

    Ok(market_tokens)
}
