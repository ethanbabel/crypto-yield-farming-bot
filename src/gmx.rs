use ethers::prelude::*;
use ethers::utils::keccak256;
use eyre::Result;

use crate::config::Config;
use crate::gmx_structs;

abigen!(
    Reader,
    "./abis/Reader.json"
);

pub enum PnlFactorType {
    Deposit,
    Withdrawal,
    Trader,
}

pub async fn get_markets(config: &Config) -> Result<Vec<gmx_structs::MarketProps>> {
    let reader = Reader::new(config.gmx_reader, config.alchemy_provider.clone());

    // Fetch markets from the GMX Reader contract
    let raw_response = reader.get_markets(
        config.gmx_datastore,
        U256::from(0), 
        U256::from(1000), // Intentionally large to fetch all markets
    ).call().await?;

    let markets: Vec<gmx_structs::MarketProps> = raw_response
        .into_iter()
        .map(|market| market.into())
        .collect();

    Ok(markets)
}

pub async fn get_market_info(config: &Config, market_address: Address, market_prices: gmx_structs::MarketPrices) -> Result<gmx_structs::MarketInfo> {
    let reader = Reader::new(config.gmx_reader, config.alchemy_provider.clone());

    // Fetch market info from the GMX Reader contract
    let raw_response = reader.get_market_info(
        config.gmx_datastore,
        market_prices.into(),
        market_address,
    ).call().await?;

    let market_info: gmx_structs::MarketInfo = raw_response.into();

    Ok(market_info)
}

pub async fn get_market_token_price(config: &Config, market_props: gmx_structs::MarketProps, market_prices: gmx_structs::MarketPrices, 
            pnl_factor_type: PnlFactorType, maximize: bool) -> Result<(I256, gmx_structs::MarketPoolValueInfoProps)> {
    let reader = Reader::new(config.gmx_reader, config.alchemy_provider.clone());

    let pnl_factor_type: H256 = match pnl_factor_type {
        PnlFactorType::Deposit => H256::from(keccak256("MAX_PNL_FACTOR_FOR_DEPOSITS")),
        PnlFactorType::Withdrawal => H256::from(keccak256("MAX_PNL_FACTOR_FOR_WITHDRAWALS")),
        PnlFactorType::Trader => H256::from(keccak256("MAX_PNL_FACTOR_FOR_TRADERS")),
    };

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
    let market_pool_value_info_props: gmx_structs::MarketPoolValueInfoProps = raw_response.1.into();

    Ok((market_token_price, market_pool_value_info_props))
}

//----------------------------------------------------------------------------------------------------------------------------------------

impl From<gmx_structs::MarketProps> for MarketProps {
    fn from(m: gmx_structs::MarketProps) -> Self {
        Self {
            market_token: m.market_token,
            index_token: m.index_token,
            long_token: m.long_token,
            short_token: m.short_token,
        }
    }
}

impl From<gmx_structs::MarketPrices> for MarketPrices {
    fn from(m: gmx_structs::MarketPrices) -> Self {
        Self {
            index_token_price: m.index_token_price.into(),
            long_token_price: m.long_token_price.into(),
            short_token_price: m.short_token_price.into(),
        }
    }
}

impl From<gmx_structs::PriceProps> for PriceProps {
    fn from(p: gmx_structs::PriceProps) -> Self {
        Self {
            min: p.min,
            max: p.max,
        }
    }
}
