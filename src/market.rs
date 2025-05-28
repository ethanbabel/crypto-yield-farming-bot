use std::collections::HashMap;
use std::fmt;

use ethers::types::{Address, H160};

use crate::token::{AssetToken, AssetTokenRegistry};
use crate::gmx_structs::{MarketPrices, MarketProps};

#[derive(Debug, Clone)]
pub struct Market {
    pub market_token: Address,
    pub index_token: AssetToken,
    pub long_token: AssetToken,
    pub short_token: AssetToken,
}

impl fmt::Display for Market {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}/USD [{} - {}]",
            self.index_token.symbol,
            self.long_token.symbol,
            self.short_token.symbol
        )
    }
}

impl Market {
    /// Construct a MarketProps struct from the latest price data
    pub fn market_props(&self) -> MarketProps {
        MarketProps {
            market_token: self.market_token,
            index_token: self.index_token.address,
            long_token: self.long_token.address,
            short_token: self.short_token.address,
        }
    }

    /// Construct a MarketPrices struct from the latest price data
    pub fn market_prices(&self) -> Option<MarketPrices> {
        Some(MarketPrices {
            index_token_price: self.index_token.price_props()?,
            long_token_price: self.long_token.price_props()?,
            short_token_price: self.short_token.price_props()?,
        })
    }

}

pub struct MarketRegistry {
    markets: HashMap<Address, Market>,
}

impl MarketRegistry {
    pub fn new() -> Self {
        Self {
            markets: HashMap::new(),
        }
    }

    /// Populate the registry using MarketProps and a reference to the TokenRegistry
    pub fn populate(
        &mut self,
        market_props_list: &Vec<MarketProps>,
        asset_token_registry: &AssetTokenRegistry,
    ) {
        for props in market_props_list {
            let index = asset_token_registry.get_asset_token(&props.index_token);
            let long = asset_token_registry.get_asset_token(&props.long_token);
            let short = asset_token_registry.get_asset_token(&props.short_token);

            if let (Some(index), Some(long), Some(short)) = (index, long, short) {
                let market = Market {
                    market_token: props.market_token,
                    index_token: index.clone(),
                    long_token: long.clone(),
                    short_token: short.clone(),
                };
                self.markets.insert(props.market_token, market);
            } else {
                // Index token address is zero for swap markets, safe to ignore for this bot
                if props.index_token != H160::zero() {
                    tracing::warn!(
                        "Missing tokens for market {:?}: index {:?}, long {:?}, short {:?}",
                        props.market_token, props.index_token, props.long_token, props.short_token
                    );
                }
            }
        }
    }

    pub fn get_market(&self, market_token: &Address) -> Option<&Market> {
        self.markets.get(market_token)
    }

    pub fn num_markets(&self) -> usize {
        self.markets.len()
    }

    pub fn all_markets(&self) -> impl Iterator<Item = &Market> {
        self.markets.values()
    }

    pub fn print_markets(&self) {
        for market in self.all_markets() {
            println!("{}", market);
        }
    }
}