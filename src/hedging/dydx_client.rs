use eyre::Result;
use std::sync::Arc;
use std::collections::HashMap;
use futures::stream::{self, StreamExt};
use dydx::{
    config::ClientConfig,
    node::NodeClient,
    indexer::{
        IndexerClient,
        types::{Ticker, PerpetualMarket},
    },
};

use crate::config;
use crate::wallet::WalletManager;
use super::hedge_utils;

pub struct DydxClient {
    node_client: NodeClient,
    indexer_client: IndexerClient,
    wallet_manager: Arc<WalletManager>,
}

impl DydxClient {
    pub async fn new(wallet_manager: Arc<WalletManager>) -> Result<Self> {
        // Initialize crypto provider
        config::init_crypto_provider();

        let config = ClientConfig::from_file("src/hedging/dydx_mainnet.toml")
            .await
            .map_err(|e| eyre::eyre!("Failed to load dYdX config: {}", e))?;
        let node_client = NodeClient::connect(config.node)
            .await
            .map_err(|e| eyre::eyre!("Failed to connect to dYdX node: {}", e))?;
        let indexer_client = IndexerClient::new(config.indexer);
        
        Ok(Self {
            node_client,
            indexer_client,
            wallet_manager,
        })
    }

    pub async fn get_perpetual_markets(&self) -> Result<HashMap<Ticker, PerpetualMarket>> {
        let markets = self.indexer_client.markets().get_perpetual_markets(None).await
            .map_err(|e| eyre::eyre!("Failed to fetch perpetual markets: {}", e))?;
        Ok(markets)
    }

    pub async fn get_perpetual_market(&self, long_token: &str) -> Result<Option<PerpetualMarket>> {
        if hedge_utils::STABLE_COINS.contains(&long_token) {
            return Ok(None);
        }
        let ticker = hedge_utils::get_dydx_perp_ticker(long_token);
        match self.indexer_client.markets().get_perpetual_market(&ticker.into()).await {
            Ok(market) => Ok(Some(market)),
            Err(e) => {
                if e.to_string().contains("400 Bad Request") {
                    Ok(None)
                } else {
                    Err(eyre::eyre!("Failed to fetch perpetual market for {}: {}", long_token, e))
                }
            }
        }
    }

    pub async fn get_token_perp_map(&self) -> Result<HashMap<String, Option<PerpetualMarket>>> {
        let token_symbols: Vec<String> = self.wallet_manager.asset_tokens.values()
            .map(|token| token.symbol.clone())
            .collect();

        let results: Vec<_> = stream::iter(token_symbols)
            .map(|token_symbol| async move {
                let market = self.get_perpetual_market(&token_symbol).await;
                (token_symbol, market)
            })
            .buffer_unordered(10)
            .collect()
            .await;

        let mut token_perp_map = HashMap::new();
        for (token_symbol, market_result) in results {
            match market_result {
                Ok(market) => {
                    token_perp_map.insert(token_symbol, market);
                }
                Err(e) => {
                    return Err(eyre::eyre!("Error fetching market for {}: {}", token_symbol, e));
                }
            }
        }
        Ok(token_perp_map)
    }              
}
    
