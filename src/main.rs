use ethers::providers::Middleware;
use ethers::signers::Signer;
use ethers::types::Address;

use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::wallet;
use crypto_yield_farming_bot::gmx;
use crypto_yield_farming_bot::abi_fetcher;
use crypto_yield_farming_bot::token;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // Load configuration (including provider)
    let cfg = config::Config::load();

    // Print config mode (test or prod)
    println!("Running in {} mode", cfg.mode);

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

    // Fetch ABIs
    abi_fetcher::fetch_all_abis(&cfg).await?;
    println!("✅ All ABIs fetched successfully!");

    // Fetch GMX markets
    let markets = gmx::get_markets(&cfg).await?;
    println!("Fetched {} markets from GMX", markets.len());

    // Initialize token registry
    let mut token_registry = token::TokenRegistry::new();
    // Load tokens from file
    token_registry.load_from_file("tokens/asset_token_data.json")?;
    println!("Loaded {} asset tokens to registry", token_registry.num_asset_tokens());
    // Example: Fetch and print a specific asset token
    let weth_address: Address = "0x82aF49447D8a07e3bd95BD0d56f35241523fBab1".parse()?;
    if let Some(weth_token) = token_registry.get_asset_token(&weth_address) {
        println!("WETH Token: {:?}", weth_token);
    } else {
        println!("WETH token not found in registry");
    }
    // Fetch token prices from GMX
    token_registry.update_all_gmx_prices().await?;
    println!("✅ All GMX prices updated successfully!");
    // Fetch token prices from oracles
    token_registry.update_all_oracle_prices(&cfg).await?;
    println!("✅ All oracle prices updated successfully!");
    // Example: Fetch and print a specific asset token after price update (single oracle)
    if let Some(weth_token) = token_registry.get_asset_token(&weth_address) {
        println!("Updated WETH Token: {:?}", weth_token);
    } else {
        println!("WETH token not found in registry after price update");
    }
    // Example: Fetch and print a specific asset token after price update (composite oracle)
    let wsteth_address: Address = "0x5979D7b546E38E414F7E9822514be443A4800529".parse()?;
    if let Some(wsteth_token) = token_registry.get_asset_token(&wsteth_address) {
        println!("Updated WSTETH Token: {:?}", wsteth_token);
    } else {
        println!("WSTETH token not found in registry after price update");
    }
    // Example: Fetch and print a specific asset token after price update (no oracle)
    let bonk_address: Address = "0x1FD10E767187A92f0AB2ABDEEF4505e319cA06B2".parse()?;
    if let Some(bonk_token) = token_registry.get_asset_token(&bonk_address) {
        println!("Updated BONK token: {:?}", bonk_token);
    } else {
        println!("BONK token not found in registry after price update");
    }


    Ok(())
}