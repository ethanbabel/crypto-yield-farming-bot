use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::logging;
use crypto_yield_farming_bot::gmx::event_listener::GmxEventListener;
use crypto_yield_farming_bot::data_ingestion::token::token_registry;
use crypto_yield_farming_bot::data_ingestion::market::market_registry;
use crypto_yield_farming_bot::db;

use tracing::{info, error, debug, instrument};
use dotenvy::dotenv;
use std::time::Duration;
use tokio::time::interval;
use std::sync::Arc;
use redis::AsyncCommands;

#[instrument(name = "data_collector_main")]
#[tokio::main]
async fn main() -> eyre::Result<()> {
    // Load environment variables from .env file
    dotenv()?;

    // Initialize logging
    if let Err(e) = logging::init_logging(env!("CARGO_BIN_NAME").to_string()) {
        eprintln!("Failed to initialize logging: {}", e);
        return Err(e.into());
    }

    // Load configuration (including provider)
    let cfg = config::Config::load().await;
    info!(network_mode = %cfg.network_mode, "Configuration loaded and logging initialized");

    // Initialize and populate token registry
    let mut token_registry = token_registry::AssetTokenRegistry::new(&cfg);
    if let Err(err) = token_registry.load_from_file() {
        error!(?err, "Failed to load asset tokens from file");
        return Err(err);
    }
    info!(count = token_registry.num_asset_tokens(), "Asset token registry initialized");

    // Initialize and populate market registry
    let mut market_registry = market_registry::MarketRegistry::new(&cfg);
    if let Err(err) = market_registry.populate(&cfg, &token_registry).await {
        error!(?err, "Failed to populate market registry");
        return Err(err);
    }
    info!(
        total_markets = market_registry.num_markets(),
        relevant_markets = market_registry.num_relevant_markets(),
        "Market registry populated"
    );

    // Initialize database manager
    let mut db = db::db_manager::DbManager::init(&cfg).await?;
    info!("Database manager initialized");

    // Sync tokens and markets with the database
    if let Err(e) = db.sync_tokens(token_registry.asset_tokens()).await {
        error!(?e, "Failed to sync tokens with database");
        return Err(e.into());
    }
    if let Err(e) = db.sync_markets(market_registry.all_markets()).await {
        error!(?e, "Failed to sync markets with database");
        return Err(e.into());
    }
    info!("Tokens and markets synchronized with database");

    // Create Redis client
    let redis_client = redis::Client::open("redis://redis:6379")?;
    let mut redis_connection = redis_client.get_multiplexed_async_connection().await?;
    info!("Redis connection established");

    // Initialize the GMX event listener
    let event_listener = GmxEventListener::init(
        cfg.alchemy_ws_url.clone(),
        cfg.gmx_eventemitter,
    );
    let fees_map = Arc::clone(&event_listener.fees);

    // Spawn the event listener in a background task
    tokio::spawn(async move {
        if let Err(e) = event_listener.start_listening().await {
            error!(?e, "GMX event listener failed");
        }
    });
    info!("GMX event listener started in background task");

    // Periodically update markets and save to database
    let mut ticker = interval(Duration::from_secs(300));
    info!("Starting main data collection loop with 300s interval");
    
    loop {
        ticker.tick().await;
        debug!("Data collection cycle started");
        
        // Repopulate the market registry and get new tokens/markets
        let (new_tokens, new_market_addresses) = match market_registry.repopulate(&cfg, &mut token_registry).await {
            Ok(result) => result,
            Err(e) => {
                error!(?e, "Failed to repopulate market registry");
                return Err(e);
            }
        };
        debug!("Market registry repopulated");

        // If we found new tokens or markets, send them to Redis streams
        if !new_tokens.is_empty() || !new_market_addresses.is_empty() {
            info!(
                new_token_count = new_tokens.len(),
                new_market_count = new_market_addresses.len(),
                "Detected new tokens/markets, sending to data recorder"
            );
            
            // Prepare and serialize new tokens using db_manager
            if !new_tokens.is_empty() {
                let new_token_models = db.prepare_new_tokens(&new_tokens);
                for token_model in &new_token_models {
                    if let Ok(serialized) = serde_json::to_string(token_model) {
                        let _: () = redis_connection.xadd("new_tokens", "*", &[("data", serialized)]).await?;
                    }
                }
            }
            
            // Get full market data for new market addresses and prepare models
            if !new_market_addresses.is_empty() {
                let mut new_markets = Vec::new();
                for &market_address in &new_market_addresses {
                    if let Some(market) = market_registry.get_market(&market_address) {
                        new_markets.push(market);
                    }
                }
                
                if !new_markets.is_empty() {
                    let new_market_models = db.prepare_new_markets(&new_markets).await;
                    for market_model in &new_market_models {
                        if let Ok(serialized) = serde_json::to_string(market_model) {
                            let _: () = redis_connection.xadd("new_markets", "*", &[("data", serialized)]).await?;
                        }
                    }
                }
            }
            
            debug!(
                new_tokens_sent = new_tokens.len(),
                new_markets_sent = new_market_addresses.len(),
                "New tokens and markets sent to Redis streams"
            );
        }

        // Fetch Asset Token price data from GMX
        if let Err(e) = token_registry.update_all_gmx_prices().await {
            error!(?e, "Failed to update asset token prices from GMX");
            return Err(e);
        }
        debug!("Asset token prices updated from GMX");

        // Fetch GMX fees
        let snapshot = {
            let mut map = fees_map.lock().await;
            let ss = map.clone();
            map.clear();
            ss
        };
        debug!(fee_markets = snapshot.len(), "Fee snapshot captured");

        // Update market data
        if let Err(e) = market_registry.update_all_market_data(Arc::clone(&cfg), &snapshot).await {
            error!(?e, "Failed to update market data");
            return Err(e);
        }
        market_registry.print_relevant_markets();

        // Get token_price and market_state models
        let token_prices = db.prepare_token_prices(token_registry.asset_tokens()).await;
        let market_states = db.prepare_market_states(market_registry.relevant_markets());

        // Serialize token_price and market_state models
        let serialized_token_prices: Vec<String> = token_prices
            .iter()
            .filter_map(|tp| serde_json::to_string(tp).ok())
            .collect();
        let serialized_market_states: Vec<String> = market_states
            .iter()
            .filter_map(|ms| serde_json::to_string(ms).ok())
            .collect();
        
        // Send token_price and market_state models to redis
        let token_count = serialized_token_prices.len();
        let market_count = serialized_market_states.len();
        
        // Publish pre-emptive coordination event with expected counts
        let message = format!("starting:{}:{}", token_count, market_count);
        let _: () = redis_connection.publish("data_collection_starting", message).await?;
        debug!(
            token_count = token_count,
            market_count = market_count,
            "Published data collection coordination event"
        );
        
        for tp in serialized_token_prices {
            let _: () = redis_connection.xadd("token_prices", "*", &[("data", tp)]).await?;
        }
        for ms in serialized_market_states {
            let _: () = redis_connection.xadd("market_states", "*", &[("data", ms)]).await?;
        }

        info!(
            token_count = token_count,
            market_count = market_count,
            "Data collection cycle completed"
        );

        // Zero out tracked fields for all markets at the end of the data collection loop
        market_registry.zero_all_tracked_fields();
    }
}