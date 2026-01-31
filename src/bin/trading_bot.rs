use dotenvy::dotenv;
use futures::StreamExt;
use std::sync::Arc;
use tracing::{instrument, info};

use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::db::db_manager::DbManager;
use crypto_yield_farming_bot::execution::engine::ExecutionEngine;
use crypto_yield_farming_bot::execution::types::ExecutionMode;
use crypto_yield_farming_bot::hedging::dydx_client::DydxClient;
use crypto_yield_farming_bot::logging;
use crypto_yield_farming_bot::strategy::engine;
use crypto_yield_farming_bot::wallet::WalletManager;


#[instrument(name = "trading_bot_main")]
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

    // Initialize db manager
    let db = DbManager::init(&cfg).await?;
    let db = Arc::new(db);
    info!("Database manager initialized");

    // Initialize and load wallet manager
    let mut wallet_manager = WalletManager::new(&cfg)?;
    wallet_manager.load_tokens(&db).await?;
    let wallet_manager = Arc::new(wallet_manager);
    info!("Wallet manager initialized");

    // Initialize dydx client
    let dydx_client = DydxClient::new(cfg.clone(), wallet_manager.clone()).await?;
    let dydx_client = Arc::new(tokio::sync::Mutex::new(dydx_client));
    info!("dYdX client initialized");

    let execution_mode = ExecutionMode::from_str(&cfg.execution_mode).unwrap_or(ExecutionMode::Paper);
    let execution_engine = ExecutionEngine::new(
        cfg.clone(),
        db.clone(),
        wallet_manager.clone(),
        dydx_client.clone(),
        execution_mode,
    );
    info!(mode = %execution_mode.as_str(), "Execution engine initialized");

    // Subscribe to data collection completion signal
    let redis_client = redis::Client::open("redis://redis:6379")?;
    let mut pubsub = redis_client.get_async_pubsub().await?;
    pubsub.subscribe("data_collection_completed").await?;
    let mut messages = pubsub.on_message();

    info!("Waiting for data_collection_completed signals");
    while let Some(msg) = messages.next().await {
        let payload: String = msg.get_payload().unwrap_or_default();
        info!(payload = %payload, "Received data collection completion signal");

        let portfolio_data = engine::run_strategy_engine(db.clone(), dydx_client.clone()).await?;
        info!(
            "Strategy engine completed with {} markets",
            portfolio_data.market_addresses.len()
        );
        portfolio_data.log_portfolio_data();

        if let Err(e) = execution_engine.run_once(&portfolio_data).await {
            eprintln!("Execution engine failed: {}", e);
        }
    }

    Ok(())
}
