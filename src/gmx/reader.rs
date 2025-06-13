use ethers::prelude::*;
use ethers::utils::keccak256;
use eyre::Result;

use crate::config::Config;
use super::reader_utils;

abigen!(
    Reader,
    "./abis/Reader.json"
);

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
