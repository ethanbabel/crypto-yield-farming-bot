use std::collections::HashMap;

use chrono::{DateTime, Utc};
use ethers::types::Address;
use rust_decimal::Decimal;

#[derive(Debug, Clone)]
pub struct PortfolioSnapshot {
    pub timestamp: DateTime<Utc>,
    pub market_balances: HashMap<Address, Decimal>,
    pub market_values_usd: HashMap<Address, Decimal>,
    pub asset_balances: HashMap<Address, Decimal>,
    pub asset_values_usd: HashMap<Address, Decimal>,
    pub hedge_positions: HashMap<String, Decimal>,
    pub native_balance: Decimal,
    pub native_value_usd: Decimal,
    pub dydx_main_usdc: Decimal,
    pub dydx_subaccount_equity: Decimal,
    pub dydx_free_collateral: Decimal,
    pub total_value_usd: Decimal,
    pub market_value_usd: Decimal,
    pub asset_value_usd: Decimal,
    pub arbitrum_value_usd: Decimal,
}

#[derive(Debug, Clone)]
pub struct ExecutionTargets {
    pub market_addresses: Vec<Address>,
    pub weights: Vec<Decimal>,
}

impl ExecutionTargets {
    pub fn new(market_addresses: Vec<Address>, weights: Vec<Decimal>) -> Self {
        assert_eq!(market_addresses.len(), weights.len());

        Self {
            market_addresses,
            weights,
        }
    }
}

#[derive(Debug, Clone)]
pub enum TradeAction {
    GmDeposit {
        market: Address,
        long_amount: Decimal,
        short_amount: Decimal,
    },
    GmWithdrawal {
        market: Address,
        amount: Decimal,
    },
    GmShift {
        from_market: Address,
        to_market: Address,
        amount: Decimal, // amount in from_market to shift
    },
    SpotSwap {
        from_token: Address,
        to_token: Address,
        amount: Decimal,
        side: String,
    },
    HedgeOrder {
        token_symbol: String,
        size: Decimal,
        side_is_buy: bool,
    },
}

impl TradeAction {
    pub fn action_type(&self) -> &'static str {
        match self {
            TradeAction::GmDeposit { .. } => "gm_deposit",
            TradeAction::GmWithdrawal { .. } => "gm_withdrawal",
            TradeAction::GmShift { .. } => "gm_shift",
            TradeAction::SpotSwap { .. } => "spot_swap",
            TradeAction::HedgeOrder { .. } => "hedge_order",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TradeStatus {
    Planned,
    Executed,
    Failed,
    Simulated,
}

impl TradeStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TradeStatus::Planned => "planned",
            TradeStatus::Executed => "executed",
            TradeStatus::Failed => "failed",
            TradeStatus::Simulated => "simulated",
        }
    }
}

#[derive(Debug, Clone)]
pub struct RebalancePlan {
    pub actions: Vec<TradeAction>,
    pub target_weights: HashMap<Address, Decimal>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct PlannerConfig {
    pub min_weight_delta: Decimal,
    pub min_value_usd: Decimal,
    pub unwind_min_value_usd: Decimal,
    pub stable_only_native_buffer_usd: Decimal,
    pub reserve_pct: Decimal,
    pub gas_reserve_pct: Decimal,
}

impl Default for PlannerConfig {
    fn default() -> Self {
        Self {
            min_weight_delta: Decimal::new(1, 2),
            min_value_usd: Decimal::new(10, 0),
            unwind_min_value_usd: Decimal::new(1, 0),
            stable_only_native_buffer_usd: Decimal::new(10, 0),
            reserve_pct: Decimal::new(5, 2),
            gas_reserve_pct: Decimal::new(1, 2),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ReserveState {
    pub reserve_total: Decimal,
    pub investable_capital: Decimal,
    pub required_margin: Decimal,
    pub required_equity: Decimal,
    pub required_free_collateral: Decimal,
    pub upper_equity: Decimal,
    pub gas_reserve_target_usd: Decimal,
    pub gas_reserve_target_eth: Decimal,
}
