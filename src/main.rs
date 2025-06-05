use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::abi_fetcher;
use crypto_yield_farming_bot::token;
use crypto_yield_farming_bot::market;
use crypto_yield_farming_bot::logging;
use dotenvy::dotenv;


#[tokio::main]
async fn main() -> eyre::Result<()> {
    // Load environment variables from .env file
    dotenv().ok();

    // Initialize logging (console always, file if enabled, structured JSON)
    logging::init_logging();

    // Load configuration (including provider)
    let cfg = config::Config::load();

    tracing::info!(network_mode = %cfg.network_mode, "Loaded configuration and initialized logging");

    // Fetch ABIs
    abi_fetcher::fetch_all_abis(&cfg).await?;
    tracing::info!("All ABIs fetched successfully");

    // Initialize token registry
    let mut token_registry = token::AssetTokenRegistry::new(&cfg);
    // Load tokens from file
    token_registry.load_from_file()?;
    tracing::info!(count = token_registry.num_asset_tokens(), "Loaded asset tokens to registry");

    // Fetch token prices from GMX
    token_registry.update_all_gmx_prices().await?;
    tracing::info!("All GMX prices updated successfully");

    // Fetch token prices from oracles
    token_registry.update_all_oracle_prices(&cfg).await?;
    tracing::info!("All oracle prices updated successfully");

    // Wait until all token prices are fetched before proceeding
    let mut waited_secs = 0;
    while !token_registry.all_prices_fetched() {
        tracing::debug!(waited_secs, "Waiting for all token prices to be fetched");
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        waited_secs += 1;
    }
    tracing::info!("All token prices fetched in {} seconds", waited_secs);

    // Fetch GMX markets and populate market registry
    let mut market_registry = market::MarketRegistry::new();
    market_registry.populate(&cfg, &token_registry).await?;
    tracing::info!(count = market_registry.num_markets(), "Populated market registry");

    // Repopulate market registry
    market_registry.repopulate(&cfg, &mut token_registry).await?;
    tracing::info!(count = market_registry.num_markets(), "Repopulated market registry");

    // Update all markets with their data and calculate APRs
    market_registry.update_all_market_data(&cfg).await?;
    tracing::info!(relevant = market_registry.num_relevant_markets(), "All market data updated successfully");
    market_registry.calculate_all_aprs();
    tracing::info!("All APRs calculated successfully");
    market_registry.print_markets_by_apr_desc();
    

    Ok(())
}