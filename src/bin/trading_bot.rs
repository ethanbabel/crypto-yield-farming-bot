use dotenvy::dotenv;
use tracing::{instrument, info};

use crypto_yield_farming_bot::logging;
use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::wallet::WalletManager;
use crypto_yield_farming_bot::db::db_manager::DbManager;
use crypto_yield_farming_bot::strategy::engine;

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
    info!("Database manager initialized");

    // Initialize and load wallet manager
    let mut wallet_manager = WalletManager::new(&cfg)?;
    wallet_manager.load_tokens(&db).await?;
    info!(address = ?wallet_manager.address, "Wallet manager initialized");

    // Log wallet token balances
    wallet_manager.log_all_balances(true).await?;

    // Run strategy engine
    let portfolio_data = match engine::run_strategy_engine(&db).await {
        Some(data) => data,
        None => {
            info!("No portfolio data available");
            return Ok(());
        }
    };
    
    // Log basic diagnostics
    info!("Strategy engine completed with {} markets", portfolio_data.market_addresses.len());
    portfolio_data.log_portfolio_data();
    
    tokio::time::sleep(std::time::Duration::from_secs(3)).await; // Allow time for logging to flush

    Ok(())
}
