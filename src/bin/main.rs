use dotenvy::dotenv;

use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::logging;
use crypto_yield_farming_bot::db;
use crypto_yield_farming_bot::token::token_registry;
use crypto_yield_farming_bot::market::market_registry;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // Load environment variables from .env file
    dotenv().ok();

    // Initialize logging
    logging::init_logging();
    // Set panic hook to log panics
    logging::set_panic_hook();

    // Load configuration (including provider)
    let cfg = config::Config::load().await;
    tracing::info!(network_mode = %cfg.network_mode, "Loaded configuration and initialized logging");

    // Initialize token registry
    let mut token_registry = token_registry::AssetTokenRegistry::new(&cfg);
    if let Err(err) = token_registry.load_from_file() {
        tracing::error!(?err, "Failed to load asset tokens from file");
        return Err(err);
    }
    tracing::info!(count = token_registry.num_asset_tokens(), "Loaded asset tokens to registry");

    // Initialize market registry
    let mut market_registry = market_registry::MarketRegistry::new(&cfg);
    if let Err(err) = market_registry.populate(&cfg, &token_registry).await {
        tracing::error!(?err, "Failed to populate market registry");
        return Err(err);
    }
    tracing::info!(count = market_registry.num_markets(), "Populated market registry");

    // Initialize database
    let mut db =  db::db_manager::DbManager::init(&cfg).await?;

    // Sync tokens and markets with the database
    db.sync_tokens(token_registry.asset_tokens()).await?;
    db.sync_markets(market_registry.all_markets()).await?;
    tracing::info!("Synchronized tokens and markets with the database");

    // Print token and market id maps length
    tracing::info!(
        token_id_map_length = db.token_id_map.len(),
        market_id_map_length = db.market_id_map.len(),
        "Token and market ID maps loaded"
    );

    Ok(())
}