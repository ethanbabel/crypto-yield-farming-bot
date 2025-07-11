use ethers::{
    types::{Address, U256, H256},
    utils::{keccak256},
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

// --- Helper functions ---
pub fn string_to_bytes32(s: &str) -> H256 {
    H256::from_slice(&keccak256(s.as_bytes()))
}

// --- Cumulative Fees Struct (for each market) ---
#[derive(Debug, Clone)]
pub struct MarketFees {
    // Cumulative fees for each fee type
    pub position_fees: HashMap<Address, U256>,
    pub liquidation_fees: HashMap<Address, U256>,
    pub swap_fees: HashMap<Address, U256>,
    pub borrowing_fees: HashMap<Address, U256>,

    pub trading_volume: U256,
    pub swap_volume: HashMap<Address, U256>,
}

impl MarketFees {
    pub fn new() -> Self {
        Self {
            position_fees: HashMap::new(),
            liquidation_fees: HashMap::new(),
            swap_fees: HashMap::new(),
            borrowing_fees: HashMap::new(),
            trading_volume: U256::zero(),
            swap_volume: HashMap::new(),
        }
    }
}

// --- Cumulative Fees Map type ---
pub type CumulativeFeesMap = Arc<Mutex<HashMap<Address, MarketFees>>>;