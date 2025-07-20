use dotenvy::dotenv;

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
    let portfolio_data = match engine::run_strategy_engine(&db).await {
        Some(data) => data,
        None => {
            info!("No portfolio data available");
            return Ok(());
        }
    };
    
    // Log basic diagnostics
    info!("Strategy engine completed with {} markets", portfolio_data.market_addresses.len());
    
    // Calculate Sharpe ratios and create sorted data
    let mut market_data: Vec<(usize, String, f64, f64, f64)> = portfolio_data.market_addresses
        .iter()
        .enumerate()
        .map(|(i, &addr)| {
            let expected_return = portfolio_data.expected_returns[i];
            let variance = portfolio_data.get_variance(addr).unwrap_or(0.0);
            let std_dev = variance.sqrt();
            let sharpe = if std_dev > 0.0 { expected_return / std_dev } else { 0.0 };
            
            (i, portfolio_data.display_names[i].clone(), expected_return * 10000.0, std_dev * 10000.0, sharpe)
        })
        .collect();
    
    // Sort by Sharpe ratio (descending)
    market_data.sort_by(|a, b| b.4.partial_cmp(&a.4).unwrap_or(std::cmp::Ordering::Equal));
    
    // Create formatted output
    let market_summary = market_data
        .iter()
        .map(|(_, name, return_pct, variance_pct, sharpe)| {
            format!(
                "{}: Return={:.8}bps, Vol={:.8}bps, Sharpe={:.3}",
                name, return_pct, variance_pct, sharpe
            )
        })
        .collect::<Vec<_>>()
        .join("\n  ");
    
    info!("Portfolio Analysis (sorted by Sharpe):\n  {}", market_summary);
    tokio::time::sleep(std::time::Duration::from_secs(3)).await; // Allow time for logging to flush

    Ok(())
}
