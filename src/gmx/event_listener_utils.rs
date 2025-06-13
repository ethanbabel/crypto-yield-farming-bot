use ethers::{
    prelude::*,
    types::{Address, U256, H256, TxHash},
    utils::{format_units, keccak256},
    abi::{encode, decode, Token},
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;

use crate::constants::GMX_DECIMALS;

// --- Helper functions ---
pub fn string_to_bytes32(s: &str) -> H256 {
    H256::from_slice(&keccak256(s.as_bytes()))
}

// --- Cumulative Fees Struct (for each market) ---
#[derive(Debug, Clone)]
pub struct MarketFees {
    /// Cumulative fees for each fee type (e.g. "position_fees")
    /// Each fee type is now a map from token address to amount (U256)
    pub position_fees: HashMap<Address, U256>,
    pub liquidation_fees: HashMap<Address, U256>,
    pub swap_fees: HashMap<Address, U256>,
    pub borrowing_fees: HashMap<Address, U256>,
}

impl MarketFees {
    pub fn new() -> Self {
        Self {
            position_fees: HashMap::new(),
            liquidation_fees: HashMap::new(),
            swap_fees: HashMap::new(),
            borrowing_fees: HashMap::new(),
        }
    }
}

// --- Cumulative Fees Map type ---
pub type CumulativeFeesMap = Arc<Mutex<HashMap<Address, MarketFees>>>;