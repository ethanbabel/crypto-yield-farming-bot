use ethers::types::{I256, U256};
use rust_decimal::prelude::*;
use rust_decimal::Decimal;

use crate::constants::GMX_DECIMALS;

// --- HELPER FUNCTIONS ---
pub fn i256_to_decimal_scaled(val: I256) -> Decimal {
    let formatted = ethers::utils::format_units(val, GMX_DECIMALS as usize).unwrap_or_else(|_| {
        tracing::warn!("Failed to format I256 value: {}", val);
        "0".to_string()
    });
    Decimal::from_str(&formatted).unwrap_or(Decimal::ZERO)
}

pub fn i256_to_decimal_scaled_decimals(val: I256, decimals: u8) -> Decimal {
    let formatted = ethers::utils::format_units(val, decimals as usize).unwrap_or_else(|_| {
        tracing::warn!("Failed to format I256 value: {}", val);
        "0".to_string()
    });
    Decimal::from_str(&formatted).unwrap_or(Decimal::ZERO)
}

pub fn u256_to_decimal_scaled(val: U256) -> Decimal {
    let formatted = ethers::utils::format_units(val, GMX_DECIMALS as usize).unwrap_or_else(|_| {
        tracing::warn!("Failed to format U256 value: {}", val);
        "0".to_string()
    });
    Decimal::from_str(&formatted).unwrap_or(Decimal::ZERO)
}

pub fn u256_to_decimal_scaled_decimals(val: U256, decimals: u8) -> Decimal {
    let formatted = ethers::utils::format_units(val, decimals as usize).unwrap_or_else(|_| {
        tracing::warn!("Failed to format U256 value: {}", val);
        "0".to_string()
    });
    Decimal::from_str(&formatted).unwrap_or(Decimal::ZERO)
}

// --- Structs for GMX Market Data ---
#[derive(Debug, Clone)]
pub struct BorrowingFactorPerSecond {
    pub longs: Decimal,
    pub shorts: Decimal,
}

#[derive(Debug, Clone)]
pub struct Pnl {
    pub long: Decimal,
    pub short: Decimal,
    pub net: Decimal,
}

#[derive(Debug, Clone)]
pub struct TokenPool {
    pub long_token_amount: Decimal,
    pub short_token_amount: Decimal,
    pub long_token_usd: Decimal,
    pub short_token_usd: Decimal,
}

#[derive(Debug, Clone)]
pub struct GmTokenPrice {
    pub min: Decimal,
    pub max: Decimal,
    pub mid: Decimal,
}

#[derive(Debug, Clone)]
pub struct OpenInterest {
    pub long: Decimal,
    pub short: Decimal,
    pub long_via_tokens: Decimal,
    pub short_via_tokens: Decimal,
}

#[derive(Debug, Clone)]
pub struct Volume {
    pub trading: Decimal,
    pub swap: Decimal,
}

#[derive(Debug, Clone)]
pub struct CumulativeFees {
    pub position_fees: Decimal,
    pub liquidation_fees: Decimal,
    pub swap_fees: Decimal,
    pub borrowing_fees: Decimal,
    pub total_fees: Decimal,
}

impl CumulativeFees {
    pub fn new() -> Self {
        Self {
            position_fees: Decimal::ZERO,
            liquidation_fees: Decimal::ZERO,
            swap_fees: Decimal::ZERO,
            borrowing_fees: Decimal::ZERO,
            total_fees: Decimal::ZERO,
        }
    }
}
