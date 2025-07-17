use ethers::prelude::*;
use ethers::utils::keccak256;
use ethers::contract::Multicall;
use eyre::Result;
use std::collections::HashMap;
use tracing::{debug, instrument};

use crate::config::Config;
use crate::constants::GMX_DECIMALS;
use super::reader_utils;

abigen!(
    DataStore,
    "./abis/DataStore.json"
);

async fn get_uint(config: &Config, key: H256) -> Result<U256> {
    let datastore = DataStore::new(config.gmx_datastore, config.alchemy_provider.clone());
    let value: U256 = datastore.get_uint(key.into()).call().await?;
    Ok(value)
}

pub async fn get_open_interest(config: &Config, market_props: reader_utils::MarketProps, is_long: bool) -> Result<U256> {
    fn get_key(market: Address, collateral_token: Address, is_long: bool) -> H256 {
        let open_interest_encoded = ethers::abi::encode(&[ethers::abi::Token::String("OPEN_INTEREST".to_string())]);
        let open_interest_key = H256::from_slice(&keccak256(&open_interest_encoded));
        let encoded = ethers::abi::encode(&[
            ethers::abi::Token::FixedBytes(open_interest_key.as_bytes().to_vec()),
            ethers::abi::Token::Address(market),
            ethers::abi::Token::Address(collateral_token),
            ethers::abi::Token::Bool(is_long),
        ]);
        H256::from(keccak256(encoded))
    }

    let long_collateral_key = get_key(market_props.market_token, market_props.long_token, is_long);
    let short_collateral_key = get_key(market_props.market_token, market_props.short_token, is_long);

    let open_interest_using_long_token_collateral = get_uint(config, long_collateral_key).await?;
    let open_interest_using_short_token_collateral = get_uint(config, short_collateral_key).await?;

    let divisor = if market_props.long_token == market_props.short_token { 2 } else { 1 };

    let open_interest = (open_interest_using_long_token_collateral / U256::from(divisor))
        + (open_interest_using_short_token_collateral / U256::from(divisor));
    Ok(open_interest)
}

pub async fn get_open_interest_in_tokens(config: &Config, market_props: reader_utils::MarketProps, is_long: bool) -> Result<U256> {
    fn get_key(market: Address, collateral_token: Address, is_long: bool) -> H256 {
        let open_interest_in_tokens_encoded = ethers::abi::encode(&[ethers::abi::Token::String("OPEN_INTEREST_IN_TOKENS".to_string())]);
        let open_interest_in_tokens_key = H256::from_slice(&keccak256(&open_interest_in_tokens_encoded));
        let encoded = ethers::abi::encode(&[
            ethers::abi::Token::FixedBytes(open_interest_in_tokens_key.as_bytes().to_vec()),
            ethers::abi::Token::Address(market),
            ethers::abi::Token::Address(collateral_token),
            ethers::abi::Token::Bool(is_long),
        ]);
        H256::from(keccak256(encoded))
    }

    let long_collateral_key = get_key(market_props.market_token, market_props.long_token, is_long);
    let short_collateral_key = get_key(market_props.market_token, market_props.short_token, is_long);

    let open_interest_using_long_token_collateral = get_uint(config, long_collateral_key).await?;
    let open_interest_using_short_token_collateral = get_uint(config, short_collateral_key).await?;

    let divisor = if market_props.long_token == market_props.short_token { 2 } else { 1 };

    let open_interest = (open_interest_using_long_token_collateral / U256::from(divisor))
        + (open_interest_using_short_token_collateral / U256::from(divisor));
    Ok(open_interest)
}

pub async fn get_lp_fee_pool_factors(config: &Config) -> Result<(U256, U256, U256, U256)> {
    let pool_factor_strs = [
        "POSITION_FEE_RECEIVER_FACTOR",
        "LIQUIDATION_FEE_RECEIVER_FACTOR",
        "SWAP_FEE_RECEIVER_FACTOR",
        "BORROWING_FEE_RECEIVER_FACTOR"
    ];

    let mut factors = Vec::with_capacity(4);
    let gmx_precision = U256::from(10).pow(U256::from(GMX_DECIMALS));

    for factor_str in pool_factor_strs.iter() {
        let encoded = ethers::abi::encode(&[ethers::abi::Token::String(factor_str.to_string())]);
        let key = H256::from_slice(&keccak256(&encoded));
        let value = get_uint(config, key).await?;
        factors.push(gmx_precision - value);
    }

    Ok((factors[0], factors[1], factors[2], factors[3]))
}

/// Batch version: Get open interest for multiple markets using multicall
#[instrument(skip(config, markets), fields(market_count = markets.len()))]
pub async fn get_open_interest_batch(
    config: &Config,
    markets: &[reader_utils::MarketProps],
) -> Result<(HashMap<Address, U256>, HashMap<Address, U256>)> {
    debug!(market_count = markets.len(), "Fetching open interest batch");
    
    // Create multicall instance
    let mut multicall = Multicall::new(config.alchemy_provider.clone(), None).await?;
    let datastore = DataStore::new(config.gmx_datastore, config.alchemy_provider.clone());
    
    // Add all open interest calls
    for market_props in markets {
        // Add calls for long and short open interest
        for is_long in [true, false] {
            let long_collateral_key = get_open_interest_key(
                market_props.market_token,
                market_props.long_token,
                is_long,
            );
            let short_collateral_key = get_open_interest_key(
                market_props.market_token,
                market_props.short_token,
                is_long,
            );

            let long_call = datastore.get_uint(long_collateral_key.into());
            let short_call = datastore.get_uint(short_collateral_key.into());
            
            multicall.add_call(long_call, false);
            multicall.add_call(short_call, false);
        }
    }
    
    // Execute the multicall
    debug!(call_count = markets.len() * 4, "Executing open interest multicall");
    let results: Vec<U256> = multicall.call_array().await?;

    // Parse results - each market has 4 results: long_long, long_short, short_long, short_short
    let mut long_interests = HashMap::new();
    let mut short_interests = HashMap::new();
    
    for (i, market_props) in markets.iter().enumerate() {
        let base_idx = i * 4;
        
        // Extract the open interest values
        let long_long = results.get(base_idx).cloned().unwrap_or(U256::zero());
        let long_short = results.get(base_idx + 1).cloned().unwrap_or(U256::zero());
        let short_long = results.get(base_idx + 2).cloned().unwrap_or(U256::zero());
        let short_short = results.get(base_idx + 3).cloned().unwrap_or(U256::zero());
        
        // Calculate totals (same logic as non-batch version)
        let divisor = if market_props.long_token == market_props.short_token { 2 } else { 1 };
        let long_total = (long_long / U256::from(divisor)) + (long_short / U256::from(divisor));
        let short_total = (short_long / U256::from(divisor)) + (short_short / U256::from(divisor));
        
        long_interests.insert(market_props.market_token, long_total);
        short_interests.insert(market_props.market_token, short_total);
    }
    
    debug!(
        market_count = markets.len(),
        long_result_count = long_interests.len(),
        short_result_count = short_interests.len(),
        "Open interest batch fetch completed"
    );
    
    Ok((long_interests, short_interests))
}

/// Batch version: Get open interest in tokens for multiple markets using multicall
#[instrument(skip(config, markets), fields(market_count = markets.len()))]
pub async fn get_open_interest_in_tokens_batch(
    config: &Config,
    markets: &[reader_utils::MarketProps],
) -> Result<(HashMap<Address, U256>, HashMap<Address, U256>)> {
    debug!(market_count = markets.len(), "Fetching open interest in tokens batch");
    
    // Create multicall instance
    let mut multicall = Multicall::new(config.alchemy_provider.clone(), None).await?;
    let datastore = DataStore::new(config.gmx_datastore, config.alchemy_provider.clone());
    
    // Add all open interest in tokens calls
    for market_props in markets {
        // Add calls for long and short open interest in tokens
        for is_long in [true, false] {
            let long_collateral_key = get_open_interest_in_tokens_key(
                market_props.market_token,
                market_props.long_token,
                is_long,
            );
            let short_collateral_key = get_open_interest_in_tokens_key(
                market_props.market_token,
                market_props.short_token,
                is_long,
            );

            let long_call = datastore.get_uint(long_collateral_key.into());
            let short_call = datastore.get_uint(short_collateral_key.into());
            
            multicall.add_call(long_call, false);
            multicall.add_call(short_call, false);
        }
    }
    
    // Execute the multicall
    debug!(call_count = markets.len() * 4, "Executing open interest in tokens multicall");
    let results: Vec<U256> = multicall.call_array().await?;
    
    // Parse results - each market has 4 results: long_long, long_short, short_long, short_short
    let mut long_interests = HashMap::new();
    let mut short_interests = HashMap::new();
    
    for (i, market_props) in markets.iter().enumerate() {
        let base_idx = i * 4;
        
        // Extract open interest values
        let long_long = results.get(base_idx).cloned().unwrap_or(U256::zero());
        let long_short = results.get(base_idx + 1).cloned().unwrap_or(U256::zero());
        let short_long = results.get(base_idx + 2).cloned().unwrap_or(U256::zero());
        let short_short = results.get(base_idx + 3).cloned().unwrap_or(U256::zero());
        
        // Calculate totals (same logic as non-batch version)
        let divisor = if market_props.long_token == market_props.short_token { 2 } else { 1 };
        let long_total = (long_long / U256::from(divisor)) + (long_short / U256::from(divisor));
        let short_total = (short_long / U256::from(divisor)) + (short_short / U256::from(divisor));
        
        long_interests.insert(market_props.market_token, long_total);
        short_interests.insert(market_props.market_token, short_total);
    }
    
    debug!(
        market_count = markets.len(),
        long_result_count = long_interests.len(),
        short_result_count = short_interests.len(),
        "Open interest in tokens batch fetch completed"
    );
    
    Ok((long_interests, short_interests))
}

/// Helper function to generate open interest key
fn get_open_interest_key(market: Address, collateral_token: Address, is_long: bool) -> H256 {
    let open_interest_encoded = ethers::abi::encode(&[ethers::abi::Token::String("OPEN_INTEREST".to_string())]);
    let open_interest_key = H256::from_slice(&keccak256(&open_interest_encoded));
    let encoded = ethers::abi::encode(&[
        ethers::abi::Token::FixedBytes(open_interest_key.as_bytes().to_vec()),
        ethers::abi::Token::Address(market),
        ethers::abi::Token::Address(collateral_token),
        ethers::abi::Token::Bool(is_long),
    ]);
    H256::from(keccak256(encoded))
}

/// Helper function to generate open interest in tokens key
fn get_open_interest_in_tokens_key(market: Address, collateral_token: Address, is_long: bool) -> H256 {
    let open_interest_in_tokens_encoded = ethers::abi::encode(&[ethers::abi::Token::String("OPEN_INTEREST_IN_TOKENS".to_string())]);
    let open_interest_in_tokens_key = H256::from_slice(&keccak256(&open_interest_in_tokens_encoded));
    let encoded = ethers::abi::encode(&[
        ethers::abi::Token::FixedBytes(open_interest_in_tokens_key.as_bytes().to_vec()),
        ethers::abi::Token::Address(market),
        ethers::abi::Token::Address(collateral_token),
        ethers::abi::Token::Bool(is_long),
    ]);
    H256::from(keccak256(encoded))
}