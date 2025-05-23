use dotenvy::dotenv;
use std::env;
use std::sync::Arc;
use ethers::providers::{Provider, Http};
use reqwest::Client;

pub struct Config {
    pub alchemy_provider: Arc<Provider<Http>>,
    pub wallet_private_key: String,
    pub http_client: Client,
    pub mode: String,
}

impl Config {
    pub fn load() -> Self {
        dotenv().ok();

        let mode = env::var("MODE").unwrap_or_else(|_| "test".to_string());

        let alchemy_rpc_url = match mode.as_str() {
            "test" => env::var("ALCHEMY_RPC_URL_TEST").expect("Missing ALCHEMY_RPC_URL_TEST"),
            "prod" => env::var("ALCHEMY_RPC_URL_PROD").expect("Missing ALCHEMY_RPC_URL_PROD"),
            _ => panic!("Invalid MODE value (must be 'test' or 'prod')"),
        };

        let wallet_private_key = match mode.as_str() {
            "test" => env::var("WALLET_PRIVATE_KEY_TEST").expect("Missing WALLET_PRIVATE_KEY_TEST"),
            "prod" => env::var("WALLET_PRIVATE_KEY_PROD").expect("Missing WALLET_PRIVATE_KEY_PROD"),
            _ => panic!("Invalid MODE"),
        };

        let provider = Provider::<Http>::try_from(alchemy_rpc_url)
            .expect("Failed to create Alchemy provider");

        let http_client = Client::new();

        Config {
            alchemy_provider: Arc::new(provider),
            wallet_private_key,
            http_client,
            mode,
        }
    }
}