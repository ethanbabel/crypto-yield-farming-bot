use ethers::types::Address;
use crate::oracle::Oracle;

#[derive(Debug, Clone)]
pub struct AssetToken {
    pub symbol: String,
    pub address: Address,
    pub decimals: u8,
    pub is_synthetic: bool,
    pub oracle: Option<Oracle>,
    pub last_min_price: Option<f64>,
    pub last_max_price: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct GMToken {
    pub symbol: String,
    pub address: Address,
    pub decimals: u8,
    pub index_token: Address,
    pub long_token: Address,
    pub short_token: Address,
    pub last_min_price: Option<f64>,
    pub last_max_price: Option<f64>,
}
