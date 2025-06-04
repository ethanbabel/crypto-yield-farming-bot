use tracing_subscriber::FmtSubscriber;

use crypto_yield_farming_bot::config;
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

    // Print network mode (test or prod)
    println!("Running in {} mode", cfg.network_mode);

    // Fetch ABIs
    abi_fetcher::fetch_all_abis(&cfg).await?;
    println!("✅ All ABIs fetched successfully!");

    // Initialize token registry
    let mut token_registry = token::AssetTokenRegistry::new(&cfg);

    // Load tokens from file
    token_registry.load_from_file()?;
    println!("Loaded {} asset tokens to registry", token_registry.num_asset_tokens());

    // Update token registry and token data file with latest supported tokens
    token_registry.update_tracked_tokens().await?;
    
    // Fetch token prices from GMX
    token_registry.update_all_gmx_prices().await?;
    println!("✅ All GMX prices updated successfully!");

    // Fetch token prices from oracles
    token_registry.update_all_oracle_prices(&cfg).await?;
    println!("✅ All oracle prices updated successfully!");

    // Wait until all token prices are fetched before proceeding
    while !token_registry.all_prices_fetched() {
        println!("Waiting for all token prices to be fetched...");
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    // Fetch GMX markets and populate market registry
    let mut market_registry = market::MarketRegistry::new();
    market_registry.populate(&cfg, &token_registry).await?;
    println!("Populated market registry with {} markets", market_registry.num_markets());

    // Update all markets with their data and calculate APRs
    market_registry.update_all_market_data(&cfg).await?;
    println!("✅ All market data updated successfully! Total Relevant Markets: {}", market_registry.num_relevant_markets());
    market_registry.calculate_all_aprs();
    println!("✅ All APRs calculated successfully!");
    market_registry.print_markets_by_apr_desc();

    Ok(())
}