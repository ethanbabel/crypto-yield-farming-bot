use dotenvy::dotenv;
use std::env;
use std::sync::Arc;
use ethers::providers::{Provider, Http};
use ethers::types::Address;

use crate::constants;

#[derive(Debug, Clone)]
pub struct Config {
    pub alchemy_provider: Arc<Provider<Http>>,
    pub wallet_private_key: String,
    pub network_mode: String,
    pub chain_id: u64,
    pub gmx_datastore: Address,
    pub gmx_reader: Address,
    pub etherscan_api_key: String,
    pub refetch_abis: bool,
}

impl Config {
    pub fn load() -> Self {
        // Load environment variables from .env file
        dotenv().ok();

        // Load network mode env var and validate it
        let network_mode = env::var("NETWORK_MODE").expect("Missing NETWORK_MODE environment variable");
        if network_mode != "test" && network_mode != "prod" {
            panic!("NETWORK_MODE must be either 'test' or 'prod'");
        }
        
        // Load alchemy RPC URL based on network mode, create ethers provider, and HTTP client
        let alchemy_rpc_url = match network_mode.as_str() {
            "test" => env::var("ALCHEMY_RPC_URL_TEST").expect("Missing ALCHEMY_RPC_URL_TEST"),
            "prod" => env::var("ALCHEMY_RPC_URL_PROD").expect("Missing ALCHEMY_RPC_URL_PROD"),
            _ => panic!("Invalid NETWORK_MODE"),
        };

        let provider = Provider::<Http>::try_from(alchemy_rpc_url)
            .expect("Failed to create Alchemy provider");

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

        // Load GMX DataStore and Reader addresses based on network mode
        let (gmx_datastore, gmx_reader) = match network_mode.as_str() {
            "test" => (constants::GMX_DATASTORE_ADDRESS_SEPOLIA, constants::GMX_READER_ADDRESS_SEPOLIA),
            "prod" => (constants::GMX_DATASTORE_ADDRESS_MAINNET, constants::GMX_READER_ADDRESS_MAINNET),
            _ => panic!("Invalid NETWORK_MODE"),
        };

        // Load Etherscan API key, refetch ABIs flag
        let etherscan_api_key = env::var("ETHERSCAN_API_KEY").expect("Missing ETHERSCAN_API_KEY");
        let refetch_abis = env::var("REFETCH_ABIS")
            .map(|v| v.parse().unwrap_or(false))
            .unwrap_or(false);
            
        Config {
            alchemy_provider: Arc::new(provider),
            wallet_private_key,
            network_mode,
            chain_id,
            gmx_datastore: gmx_datastore.parse().expect("Invalid GMX DataStore address"),
            gmx_reader: gmx_reader.parse().expect("Invalid GMX Reader address"),
            etherscan_api_key,
            refetch_abis,
        }
    }
}