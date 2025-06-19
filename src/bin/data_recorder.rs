use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::logging;
use crypto_yield_farming_bot::gmx::event_listener::GmxEventListener;
use crypto_yield_farming_bot::token;
use crypto_yield_farming_bot::market;
use crypto_yield_farming_bot::db;

use tracing;
use dotenvy::dotenv;
use std::time::Duration;
use tokio::time::interval;
use std::sync::Arc;
// use tokio::sync::Mutex;

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

    // Initialize and populate token registry
    let mut token_registry = token::AssetTokenRegistry::new(&cfg);
    if let Err(err) = token_registry.load_from_file() {
        tracing::error!(?err, "Failed to load asset tokens from file");
        return Err(err);
    }
    tracing::info!(count = token_registry.num_asset_tokens(), "Loaded asset tokens to registry");

    // Initialize and populate market registry
    let mut market_registry = market::MarketRegistry::new(&cfg);
    if let Err(err) = market_registry.populate(&cfg, &token_registry).await {
        tracing::error!(?err, "Failed to populate market registry");
        return Err(err);
    }
    tracing::info!(count = market_registry.num_markets(), "Populated market registry");

    // Initialize database
    let mut db =  db::db_manager::DbManager::init(&cfg).await?;

    // Sync tokens and markets with the database
    if let Err(e) = db.sync_tokens(token_registry.asset_tokens()).await {
        tracing::error!(?e, "Failed to sync tokens with the database");
        return Err(e.into());
    }
    if let Err(e) = db.sync_markets(market_registry.all_markets()).await {
        tracing::error!(?e, "Failed to sync markets with the database");
        return Err(e.into());
    }
    tracing::info!("Synchronized tokens and markets with the database");

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
    tracing::info!("Started GMX event listener");

    // Periodically update markets and save to database
    let mut ticker = interval(Duration::from_secs(300));
    loop {
        ticker.tick().await;
        
        // Repopulate the market registry 
        if let Err(e) = market_registry.repopulate(&cfg, &mut token_registry).await {
            tracing::error!(?e, "Failed to repopulate market registry");
            return Err(e);
        }

        // Fetch Asset Token price data from GMX
        if let Err(e) = token_registry.update_all_gmx_prices().await {
            tracing::error!(?e, "Failed to update asset token prices from GMX");
            return Err(e);
        }
        tracing::info!("Updated asset token prices from GMX");

        // Fetch GMX fees
        let snapshot = {
            let mut map = fees_map.lock().await;
            let ss = map.clone();
            map.clear();
            ss
        };

        // Update market data
        if let Err(e) = market_registry.update_all_market_data(Arc::clone(&cfg), &snapshot).await {
            tracing::error!(?e, "Failed to update market data");
            return Err(e);
        }
        market_registry.print_relevant_markets();

        // Save token prices and market states to database
        if let Err(e) = db.insert_token_prices(token_registry.asset_tokens()).await {
            tracing::error!(?e, "Failed to insert token prices into the database");
            return Err(e.into());
        }
        if let Err(e) = db.insert_market_states(market_registry.relevant_markets()).await {
            tracing::error!(?e, "Failed to insert market states into the database");
            return Err(e.into());
        }
        tracing::info!("Updated market data and saved to database");
    }
}