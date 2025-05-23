use ethers::providers::Middleware;
use ethers::signers::Signer;

use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::wallet;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // Load configuration (including provider)
    let cfg = config::Config::load();

    // Send a test request to Alchemy via ethers::Provider
    let block_number = cfg.alchemy_provider.get_block_number().await?;
    println!("✅ Connected! Current block number: {block_number}");

    // Load wallet + provider combo client
    let client = wallet::create_wallet(&cfg).await?;

    // Get and print wallet address and chain ID
    let wallet_address = client.address();
    let chain_id = client.signer().chain_id();

    println!("Wallet address: {wallet_address}");
    println!("Chain ID: {chain_id}");

    Ok(())
}