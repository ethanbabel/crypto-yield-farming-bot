use dotenvy::dotenv;
use eyre::Result;
use tracing::{info};

use crypto_yield_farming_bot::logging;
use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::db::{
    connection,
    schema,
};

#[tokio::main]
async fn main() -> Result<()> {
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

    // Initialize database connection pool
    let pool = connection::create_pool(&cfg).await?;
    info!("Database connection pool created");

    // Initialize database schema
    schema::init_schema(&pool).await?;
    info!("Database schema initialized");

    tokio::time::sleep(std::time::Duration::from_secs(3)).await; // Allow time for logging to flush
    Ok(())
}
