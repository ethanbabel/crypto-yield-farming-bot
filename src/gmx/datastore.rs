use ethers::prelude::*;
use ethers::utils::keccak256;
use eyre::Result;

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