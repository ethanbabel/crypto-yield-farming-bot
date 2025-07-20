use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use chrono::{DateTime, Utc, Timelike};
use std::collections::BTreeMap;

use super::types::MarketStateSlice;
use super::strategy_constants::EWMA_ALPHA;

/// Returns expected return over the time horizon (as % of pool value)
pub fn simulate_fee_return(slice: &MarketStateSlice) -> Option<Decimal> {
    let hourly_fees = standardize_to_hourly(&slice.timestamps, &slice.fees_usd)?;

    let pool_value = slice.pool_long_collateral_usd + slice.pool_short_collateral_usd - slice.impact_pool_usd;

    if hourly_fees.is_empty() || pool_value <= Decimal::ZERO {
        return None;
    }

    let hourly_ewma = compute_ewma(&hourly_fees)?;
    let total_expected_fees = hourly_ewma;

    let expected_return = total_expected_fees / pool_value;

    // Scale
    let scale = Decimal::from_f64(12.0).unwrap();
    Some(expected_return / scale)
}

/// Aggregates ~5-min fee data into hourly fee buckets
fn standardize_to_hourly(timestamps: &[DateTime<Utc>], fees_usd: &[Decimal]) -> Option<Vec<Decimal>> {
    if timestamps.len() != fees_usd.len() || timestamps.is_empty() {
        return None;
    }

    let mut hourly_buckets: BTreeMap<DateTime<Utc>, Decimal> = BTreeMap::new();

    for (ts, fee) in timestamps.iter().zip(fees_usd.iter()) {
        // Truncate timestamp to the hour
        let hour_key = ts.with_minute(0).unwrap().with_second(0).unwrap().with_nanosecond(0).unwrap();
        *hourly_buckets.entry(hour_key).or_insert(Decimal::ZERO) += *fee;
    }

    Some(hourly_buckets.values().cloned().collect())
}

/// Standard EWMA computation over a Decimal vector
fn compute_ewma(values: &[Decimal]) -> Option<Decimal> {
    if values.is_empty() {
        return None;
    }

    let alpha = Decimal::from_f64(EWMA_ALPHA)?;
    let mut ewma = values[0];

    for value in &values[1..] {
        ewma = alpha * *value + (Decimal::ONE - alpha) * ewma;
    }

    Some(ewma)
}