use chrono::{DateTime, Utc};
use ethers::types::Address;
use rust_decimal::Decimal;
use std::collections::HashMap;

/// Historical slice of market data for one GMX market
#[derive(Debug, Clone)]
pub struct MarketStateSlice {
    pub market_address: Address,
    pub display_name: String, // e.g. "ETH/USD [WETH - USDC]"

    // --- Historical data ---
    pub timestamps: Vec<DateTime<Utc>>,
    pub index_prices: Vec<Decimal>,   // Index token prices from token_prices table
    pub index_token_timestamps: Vec<DateTime<Utc>>, // Timestamps corresponding to index token prices
    pub fees_usd: Vec<Decimal>,       // Total fees collected per timestep

    // --- Current state ---
    // PnL
    pub pnl_net: Decimal, // Most recent net PnL (USD)
    pub pnl_long: Decimal, // Most recent PnL from long positions (USD)
    pub pnl_short: Decimal, // Most recent PnL from short positions (USD)

    // Open interest
    pub oi_long: Decimal, // Most recent cost of open long positions (USD)
    pub oi_short: Decimal, // Most recent cost of open short positions (USD)
    pub oi_long_via_tokens: Decimal, // Most recent value of open long positions (USD)
    pub oi_short_via_tokens: Decimal, // Most recent value of open short positions (USD)
    pub oi_long_token_amount: Decimal, // Most recent long OI in index token
    pub oi_short_token_amount: Decimal, // Most recent short OI in index token

    // Pool composition
    pub pool_long_collateral_usd: Decimal, // Total value of long collateral token (USD)
    pub pool_short_collateral_usd: Decimal, // Total value of short collateral token (USD)
    pub pool_long_collateral_token_amount: Decimal, // Total amount of long collateral token
    pub pool_short_collateral_token_amount: Decimal, // Total amount of short collateral token
    pub impact_pool_usd: Decimal, // Total impact pool value (USD)
    pub impact_pool_token_amount: Decimal, // Total impact pool value in index token
}

/// Return + risk breakdown for a single market
#[derive(Debug, Clone)]
pub struct MarketDiagnostics {
    pub market_address: Address,
    pub display_name: String, // e.g. "ETH/USD [WETH - USDC]"

    pub expected_return: Decimal,
    pub variance: Decimal,

    pub pnl_return: Decimal,
    pub pnl_variance: Decimal,

    pub fee_return: Decimal,
    pub fee_variance: Decimal,
}

/// Final allocation decision and diagnostics
#[derive(Debug, Clone)]
pub struct AllocationPlan {
    pub weights: HashMap<Address, Decimal>,             // market_address -> weight
    pub diagnostics: HashMap<Address, MarketDiagnostics>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TokenCategory {
    BlueChip,     // e.g. BTC, ETH
    MidCap,       // e.g. LINK, UNI
    Unreliable,   // e.g. low liquidity or high volatility tokens
}

/// Configuration for PnL simulation
pub struct PnLSimulationConfig {
    pub time_horizon_hrs: u64,       // e.g. 72 for 3 days
    pub n_simulations: usize,        // e.g. 1000
    pub token_category: TokenCategory,
}