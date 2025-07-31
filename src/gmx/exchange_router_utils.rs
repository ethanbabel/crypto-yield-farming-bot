use ethers::types::{
    Address, U256,
};

#[derive(Debug, Clone)]
pub struct CreateDepositParams {
    pub receiver: Address, // Address of the receiver of GM tokens (e.g. my wallet)
    pub callback_contract: Address, // Address of the contract to call on deposit execution/cancellation
    pub ui_fee_receiver: Address, // Address to receive UI fees
    pub market: Address, // Address of the market to deposit into
    pub initial_long_token: Address, // Address of the long token initially sent into the contract to deposit
    pub initial_short_token: Address, // Address of the short token initially sent into the contract to deposit
    pub long_token_swap_path: Vec<Address>, // Array of market addresses to swap initial_long_token for deposits
    pub short_token_swap_path: Vec<Address>, // Array of market addresses to swap initial_short_token for deposits
    pub min_market_tokens: U256, // Minimum amount of market tokens to receive from the deposit
    pub should_unwrap_native_token: bool, // Whether to unwrap native token (e.g. if deposit is cancelled and initialLongToken is WETH, set this true to convert WETH to ETH before refund)
    pub execution_fee: U256, // Max amount of native token (ETH) to pay for deposit fees
    pub callback_gas_limit: U256, // Gas limit for the callback contract execution
}

#[derive(Debug, Clone)]
pub struct CreateWithdrawalParams {
    pub receiver: Address, // Address of the receiver of withdrawal tokens (e.g. my wallet)
    pub callback_contract: Address, // Address of the contract to call on withdrawal execution/cancellation
    pub ui_fee_receiver: Address, // Address to receive UI fees
    pub market: Address, // Address of the market to withdraw from
    pub long_token_swap_path: Vec<Address>, // Array of market addresses to swap long tokens for withdrawals
    pub short_token_swap_path: Vec<Address>, // Array of market addresses to swap short tokens for withdrawals
    pub min_long_token_amount: U256, // Minimum amount of long tokens to receive from the withdrawal
    pub min_short_token_amount: U256, // Minimum amount of short tokens to receive from the withdrawal
    pub should_unwrap_native_token: bool, // Whether to unwrap native token (e.g. if withdrawal long token is WETH, set this true to convert WETH to ETH before transfering)
    pub execution_fee: U256, // Max amount of native token (ETH) to pay for withdrawal fees
    pub callback_gas_limit: U256, // Gas limit for the callback contract execution
}

#[derive(Debug, Clone)]
pub struct CreateShiftParams {
    pub receiver: Address, // Address of the receiver of shift tokens (e.g. my wallet)
    pub callback_contract: Address, // Address of the contract to call on shift execution/cancellation
    pub ui_fee_receiver: Address, // Address to receive UI fees
    pub from_market: Address, // Address of the market to shift from
    pub to_market: Address, // Address of the market to shift to
    pub min_market_tokens: U256, // Minimum amount of toMarket tokens to receive from the shift
    pub execution_fee: U256, // Max amount of native token (ETH) to pay for shift fees
    pub callback_gas_limit: U256, // Gas limit for the callback contract execution
}
