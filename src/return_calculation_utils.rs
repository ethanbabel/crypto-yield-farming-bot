use ethers::types::{I256, U256};
use rust_decimal::prelude::*;
use rust_decimal::Decimal;

use crate::gmx::reader_utils::{MarketInfo, MarketPoolValueInfoProps};
use crate::constants::GMX_DECIMALS;
use crate::market_utils::{
    i256_to_decimal_scaled,
    u256_to_decimal_scaled,
};

pub fn calculate_borrowing_apr(
    market_info: &MarketInfo, 
    pool_info: &MarketPoolValueInfoProps,
    long_open_interest: U256,
    short_open_interest: U256,
) -> Decimal {
    let borrowing_factor_per_second_longs_scaled = u256_to_decimal_scaled(market_info.borrowing_factor_per_second_for_longs);
    let borrowing_factor_per_second_shorts_scaled = u256_to_decimal_scaled(market_info.borrowing_factor_per_second_for_shorts);
    let long_open_interest_scaled = u256_to_decimal_scaled(long_open_interest);
    let short_open_interest_scaled = u256_to_decimal_scaled(short_open_interest);
    let pool_value_scaled = i256_to_decimal_scaled(pool_info.pool_value);
    let lp_borrowing_fee_pool_factor_scaled = u256_to_decimal_scaled(pool_info.borrowing_fee_pool_factor);

    let total_borrowing_fees_per_second = (borrowing_factor_per_second_longs_scaled * long_open_interest_scaled) +
        (borrowing_factor_per_second_shorts_scaled * short_open_interest_scaled);
    let lp_borrowing_income_per_second = total_borrowing_fees_per_second * lp_borrowing_fee_pool_factor_scaled;
    let lp_borrowing_income_per_year = lp_borrowing_income_per_second * Decimal::from(60 * 60 * 24 * 365);
    let current_borrowing_apr = if pool_value_scaled > Decimal::ZERO {
        lp_borrowing_income_per_year / pool_value_scaled
    } else {
        Decimal::ZERO
    };
    return current_borrowing_apr;
}