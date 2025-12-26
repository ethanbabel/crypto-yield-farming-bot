use eyre::Result;
// use tracing::{info};
use dydx::{
    config::ClientConfig,
    node::NodeClient,
};
use dydx_proto::dydxprotocol::perpetuals::Perpetual;

use crate::config;

pub struct DydxClient {
    node_client: NodeClient,
}

impl DydxClient {
    pub async fn new() -> Result<Self> {
        // Initialize crypto provider
        config::init_crypto_provider();

        let config = ClientConfig::from_file("src/hedging/dydx_mainnet.toml")
            .await
            .map_err(|e| eyre::eyre!("Failed to load dYdX config: {}", e))?;
        let node_client = NodeClient::connect(config.node)
            .await
            .map_err(|e| eyre::eyre!("Failed to connect to dYdX node: {}", e))?;
        Ok(Self {
            node_client,
        })
    }

    pub async fn get_perpetual_markets(&mut self) -> Result<Vec<Perpetual>> {
        let markets = self.node_client.get_perpetuals(None)
            .await
            .map_err(|e| eyre::eyre!("Failed to fetch perpetual markets: {}", e))?;
        Ok(markets)
    }
}
    
