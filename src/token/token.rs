use std::time::SystemTime;
use rust_decimal::Decimal;
use ethers::types::{Address, U256};

use super::oracle::Oracle;
use crate::gmx::reader_utils::PriceProps;

#[derive(Debug, Clone)]
pub struct AssetToken {
    pub symbol: String,
    pub address: Address,   // If netowrk mode=test this is testnet address, if network mode=prod this is mainnet address
    pub mainnet_address: Option<Address>, // If network mode=test this is the mainnet address, if network mode=prod this is None
    pub decimals: u8,
    pub is_synthetic: bool,
    pub oracle: Option<Oracle>,
    pub last_min_price: Option<U256>,
    pub last_max_price: Option<U256>,
    pub last_min_price_usd: Option<Decimal>,
    pub last_max_price_usd: Option<Decimal>,
    pub last_mid_price_usd: Option<Decimal>, 
    pub updated_at: Option<SystemTime>, // Timestamp of last price update
}

impl AssetToken {
    pub fn price_props(&self) -> Option<PriceProps> {
        Some(PriceProps {
            min: self.last_min_price?,
            max: self.last_max_price?,
        })
    }
}

