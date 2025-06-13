use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::logging;
use crypto_yield_farming_bot::gmx::event_listener::GmxEventListener;

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

    // Periodically print the cloned fees map
    let mut ticker = interval(Duration::from_secs(60));
    loop {
        ticker.tick().await;
        let snapshot = {
            let map = fees_map.lock().await;
            map.clone()
        };
        tracing::info!("Current fees snapshot: {:?}", snapshot);
    }
}
