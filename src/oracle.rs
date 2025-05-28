use ethers::prelude::*;
use eyre::Result;
use std::time::Instant;

use crate::config::Config;

// ABI for Chainlink AggregatorV3Interface
abigen!(
    AggregatorV3Interface,
    r#"[
        function latestRoundData() external view returns (uint80, int256, uint256, uint256, uint80)
        function decimals() external view returns (uint8)
    ]"#
);

/// An individual Chainlink price feed (e.g., ETH/USD)
#[derive(Debug, Clone)]
pub struct OracleFeed {
    pub aggregator: Address,
}

/// A sequence of oracle feeds representing one composite price
#[derive(Debug, Clone)]
pub struct Oracle {
    pub feeds: Vec<OracleFeed>,         // Ordered list of feeds (e.g., [wstETH/ETH, ETH/USD])
    pub price: Option<f64>,             // Final computed USD price
    pub updated_at: Option<Instant>,    // Timestamp of last price update
}

impl Oracle {
    pub fn new_single(aggregator: Address) -> Self {
        Self {
            feeds: vec![OracleFeed { aggregator }],
            price: None,
            updated_at: None,
        }
    }

    pub fn new_composite(feeds: Vec<Address>) -> Self {
        Self {
            feeds: feeds.into_iter().map(|a| OracleFeed { aggregator: a }).collect(),
            price: None,
            updated_at: None,
        }
    }

    /// Fetch and multiply through the oracle chain (e.g., wstETH/ETH * ETH/USD)
    pub async fn fetch_price(&mut self, config: &Config) -> Result<()> {
        let provider = config.alchemy_provider.clone();
        let mut final_price = 1.0;

        for feed in &self.feeds {
            let contract = AggregatorV3Interface::new(feed.aggregator, provider.clone());
            let decimals = contract.decimals().call().await?;
            let round_data = contract.latest_round_data().call().await?;
            let raw_answer = round_data.1;

            if raw_answer <= I256::zero() {
                eyre::bail!("Oracle at {:?} returned invalid price", feed.aggregator);
            }
            
            let answer_i128: i128 = raw_answer.as_i128(); 
            let price = answer_i128 as f64 / 10f64.powi(decimals as i32);
            final_price *= price;
        }

        self.price = Some(final_price);
        self.updated_at = Some(Instant::now());
        Ok(())
    }
}