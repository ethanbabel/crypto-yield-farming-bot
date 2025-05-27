use ethers::types::{Address, U256, I256};
use crate::gmx::reader;

// Return type for Reader.getMarkets
#[derive(Debug, Clone)]
pub struct MarketProps {
    pub market_token: Address,
    pub index_token: Address,
    pub long_token: Address,
    pub short_token: Address,
}

// Return type for Reader.getMarketInfo
#[derive(Debug, Clone)]
pub struct MarketInfo {
    pub market: MarketProps,                                // Address  of market, index, long, and short tokens
    pub borrowing_factor_per_second_for_longs: U256,        // Borrowing cost growth rate for long positions (fee that short traders pay per second for holding a long position)
    pub borrowing_factor_per_second_for_shorts: U256,       // Borrowing cost growth rate for short positions (fee that long traders pay per second for holding a short position)
    pub base_funding: BaseFundingValues,                    // Struct of current cumulative funding fee values
    pub next_funding: GetNextFundingAmountPerSizeResult,    // Struct of projected next values for each funding fee
    pub virtual_inventory: VirtualInventory,                // Struct reflecting GMX internal inventory state
    pub is_disabled: bool,                                  // Whether the market is disabled   
}

#[derive(Debug, Clone)]
pub struct PriceProps { // Prices in GMX are represented as int256s with 30 decimals
    pub min: U256,
    pub max: U256,
}

#[derive(Debug, Clone)]
pub struct MarketPrices {
    pub index_token_price: PriceProps,
    pub long_token_price: PriceProps,
    pub short_token_price: PriceProps,
}

#[derive(Debug, Clone)]
pub struct CollateralType {
    pub long_token: U256,   // Amount of long token in the collateral (in raw token units)
    pub short_token: U256,  // Amount of short token in the collateral (in raw token units)
}

#[derive(Debug, Clone)]
pub struct PositionType {
    pub long: CollateralType,      // Funding fee for long positions (CollateralType splits the long fee into amounts of the short and long tokens)
    pub short: CollateralType,     // Funding fee for short positions (CollateralType splits the short fee into amounts of the long and short tokens)
}

#[derive(Debug, Clone)]
pub struct BaseFundingValues {
    pub funding_fee_amount_per_size: PositionType,          // Current funding fee per size unit
    pub claimable_funding_amount_per_size: PositionType,    // Claimable funding amount per size unit
}

#[derive(Debug, Clone)]
pub struct GetNextFundingAmountPerSizeResult {
    pub longs_pay_shorts: bool,                                 // If true, longs pay shorts (and vice versa)     
    pub funding_factor_per_second: U256,                        // Rate used to compute upcoming funding fees
    pub next_saved_funding_factor_per_second: I256,             // Cached next funding rate
    pub funding_fee_amount_per_size_delta: PositionType,        // Expected increase in fees
    pub claimable_funding_amount_per_size_delta: PositionType,  // Expected increase in claimable funding
}

#[derive(Debug, Clone)]
pub struct VirtualInventory { 
    pub virtual_pool_amount_for_long_token: U256,     // “Pretend” long-side liquidity for internal pricing
    pub virtual_pool_amount_for_short_token: U256,    // “Pretend” short-side liquidity for internal pricing
    pub virtual_inventory_for_positions: I256,  // "Pretend" net inventory for internal pricing
}

// Return type for Reader.getMarketTokenPrice
#[derive(Debug, Clone)]
pub struct MarketPoolValueInfoProps { 
    pub pool_value: I256,                   // Total pool value in USD
    pub long_pnl: I256,                     // The pending pnl of long positions
    pub short_pnl: I256,                    // The pending pnl of short positions
    pub net_pnl: I256,                      // The net pnl of short and long positions
    pub long_token_amount: U256,            // The amount of long token in the pool
    pub short_token_amount: U256,           // The amount of short token in the pool
    pub long_token_usd: U256,               // The USD value of the long tokens in the pool
    pub short_token_usd: U256,              // The USD value of the short tokens in the pool
    pub total_borrowing_fees: U256,         // The total pending borrowing fees for the market
    pub borrowing_fee_pool_factor: U256,    // The borrowing fee pool factor for the market
    pub impact_pool_amount: U256,          // Amound of tokens reserved for price impact smoothing
}

//----------------------------------------------------------------------------------------------------------------------------------------

impl From<reader::MarketProps> for MarketProps {
    fn from(m: reader::MarketProps) -> Self {
        Self {
            market_token: m.market_token,
            index_token: m.index_token,
            long_token: m.long_token,
            short_token: m.short_token,
        }
    }
}

impl From<reader::MarketInfo> for MarketInfo {
    fn from(m: reader::MarketInfo) -> Self {
        Self {
            market: m.market.into(),
            borrowing_factor_per_second_for_longs: m.borrowing_factor_per_second_for_longs,
            borrowing_factor_per_second_for_shorts: m.borrowing_factor_per_second_for_shorts,
            base_funding: m.base_funding.into(),
            next_funding: m.next_funding.into(),
            virtual_inventory: m.virtual_inventory.into(),
            is_disabled: m.is_disabled,
        }
    }
}

impl From<reader::PriceProps> for PriceProps {
    fn from(p: reader::PriceProps) -> Self {
        Self {
            min: p.min,
            max: p.max,
        }
    }
}

impl From<reader::MarketPrices> for MarketPrices {
    fn from(m: reader::MarketPrices) -> Self {
        Self {
            index_token_price: m.index_token_price.into(),
            long_token_price: m.long_token_price.into(),
            short_token_price: m.short_token_price.into(),
        }
    }
}

impl From<reader::BaseFundingValues> for BaseFundingValues {
    fn from(m: reader::BaseFundingValues) -> Self {
        Self {
            funding_fee_amount_per_size: m.funding_fee_amount_per_size.into(),
            claimable_funding_amount_per_size: m.claimable_funding_amount_per_size.into(),
        }
    }
}

impl From<reader::GetNextFundingAmountPerSizeResult> for GetNextFundingAmountPerSizeResult {
    fn from(m: reader::GetNextFundingAmountPerSizeResult) -> Self {
        Self {
            longs_pay_shorts: m.longs_pay_shorts,
            funding_factor_per_second: m.funding_factor_per_second,
            next_saved_funding_factor_per_second: m.next_saved_funding_factor_per_second,
            funding_fee_amount_per_size_delta: m.funding_fee_amount_per_size_delta.into(),
            claimable_funding_amount_per_size_delta: m.claimable_funding_amount_per_size_delta.into(),
        }
    }
}

impl From<reader::VirtualInventory> for VirtualInventory {
    fn from(m: reader::VirtualInventory) -> Self {
        Self {
            virtual_pool_amount_for_long_token: m.virtual_pool_amount_for_long_token,
            virtual_pool_amount_for_short_token: m.virtual_pool_amount_for_short_token,
            virtual_inventory_for_positions: m.virtual_inventory_for_positions,
        }
    }
}

impl From<reader::CollateralType> for CollateralType {
    fn from(m: reader::CollateralType) -> Self {
        Self {
            long_token: m.long_token,
            short_token: m.short_token,
        }
    }
}

impl From<reader::PositionType> for PositionType {
    fn from(m: reader::PositionType) -> Self {
        Self {
            long: m.long.into(),
            short: m.short.into(),
        }
    }
}

impl From<reader::MarketPoolValueInfoProps> for MarketPoolValueInfoProps {
    fn from(m: reader::MarketPoolValueInfoProps) -> Self {
        Self {
            pool_value: m.pool_value,
            long_pnl: m.long_pnl,
            short_pnl: m.short_pnl,
            net_pnl: m.net_pnl,
            long_token_amount: m.long_token_amount,
            short_token_amount: m.short_token_amount,
            long_token_usd: m.long_token_usd,
            short_token_usd: m.short_token_usd,
            total_borrowing_fees: m.total_borrowing_fees,
            borrowing_fee_pool_factor: m.borrowing_fee_pool_factor,
            impact_pool_amount: m.impact_pool_amount,
        }
    }
}