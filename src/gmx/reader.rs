use ethers::prelude::*;
use ethers::utils::keccak256;
use ethers::contract::Multicall;
use eyre::Result;
use std::collections::HashMap;
use tracing::{debug, instrument};

use crate::config::Config;
use super::reader_utils;

abigen!(
    Reader,
    "./abis/Reader.json"
);

#[derive(Debug)]
pub enum PnlFactorType {
    Deposit,
    Withdrawal,
    Trader,
}

pub async fn get_markets(config: &Config) -> Result<Vec<reader_utils::MarketProps>> {
    let reader = Reader::new(config.gmx_reader, config.alchemy_provider.clone());

    // Fetch markets from the GMX Reader contract
    let raw_response = reader.get_markets(
        config.gmx_datastore,
        U256::from(0), 
        U256::from(1000), // Intentionally large to fetch all markets
    ).call().await?;

    let markets: Vec<reader_utils::MarketProps> = raw_response
        .into_iter()
        .map(|market| market.into())
        .collect();

    Ok(markets)
}

pub async fn get_market_info(config: &Config, market_address: Address, market_prices: reader_utils::MarketPrices) -> Result<reader_utils::MarketInfo> {
    let reader = Reader::new(config.gmx_reader, config.alchemy_provider.clone());

    // Fetch market info from the GMX Reader contract
    let raw_response = reader.get_market_info(
        config.gmx_datastore,
        market_prices.into(),
        market_address,
    ).call().await?;

    let market_info: reader_utils::MarketInfo = raw_response.into();

    Ok(market_info)
}

pub async fn get_market_token_price(config: &Config, market_props: reader_utils::MarketProps, market_prices: reader_utils::MarketPrices, 
            pnl_factor_type: PnlFactorType, maximize: bool) -> Result<(I256, reader_utils::MarketPoolValueInfoProps)> {
    let reader = Reader::new(config.gmx_reader, config.alchemy_provider.clone());

    let pnl_factor_type_string = match pnl_factor_type {
        PnlFactorType::Deposit => "MAX_PNL_FACTOR_FOR_DEPOSITS".to_string(),
        PnlFactorType::Withdrawal => "MAX_PNL_FACTOR_FOR_WITHDRAWALS".to_string(),
        PnlFactorType::Trader => "MAX_PNL_FACTOR_FOR_TRADERS".to_string(),
    };
    let pnl_factor_type_encoded = ethers::abi::encode(&[ethers::abi::Token::String(pnl_factor_type_string)]);
    let pnl_factor_type = H256::from_slice(&keccak256(&pnl_factor_type_encoded));

    let raw_response = reader.get_market_token_price(
        config.gmx_datastore,
        market_props.into(),
        market_prices.index_token_price.into(),
        market_prices.long_token_price.into(),
        market_prices.short_token_price.into(),
        pnl_factor_type.into(),
        maximize,
    ).call().await?;

    let market_token_price: I256 = raw_response.0;
    let market_pool_value_info_props: reader_utils::MarketPoolValueInfoProps = raw_response.1.into();

    Ok((market_token_price, market_pool_value_info_props))
}

/// Batch version: Fetch market info for multiple markets using multicall
#[instrument(skip(config, markets), fields(market_count = markets.len()))]
pub async fn get_market_info_batch(
    config: &Config,
    markets: &[(reader_utils::MarketProps, reader_utils::MarketPrices)],
) -> Result<HashMap<Address, reader_utils::MarketInfo>> {
    debug!(market_count = markets.len(), "Fetching market info batch");
    
    // Create multicall instance
    let mut multicall = Multicall::new(config.alchemy_provider.clone(), None).await?;
    let reader = Reader::new(config.gmx_reader, config.alchemy_provider.clone());
    
    // Add all market info calls
    for (market_props, market_prices) in markets {
        let call = reader.get_market_info(
            config.gmx_datastore,
            market_prices.clone().into(),
            market_props.market_token,
        );
        multicall.add_call(call, false);
    }
    
    // Execute the multicall
    debug!(call_count = markets.len(), "Executing market info multicall");
    let results: Vec<reader::MarketInfo> = multicall.call_array().await?;
    
    // Parse results using Into trait like non-batch methods
    let mut market_infos = HashMap::new();
    for (i, (market_props, _)) in markets.iter().enumerate() {
        if let Some(result) = results.get(i) {
            // The result should be the raw tuple that we can convert using Into
            let market_info: reader_utils::MarketInfo = result.clone().into();
            market_infos.insert(market_props.market_token, market_info);
        }
    }
    
    debug!(
        market_count = markets.len(), 
        result_count = market_infos.len(),
        "Market info batch fetch completed"
    );
    
    Ok(market_infos)
}

/// Batch version: Fetch market token prices for multiple markets using multicall
#[instrument(skip(config, markets), fields(market_count = markets.len()))]
pub async fn get_market_token_price_batch(
    config: &Config,
    markets: &[(reader_utils::MarketProps, reader_utils::MarketPrices)],
    pnl_factor_type: PnlFactorType,
) -> Result<(HashMap<Address, (I256, reader_utils::MarketPoolValueInfoProps)>, HashMap<Address, (I256, reader_utils::MarketPoolValueInfoProps)>)> {
    debug!(market_count = markets.len(), "Fetching market token price batch");
    
    // Create multicall instance
    let mut multicall = Multicall::new(config.alchemy_provider.clone(), None).await?;
    let reader = Reader::new(config.gmx_reader, config.alchemy_provider.clone());
    
    // Build PNL factor type 
    let pnl_factor_type_string = match pnl_factor_type {
        PnlFactorType::Deposit => "MAX_PNL_FACTOR_FOR_DEPOSITS".to_string(),
        PnlFactorType::Withdrawal => "MAX_PNL_FACTOR_FOR_WITHDRAWALS".to_string(),
        PnlFactorType::Trader => "MAX_PNL_FACTOR_FOR_TRADERS".to_string(),
    };
    let pnl_factor_type_encoded = ethers::abi::encode(&[ethers::abi::Token::String(pnl_factor_type_string)]);
    let pnl_factor_type_hash = H256::from_slice(&keccak256(&pnl_factor_type_encoded));
    
    // Add min price calls (maximize = false)
    for (market_props, market_prices) in markets {
        let call = reader.get_market_token_price(
            config.gmx_datastore,
            market_props.clone().into(),
            market_prices.index_token_price.clone().into(),
            market_prices.long_token_price.clone().into(),
            market_prices.short_token_price.clone().into(),
            pnl_factor_type_hash.into(),
            false, // minimize for min price
        );
        multicall.add_call(call, false);
    }
    
    // Add max price calls (maximize = true)
    for (market_props, market_prices) in markets {
        let call = reader.get_market_token_price(
            config.gmx_datastore,
            market_props.clone().into(),
            market_prices.index_token_price.clone().into(),
            market_prices.long_token_price.clone().into(),
            market_prices.short_token_price.clone().into(),
            pnl_factor_type_hash.into(),
            true, // maximize for max price
        );
        multicall.add_call(call, false);
    }
    
    // Execute the multicall
    debug!(call_count = markets.len() * 2, "Executing market token price multicall");
    let results: Vec<(I256, reader::MarketPoolValueInfoProps)> = multicall.call_array().await?;
    
    // Parse min prices (first half of results)
    let mut min_prices = HashMap::new();
    for (i, (market_props, _)) in markets.iter().enumerate() {
        if let Some(result) = results.get(i) {
            let price: I256 = result.0;
            let pool_info: reader_utils::MarketPoolValueInfoProps = result.1.clone().into();
            min_prices.insert(market_props.market_token, (price, pool_info));
        }
    }
    
    // Parse max prices (second half of results)
    let mut max_prices = HashMap::new();
    for (i, (market_props, _)) in markets.iter().enumerate() {
        if let Some(result) = results.get(markets.len() + i) {
            let price: I256 = result.0;
            let pool_info: reader_utils::MarketPoolValueInfoProps = result.1.clone().into();
            max_prices.insert(market_props.market_token, (price, pool_info));
        }
    }
    
    debug!(
        market_count = markets.len(),
        min_result_count = min_prices.len(),
        max_result_count = max_prices.len(),
        "Market token price batch fetch completed"
    );
    
    Ok((min_prices, max_prices))
}

//----------------------------------------------------------------------------------------------------------------------------------------

impl From<reader_utils::MarketProps> for MarketProps {
    fn from(m: reader_utils::MarketProps) -> Self {
        Self {
            market_token: m.market_token,
            index_token: m.index_token,
            long_token: m.long_token,
            short_token: m.short_token,
        }
    }
}

impl From<reader_utils::MarketPrices> for MarketPrices {
    fn from(m: reader_utils::MarketPrices) -> Self {
        Self {
            index_token_price: m.index_token_price.into(),
            long_token_price: m.long_token_price.into(),
            short_token_price: m.short_token_price.into(),
        }
    }
}

impl From<reader_utils::PriceProps> for PriceProps {
    fn from(p: reader_utils::PriceProps) -> Self {
        Self {
            min: p.min,
            max: p.max,
        }
    }
}
