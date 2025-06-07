use ethers::types::U256;
use ethers::utils;
use rust_decimal::prelude::*;
use rust_decimal::Decimal;

use crate::gmx::gmx_reader_structs::{MarketInfo, MarketPoolValueInfoProps};
use crate::constants::GMX_DECIMALS;

pub fn calculate_apr(market_info: &MarketInfo, pool_info: &MarketPoolValueInfoProps) -> Option<Decimal> {
    // Convert U256 to Decimal using format_units and parsing
    fn u256_to_decimal_scaled(val: U256) -> Decimal {
        let formatted = utils::format_units(val, GMX_DECIMALS as usize).unwrap_or_else(|_| "0".to_string());
        Decimal::from_str(&formatted).unwrap_or(Decimal::ZERO)
    }
    let borrowing_fee_rate_longs = u256_to_decimal_scaled(market_info.borrowing_factor_per_second_for_longs);
    let borrowing_fee_rate_shorts = u256_to_decimal_scaled(market_info.borrowing_factor_per_second_for_shorts);
    let long_usd_value = u256_to_decimal_scaled(pool_info.long_token_usd);
    let short_usd_value = u256_to_decimal_scaled(pool_info.short_token_usd);
    let lp_borrowing_fee_pool_rate = u256_to_decimal_scaled(pool_info.borrowing_fee_pool_factor);

    let total_borrowing_fees_per_second = (long_usd_value * borrowing_fee_rate_longs)
        + (short_usd_value * borrowing_fee_rate_shorts);
    let lp_income_per_second = total_borrowing_fees_per_second * lp_borrowing_fee_pool_rate;
    let total_pool_value = long_usd_value + short_usd_value;

    if total_pool_value.is_zero() {
        return None;
    }

    let seconds_per_year = Decimal::from(60 * 60 * 24 * 365);
    let apr = (lp_income_per_second / total_pool_value) * seconds_per_year;
    Some(apr)
}