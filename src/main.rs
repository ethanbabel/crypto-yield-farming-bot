// use ethers::providers::Middleware;
// use ethers::signers::Signer;
// use ethers::types::Address;
use tracing_subscriber::FmtSubscriber;

use crypto_yield_farming_bot::config;
// use crypto_yield_farming_bot::wallet;
use crypto_yield_farming_bot::gmx;
// use crypto_yield_farming_bot::gmx_structs;
use crypto_yield_farming_bot::abi_fetcher;
use crypto_yield_farming_bot::token;
use crypto_yield_farming_bot::market;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // Initialize tracing subscriber for logging
    let subscriber = FmtSubscriber::builder()
        .with_max_level(tracing::Level::INFO)
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("Failed to set global tracing subscriber");

    // Load configuration (including provider)
    let cfg = config::Config::load();

    // Print config mode (test or prod)
    println!("Running in {} mode", cfg.mode);

    // Fetch ABIs
    abi_fetcher::fetch_all_abis(&cfg).await?;
    println!("✅ All ABIs fetched successfully!");

    // Initialize token registry
    let mut token_registry = token::AssetTokenRegistry::new(&cfg);

    // Load tokens from file
    token_registry.load_from_file()?;
    println!("Loaded {} asset tokens to registry", token_registry.num_asset_tokens());
    
    // Fetch token prices from GMX
    token_registry.update_all_gmx_prices().await?;
    println!("✅ All GMX prices updated successfully!");

    // Fetch token prices from oracles
    token_registry.update_all_oracle_prices(&cfg).await?;
    println!("✅ All oracle prices updated successfully!");

    // Fetch GMX markets
    let market_props_vec = gmx::get_markets(&cfg).await?;
    println!("Fetched {} markets from GMX", market_props_vec.len());
    
    // Initialize and populate market registry
    let mut market_registry = market::MarketRegistry::new();
    market_registry.populate(&market_props_vec, &token_registry);
    println!("Populated market registry with {} markets", market_registry.num_markets());
    market_registry.print_markets();
    
    // Example: Fetch market info and market token price for the first market
    // let first_market = market_registry.get_market(&market_props_vec[0].market_token).expect("Market not found in registry");
    // let market_prices = first_market.market_prices().expect("Failed to get market prices");
    // let market_info = gmx::get_market_info(&cfg, first_market.market_token, market_prices.clone()).await?;
    // let market_token_price = gmx::get_market_token_price(&cfg, first_market.market_props(), market_prices.clone(), gmx::PnlFactorType::Deposit, true).await?;
    // println!("Market: {}", first_market);
    // println!("Market Info: {:#?}", market_info);
    // println!("Market Token Price: {:#?}", market_token_price);

    Ok(())
}