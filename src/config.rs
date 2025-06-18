use std::env;
use std::sync::Arc;
use ethers::providers::{Provider, Http, Ws};
use ethers::types::Address;

use crate::constants;

#[derive(Debug, Clone)]
pub struct Config {
    pub alchemy_provider: Arc<Provider<Http>>,
    pub alchemy_ws_provider: Arc<Provider<Ws>>,
    pub wallet_private_key: String,
    pub network_mode: String,
    pub chain_id: u64,
    pub gmx_datastore: Address,
    pub gmx_reader: Address,
    pub gmx_eventemitter: Address,
    pub etherscan_api_key: String,
    pub refetch_abis: bool,
    pub database_url: String,
}

impl Config {
    pub async fn load() -> Self {
        // Load network mode env var and validate it
        let network_mode = env::var("NETWORK_MODE").expect("Missing NETWORK_MODE environment variable");
        if network_mode != "test" && network_mode != "prod" {
            panic!("NETWORK_MODE must be either 'test' or 'prod'");
        }
        
        // Load alchemy RPC HTTP URL based on network mode, create ethers provider
        let alchemy_rpc_url = match network_mode.as_str() {
            "test" => env::var("ALCHEMY_RPC_URL_TEST").expect("Missing ALCHEMY_RPC_URL_TEST"),
            "prod" => env::var("ALCHEMY_RPC_URL_PROD").expect("Missing ALCHEMY_RPC_URL_PROD"),
            _ => panic!("Invalid NETWORK_MODE"),
        };

        let provider = Provider::<Http>::try_from(alchemy_rpc_url)
            .expect("Failed to create Alchemy provider");

        // Load alchemy WebSocket URL based on network mode, create ethers provider
        let alchemy_ws_url = match network_mode.as_str() {
            "test" => env::var("ALCHEMY_WS_URL_TEST").expect("Missing ALCHEMY_WS_URL_TEST"),
            "prod" => env::var("ALCHEMY_WS_URL_PROD").expect("Missing ALCHEMY_WS_URL_PROD"),
            _ => panic!("Invalid NETWORK_MODE"),
        };

        let ws_provider = Provider::<Ws>::connect(alchemy_ws_url).await
            .expect("Failed to create Alchemy WebSocket provider");

        // Load wallet private key based on network mode
        let wallet_private_key = match network_mode.as_str() {
            "test" => env::var("WALLET_PRIVATE_KEY_TEST").expect("Missing WALLET_PRIVATE_KEY_TEST"),
            "prod" => env::var("WALLET_PRIVATE_KEY_PROD").expect("Missing WALLET_PRIVATE_KEY_PROD"),
            _ => panic!("Invalid NETWORK_MODE"),
        };

        // Load chain ID based on network mode
        let chain_id = match network_mode.as_str() {
            "test" => constants::ARBITRUM_SEPOLIA_CHAIN_ID,
            "prod" => constants::ARBITRUM_MAINNET_CHAIN_ID,
            _ => panic!("Invalid NETWORK_MODE"),
        };

        // Load GMX contract addresses based on network mode
        let (gmx_datastore, gmx_reader, gmx_eventemitter) = match network_mode.as_str() {
            "test" => (
                constants::GMX_DATASTORE_ADDRESS_SEPOLIA, 
                constants::GMX_READER_ADDRESS_SEPOLIA,
                constants::GMX_EVENTEMITTER_ADDRESS_SEPOLIA,
            ),
            "prod" => (
                constants::GMX_DATASTORE_ADDRESS_MAINNET, 
                constants::GMX_READER_ADDRESS_MAINNET,
                constants::GMX_EVENTEMITTER_ADDRESS_MAINNET,
            ),
            _ => panic!("Invalid NETWORK_MODE"),
        };

        // Load Etherscan API key, refetch ABIs flag
        let etherscan_api_key = env::var("ETHERSCAN_API_KEY").expect("Missing ETHERSCAN_API_KEY");
        let refetch_abis = env::var("REFETCH_ABIS")
            .map(|v| v.parse().unwrap_or(false))
            .unwrap_or(false);

        // Load database URL
        let database_url = env::var("DATABASE_URL").expect("Missing DATABASE_URL");

        Config {
            alchemy_provider: Arc::new(provider),
            alchemy_ws_provider: Arc::new(ws_provider),
            wallet_private_key,
            network_mode,
            chain_id,
            gmx_datastore: gmx_datastore.parse().expect("Invalid GMX DataStore address"),
            gmx_reader: gmx_reader.parse().expect("Invalid GMX Reader address"),
            gmx_eventemitter: gmx_eventemitter.parse().expect("Invalid GMX EventEmitter address"),
            etherscan_api_key,
            refetch_abis,
            database_url,
        }
    }
}