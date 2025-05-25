use ethers::prelude::*;
use std::str::FromStr;
use std::sync::Arc;
use eyre::Report;

use crate::config::Config;

/// Returns a Wallet + Provider combo as a `SignerMiddleware`
pub async fn create_wallet(config: &Config) -> Result<
    SignerMiddleware<Arc<Provider<Http>>, Wallet<k256::ecdsa::SigningKey>>,
    Report,
> {
    // Load wallet from private key
    let wallet = Wallet::from_str(&config.wallet_private_key)?
        .with_chain_id(config.chain_id); 

    // Use already-built provider (already Arc-wrapped)
    let provider = config.alchemy_provider.clone();

    // Combine wallet and provider
    let client = SignerMiddleware::new(provider, wallet);
    Ok(client)
}