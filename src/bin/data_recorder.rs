use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::logging;
use crypto_yield_farming_bot::gmx::event_listener::GmxEventListener;
use crypto_yield_farming_bot::token;
use crypto_yield_farming_bot::market;

use tracing;
use dotenvy::dotenv;
use std::time::Duration;
use tokio::time::interval;
use std::sync::Arc;
use tokio::sync::Mutex;

#[tokio::main]
async fn main() -> eyre::Result<()> {

    // Load environment variables from .env file
    dotenv().ok();

    // Initialize logging
    logging::init_logging();
    logging::set_panic_hook();

    // Load configuration (including provider)
    let cfg = config::Config::load().await;
    tracing::info!(network_mode = %cfg.network_mode, "Loaded configuration and initialized logging");

    // Initialize token registry
    let mut token_registry = token::AssetTokenRegistry::new(&cfg);
    // Load tokens from file
    if let Err(err) = token_registry.load_from_file() {
        tracing::error!(?err, "Failed to load asset tokens from file");
        return Err(err);
    }
    tracing::info!(count = token_registry.num_asset_tokens(), "Loaded asset tokens to registry");

    // Fetch token prices from GMX
    if let Err(err) = token_registry.update_all_gmx_prices().await {
        tracing::error!(?err, "Failed to update GMX prices");
        return Err(err);
    }
    tracing::info!("All GMX prices updated successfully");

    // Wait until all token prices are fetched before proceeding
    while !token_registry.all_prices_fetched() {
        tracing::debug!("Waiting for all token prices to be fetched");
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    // Fetch GMX markets and populate market registry
    let mut market_registry = market::MarketRegistry::new(&cfg);
    if let Err(err) = market_registry.populate(&cfg, &token_registry).await {
        tracing::error!(?err, "Failed to populate market registry");
        return Err(err);
    }
    tracing::info!(count = market_registry.num_markets(), "Populated market registry");

    // Initialize the GMX event listener
    let event_listener = GmxEventListener::init(
        cfg.alchemy_ws_provider.clone(),
        cfg.gmx_eventemitter,
    );
    let fees_map = Arc::clone(&event_listener.fees);

    // Spawn the event listener in a background task
    tokio::spawn(async move {
        if let Err(e) = event_listener.start_listening().await {
            tracing::error!("Event listener failed: {:?}", e);
        }
    });

    // Periodically update markets
    let mut ticker = interval(Duration::from_secs(60));
    loop {
        ticker.tick().await;
        let snapshot = {
            let map = fees_map.lock().await;
            map.clone()
        };
        tracing::info!("Updating market data with snapshot: {:?}", snapshot);
        if let Err(e) = market_registry.update_all_market_data(&cfg, &snapshot).await {
            tracing::error!(?e, "Failed to update market data");
            return Err(e);
        }
        market_registry.print_relevant_markets();
    }
}