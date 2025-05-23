use ethers::prelude::*;
use std::str::FromStr;
use std::sync::Arc;
use eyre::Report;

use crate::config::Config;
use crate::constants::{ARBITRUM_MAINNET_CHAIN_ID, ARBITRUM_SEPOLIA_CHAIN_ID};


/// Returns a Wallet + Provider combo as a `SignerMiddleware`
pub async fn create_wallet(config: &Config) -> Result<
    SignerMiddleware<Arc<Provider<Http>>, Wallet<k256::ecdsa::SigningKey>>,
    Report,
> {
    // Get chain ID
    let chain_id = match config.mode.as_str() {
        "test" => ARBITRUM_SEPOLIA_CHAIN_ID,
        "prod" => ARBITRUM_MAINNET_CHAIN_ID,
        _ => panic!("Invalid MODE value (must be 'test' or 'prod')"),
    };
    
    // Load wallet from private key
    let wallet = Wallet::from_str(&config.wallet_private_key)?
        .with_chain_id(chain_id); // Arbitrum Sepolia testnet

    // Use already-built provider (already Arc-wrapped)
    let provider = config.alchemy_provider.clone();

    // Combine wallet and provider
    let client = SignerMiddleware::new(provider, wallet);
    Ok(client)
}