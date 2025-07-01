use rust_decimal::Decimal;
use sqlx::FromRow;
use chrono::{DateTime, Utc};
use ethers::types::Address;
use std::collections::HashMap;
use serde::{Serialize, Deserialize};

use crate::data_ingestion::market::market::Market;

#[derive(Debug, FromRow)]
pub struct MarketStateModel {
    pub id: i32,
    pub market_id: i32,
    pub timestamp: DateTime<Utc>,
    pub borrowing_factor_long: Option<Decimal>,
    pub borrowing_factor_short: Option<Decimal>,
    pub pnl_long: Option<Decimal>,
    pub pnl_short: Option<Decimal>,
    pub pnl_net: Option<Decimal>,
    pub gm_price_min: Option<Decimal>,
    pub gm_price_max: Option<Decimal>,
    pub gm_price_mid: Option<Decimal>,
    pub pool_long_amount: Option<Decimal>,
    pub pool_short_amount: Option<Decimal>,
    pub pool_long_token_usd: Option<Decimal>,
    pub pool_short_token_usd: Option<Decimal>,
    pub open_interest_long: Option<Decimal>,
    pub open_interest_short: Option<Decimal>,
    pub open_interest_long_via_tokens: Option<Decimal>,
    pub open_interest_short_via_tokens: Option<Decimal>,
    pub utilization: Option<Decimal>,
    pub swap_volume: Option<Decimal>,
    pub trading_volume: Option<Decimal>,
    pub fees_position: Option<Decimal>,
    pub fees_liquidation: Option<Decimal>,
    pub fees_swap: Option<Decimal>,
    pub fees_borrowing: Option<Decimal>,
    pub fees_total: Option<Decimal>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct NewMarketStateModel {
    pub market_id: i32,
    pub timestamp: DateTime<Utc>,
    pub borrowing_factor_long: Option<Decimal>,
    pub borrowing_factor_short: Option<Decimal>,
    pub pnl_long: Option<Decimal>,
    pub pnl_short: Option<Decimal>,
    pub pnl_net: Option<Decimal>,
    pub gm_price_min: Option<Decimal>,
    pub gm_price_max: Option<Decimal>,
    pub gm_price_mid: Option<Decimal>,
    pub pool_long_amount: Option<Decimal>,
    pub pool_short_amount: Option<Decimal>,
    pub pool_long_token_usd: Option<Decimal>,
    pub pool_short_token_usd: Option<Decimal>,
    pub open_interest_long: Option<Decimal>,
    pub open_interest_short: Option<Decimal>,
    pub open_interest_long_via_tokens: Option<Decimal>,
    pub open_interest_short_via_tokens: Option<Decimal>,
    pub utilization: Option<Decimal>,
    pub swap_volume: Option<Decimal>,
    pub trading_volume: Option<Decimal>,
    pub fees_position: Option<Decimal>,
    pub fees_liquidation: Option<Decimal>,
    pub fees_swap: Option<Decimal>,
    pub fees_borrowing: Option<Decimal>,
    pub fees_total: Option<Decimal>,
}

impl NewMarketStateModel {
    pub fn from(market: &Market, market_id_map: &HashMap<Address, i32>) -> Self {
        let market_id = market_id_map[&market.market_token];
        let timestamp: DateTime<Utc> = market.updated_at.unwrap().into();
        
        Self {
            market_id,
            timestamp,
            borrowing_factor_long: market.borrowing_factor_per_second.map(|bf| bf.longs),
            borrowing_factor_short: market.borrowing_factor_per_second.map(|bf| bf.shorts),
            pnl_long: market.pnl.map(|pnl| pnl.long),
            pnl_short: market.pnl.map(|pnl| pnl.short),
            pnl_net: market.pnl.map(|pnl| pnl.net),
            gm_price_min: market.gm_token_price.map(|price| price.min),
            gm_price_max: market.gm_token_price.map(|price| price.max),
            gm_price_mid: market.gm_token_price.map(|price| price.mid),
            pool_long_amount: market.token_pool.map(|pool| pool.long_token_amount),
            pool_short_amount: market.token_pool.map(|pool| pool.short_token_amount),
            pool_long_token_usd: market.token_pool.map(|pool| pool.long_token_usd),
            pool_short_token_usd: market.token_pool.map(|pool| pool.short_token_usd),
            open_interest_long: market.open_interest.map(|oi| oi.long),
            open_interest_short: market.open_interest.map(|oi| oi.short),
            open_interest_long_via_tokens: market.open_interest.map(|oi| oi.long_via_tokens),
            open_interest_short_via_tokens: market.open_interest.map(|oi| oi.short_via_tokens),
            utilization: market.current_utilization,
            swap_volume: Some(market.volume.swap),
            trading_volume: Some(market.volume.trading),
            fees_position: Some(market.cumulative_fees.position_fees),
            fees_liquidation: Some(market.cumulative_fees.liquidation_fees),
            fees_swap: Some(market.cumulative_fees.swap_fees),
            fees_borrowing: Some(market.cumulative_fees.borrowing_fees),
            fees_total: Some(market.cumulative_fees.total_fees),
        }
    }
}