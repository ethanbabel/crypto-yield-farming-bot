use dotenvy::dotenv;
use rust_decimal::Decimal;

use crypto_yield_farming_bot::logging;
use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::db::db_manager::DbManager;
use crypto_yield_farming_bot::strategy::engine;


use tracing::{info};


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

    // Run strategy engine
    let allocation_plan = engine::run_strategy_engine(&db).await;
    let mut diagnostics_vec: Vec<_> = allocation_plan.diagnostics
        .iter()
        .collect();
    diagnostics_vec.sort_by(|a, b| b.1.expected_return.partial_cmp(&a.1.expected_return).unwrap());
    
    let output = diagnostics_vec
        .iter()
        .map(|(_, diagnostics)| {
            format!(
                "Market: {:?}, Expected Return: {:.5}%, Variance: {:.5}%",
                diagnostics.display_name,
                diagnostics.expected_return * Decimal::from(100),
                diagnostics.variance * Decimal::from(100)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    info!("Strategy engine completed. Allocation plan:\n{}", output);

    tokio::time::sleep(std::time::Duration::from_secs(3)).await; // Allow time for logging to flush

    Ok(())
}
