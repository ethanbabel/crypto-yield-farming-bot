use ethers::prelude::*;
use eyre::Result;

use crate::config::Config;

#[derive(Debug, Clone)]
pub struct MarketProps {
    pub market_token: Address,
    pub index_token: Address,
    pub long_token: Address,
    pub short_token: Address,
}

abigen!(
    Reader,
    "./abis/Reader.json"
);

pub async fn get_markets(config: &Config) -> Result<Vec<MarketProps>> {
    let reader = Reader::new(config.gmx_reader, config.alchemy_provider.clone());

    // Fetch markets from the GMX Reader contract
    let raw_response = reader.get_markets(
        config.gmx_datastore,
        U256::from(0), 
        U256::from(1000), // Intentionally large to fetch all markets
    ).call().await?;

    let markets: Vec<MarketProps> = raw_response
        .into_iter()
        .map(|market| MarketProps {
            market_token: market.market_token,
            index_token: market.index_token,
            long_token: market.long_token,
            short_token: market.short_token,
        })
        .collect();

        Ok(markets)
}