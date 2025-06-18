use rust_decimal::Decimal;
use sqlx::FromRow;
use chrono::{DateTime, Utc};
use ethers::types::Address;
use std::collections::HashMap;

use crate::market::Market;

#[derive(Debug, FromRow)]
pub struct MarketStateModel {
    pub id: i32,
    pub market_id: i32,
    pub timestamp: DateTime<Utc>,
    pub borrowing_factor_long: Decimal,
    pub borrowing_factor_short: Decimal,
    pub pnl_long: Decimal,
    pub pnl_short: Decimal,
    pub pnl_net: Decimal,
    pub gm_price_min: Decimal,
    pub gm_price_max: Decimal,
    pub gm_price_mid: Decimal,
    pub pool_long_amount: Decimal,
    pub pool_short_amount: Decimal,
    pub pool_long_token_usd: Decimal,
    pub pool_short_token_usd: Decimal,
    pub open_interest_long: Decimal,
    pub open_interest_short: Decimal,
    pub open_interest_long_via_tokens: Decimal,
    pub open_interest_short_via_tokens: Decimal,
    pub utilization: Decimal,
    pub swap_volume: Decimal,
    pub trading_volume: Decimal,
    pub fees_position: Decimal,
    pub fees_liquidation: Decimal,
    pub fees_swap: Decimal,
    pub fees_borrowing: Decimal,
    pub fees_total: Decimal,
}

#[derive(Debug)]
pub struct NewMarketStateModel {
    pub market_id: i32,
    pub timestamp: DateTime<Utc>,
    pub borrowing_factor_long: Decimal,
    pub borrowing_factor_short: Decimal,
    pub pnl_long: Decimal,
    pub pnl_short: Decimal,
    pub pnl_net: Decimal,
    pub gm_price_min: Decimal,
    pub gm_price_max: Decimal,
    pub gm_price_mid: Decimal,
    pub pool_long_amount: Decimal,
    pub pool_short_amount: Decimal,
    pub pool_long_token_usd: Decimal,
    pub pool_short_token_usd: Decimal,
    pub open_interest_long: Decimal,
    pub open_interest_short: Decimal,
    pub open_interest_long_via_tokens: Decimal,
    pub open_interest_short_via_tokens: Decimal,
    pub utilization: Decimal,
    pub swap_volume: Decimal,
    pub trading_volume: Decimal,
    pub fees_position: Decimal,
    pub fees_liquidation: Decimal,
    pub fees_swap: Decimal,
    pub fees_borrowing: Decimal,
    pub fees_total: Decimal,
}

impl NewMarketStateModel {
    pub fn from(market: &Market, market_id_map: &HashMap<Address, i32>) -> Self {
        let market_id = market_id_map[&market.market_token];
        let timestamp: DateTime<Utc> = market.updated_at.unwrap().into();
        
        Self {
            market_id,
            timestamp,
            borrowing_factor_long: market.borrowing_factor_per_second.unwrap().longs,
            borrowing_factor_short: market.borrowing_factor_per_second.unwrap().shorts,
            pnl_long: market.pnl.unwrap().long,
            pnl_short: market.pnl.unwrap().short,
            pnl_net: market.pnl.unwrap().net,
            gm_price_min: market.gm_token_price.unwrap().min,
            gm_price_max: market.gm_token_price.unwrap().max,
            gm_price_mid: market.gm_token_price.unwrap().mid,
            pool_long_amount: market.token_pool.unwrap().long_token_amount,
            pool_short_amount: market.token_pool.unwrap().short_token_amount,
            pool_long_token_usd: market.token_pool.unwrap().long_token_usd,
            pool_short_token_usd: market.token_pool.unwrap().short_token_usd,
            open_interest_long: market.open_interest.unwrap().long,
            open_interest_short: market.open_interest.unwrap().short,
            open_interest_long_via_tokens: market.open_interest.unwrap().long_via_tokens,
            open_interest_short_via_tokens: market.open_interest.unwrap().short_via_tokens,
            utilization: market.current_utilization.unwrap(),
            swap_volume: market.volume.swap,
            trading_volume: market.volume.trading,
            fees_position: market.cumulative_fees.position_fees,
            fees_liquidation: market.cumulative_fees.liquidation_fees,
            fees_swap: market.cumulative_fees.swap_fees,
            fees_borrowing: market.cumulative_fees.borrowing_fees,
            fees_total: market.cumulative_fees.total_fees,
        }
    }
}