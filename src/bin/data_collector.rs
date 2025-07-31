use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::logging;
use crypto_yield_farming_bot::gmx::event_fetcher::GmxEventFetcher;
use crypto_yield_farming_bot::data_ingestion::token::{
    token_registry,
    token::AssetToken,
};
use crypto_yield_farming_bot::data_ingestion::market::{
    market_registry,
    market::Market,
};
use crypto_yield_farming_bot::db;

use tracing::{info, error, debug, instrument};
use dotenvy::dotenv;
use std::time::Duration;
use std::sync::Arc;
use std::collections::HashMap;
use tokio::time::interval;
use tokio::sync::RwLock;
use ethers::types::Address;
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

    // Initialize token registry
    let mut token_registry = token_registry::AssetTokenRegistry::new(&cfg);
    info!("Asset token registry initialized");

    // Initialize market registry
    let mut market_registry = market_registry::MarketRegistry::new(&cfg);
    info!("Market registry initialized");

    // Initialize database manager
    let mut db = db::db_manager::DbManager::init(&cfg).await?;
    info!("Database manager initialized");

    // Initialize failed tokens and markets tracking
    let mut markets_retry_bank: HashMap<Address, Market> = HashMap::new();
    let mut token_prices_retry_bank: HashMap<Address, (Vec<AssetToken>, u32)> = HashMap::new();
    let mut market_states_retry_bank: HashMap<Address, (Vec<Market>, u32)> = HashMap::new();

    // Create Redis client
    let redis_client = redis::Client::open("redis://redis:6379")?;
    let mut redis_connection = redis_client.get_multiplexed_async_connection().await?;
    info!("Redis connection established");

    // Initialize the GMX event fetcher
    let mut event_fetcher = GmxEventFetcher::init(
        Arc::clone(&cfg.alchemy_provider),
        cfg.gmx_eventemitter,
    );
    info!("GMX event fetcher initialized");

    // Periodically update markets and save to database
    let mut ticker = interval(Duration::from_secs(300));
    info!("Starting main data collection loop with 300s interval");
    
    loop {
        ticker.tick().await;
        info!("Data collection cycle started");
        
        // Repopulate the market registry and get new tokens/markets
        let (new_tokens, new_market_addresses) = match market_registry.repopulate(&cfg, &mut token_registry).await {
            Ok(result) => result,
            Err(e) => {
                error!(?e, "Failed to repopulate market registry");
                return Err(e);
            }
        };

        // If we found new tokens or markets, send them to Redis streams
        if !new_tokens.is_empty() || !new_market_addresses.is_empty() || !markets_retry_bank.is_empty() {
            info!(
                new_token_count = new_tokens.len(),
                new_market_count = new_market_addresses.len(),
                retry_failed_market_count = markets_retry_bank.len(),
                new_tokens = ?new_tokens.iter().map(|t| t.symbol.clone()).collect::<Vec<_>>(),
                new_markets = ?new_market_addresses,
                retry_failed_markets = ?markets_retry_bank.keys().cloned().collect::<Vec<_>>(),
                "Detected new tokens/markets"
            );
            
            // Prepare and serialize new tokens using db_manager
            if !new_tokens.is_empty() {
                let new_token_models = match db.prepare_new_tokens(&new_tokens).await {
                    Ok(models) => models,
                    Err(e) => {
                        error!(?e, "Failed to prepare new token models");
                        return Err(e.into());
                    }
                };
                for token_model in &new_token_models {
                    if let Ok(serialized) = serde_json::to_string(token_model) {
                        let _: () = redis_connection.xadd("new_tokens", "*", &[("data", serialized)]).await?;
                    }
                    debug!(
                        token_address = %token_model.address, 
                        token_symbol = %token_model.symbol,
                        "New token model serialized and sent through Redis"
                    );
                }
            }
            
            // Get full market data for new market addresses and prepare models
            if !new_market_addresses.is_empty() || !markets_retry_bank.is_empty() {
                let mut new_markets = Vec::new();
                for &market_address in &new_market_addresses {
                    if let Some(market) = market_registry.get_market(&market_address) {
                        new_markets.push(market);
                    }
                }
                // Add markets from retry bank
                for market in markets_retry_bank.values() {
                    new_markets.push(&market);

                }
                
                if !new_markets.is_empty() {
                    let (new_market_models, new_failed_markets) = match db.prepare_new_markets(&new_markets).await {
                        Ok(result) => result,
                        Err(e) => {
                            error!(?e, "Failed to prepare new market models");
                            return Err(e.into());
                        }
                    };
                    for market_model in &new_market_models {
                        if let Ok(serialized) = serde_json::to_string(market_model) {
                            let _: () = redis_connection.xadd("new_markets", "*", &[("data", serialized)]).await?;
                        }
                        debug!(
                            market_address = %market_model.address,
                            "New market model serialized and sent through Redis"
                        );
                    }
                    // Add failed markets to retry bank
                    markets_retry_bank.clear();
                    for market in new_failed_markets {
                        let market_token = market.market_token;
                        markets_retry_bank.insert(market_token, market);
                        info!(market_token = ?market_token, "Market added to retry bank due to missing token IDs");
                    }
                }
            }
        }

        // Fetch Asset Token price data from GMX
        if let Err(e) = token_registry.update_all_gmx_prices().await {
            error!(?e, "Failed to update asset token prices from GMX");
            return Err(e);
        }
        debug!("Asset token prices updated from GMX");

        // Fetch GMX fees
        let fees_snapshot = match event_fetcher.fetch_fees().await {
            Ok(fees) => fees,
            Err(e) => {
                error!(?e, "Failed to fetch GMX fees");
                return Err(e);
            }
        };
        debug!(fee_markets = fees_snapshot.len(), "Fee snapshot captured");

        // Update market data
        if let Err(e) = market_registry.update_all_market_data(Arc::clone(&cfg), &fees_snapshot).await {
            error!(?e, "Failed to update market data");
            return Err(e);
        }

        // Get token_price models (new)
        let (mut token_prices, new_failed_token_prices) = match db.prepare_token_prices(token_registry.asset_tokens()).await {
            Ok(result) => result,
            Err(e) => {
                error!(?e, "Failed to prepare token price models");
                return Err(e.into());
            }
        };
        info!(
            new_token_prices_count = token_prices.len(),
            new_failed_token_prices_count = new_failed_token_prices.len(),
            new_failed_token_prices = ?new_failed_token_prices.iter().map(|t| t.symbol.clone()).collect::<Vec<_>>(),
            "Token price models prepared"
        );

        // Get market_state models (new)
        let (mut market_states, new_failed_market_states) = match db.prepare_market_states(market_registry.relevant_markets()).await {
            Ok(result) => result,
            Err(e) => {
                error!(?e, "Failed to prepare market state models");
                return Err(e.into());
            }
        };
        info!(
            new_market_states_count = market_states.len(),
            new_failed_market_states_count = new_failed_market_states.len(),
            new_failed_market_states = ?new_failed_market_states.iter().map(|m| m.market_token).collect::<Vec<_>>(),
            "Market state models prepared"
        );

        // Get token_price models (retry)
        if !token_prices_retry_bank.is_empty() {
            let token_prices_to_retry: Vec<_> = token_prices_retry_bank.values()
                .flat_map(|(token_vec, _)| token_vec.iter().map(|t| Arc::new(RwLock::new(t.clone()))))
                .collect();
            let (retry_token_prices, retry_failed_token_prices) = match db.prepare_token_prices(token_prices_to_retry).await {
                Ok(result) => result,
                Err(e) => {
                    error!(?e, "Failed to prepare token price models for retry");
                    return Err(e.into());
                }
            };
            info!(
                retry_succeeded_token_prices_count = retry_token_prices.len(),
                retry_failed_token_prices_count = retry_failed_token_prices.len(),
                retry_failed_token_prices = ?retry_failed_token_prices.iter().map(|t| t.symbol.clone()).collect::<Vec<_>>(),
                "Retry token price models prepared"
            );
            // Extend the main token prices with successfully retried tokens
            token_prices.extend(retry_token_prices);

            // Update retry bank based on success/failure
            let mut addresses_to_remove = Vec::new();
            for (&address, (_tokens, retry_count)) in token_prices_retry_bank.iter_mut() {
                let any_succeeded = retry_failed_token_prices.iter().all(|failed_token| failed_token.address != address);
                
                if any_succeeded {
                    // Token succeeded, remove from retry bank
                    addresses_to_remove.push(address);
                } else {
                    // Token failed, increment retry count
                    *retry_count += 1;
                    if *retry_count >= 10 {
                        error!(token_address = %address, "Token failed to update after 10 retries, removing from retry bank");
                        addresses_to_remove.push(address);
                    }
                }
            }
            
            // Remove successful and max-retry tokens from retry bank
            for address in addresses_to_remove {
                token_prices_retry_bank.remove(&address);
            }
        }

        // Get market_state models (retry)
        if !market_states_retry_bank.is_empty() {
            let market_states_to_retry: Vec<_> = market_states_retry_bank.values()
                .flat_map(|(market_vec, _)| market_vec.iter())
                .collect();
            let (retry_market_states, retry_failed_market_states) = match db.prepare_market_states(market_states_to_retry).await {
                Ok(result) => result,
                Err(e) => {
                    error!(?e, "Failed to prepare market state models for retry");
                    return Err(e.into());
                }
            };
            info!(
                retry_succeeded_market_states_count = retry_market_states.len(),
                retry_failed_market_states_count = retry_failed_market_states.len(),
                retry_failed_market_states = ?retry_failed_market_states.iter().map(|m| m.market_token).collect::<Vec<_>>(),
                "Retry market state models prepared"
            );
            // Extend the main market states with successfully retried markets
            market_states.extend(retry_market_states);

            // Update retry bank based on success/failure
            let mut addresses_to_remove = Vec::new();
            for (&address, (_markets, retry_count)) in market_states_retry_bank.iter_mut() {
                let any_succeeded = retry_failed_market_states.iter().all(|failed_market| failed_market.market_token != address);
                
                if any_succeeded {
                    // Market succeeded, remove from retry bank
                    addresses_to_remove.push(address);
                } else {
                    // Market failed, increment retry count
                    *retry_count += 1;
                    if *retry_count >= 10 {
                        error!(market_token = %address, "Market failed to update after 10 retries, removing from retry bank");
                        addresses_to_remove.push(address);
                    }
                }
            }
            
            // Remove successful and max-retry markets from retry bank
            for address in addresses_to_remove {
                market_states_retry_bank.remove(&address);
            }
        }

        // Add new failed tokens and markets to retry banks
        for token_price in new_failed_token_prices {
            let token_address = token_price.address; // Store address before move
            if token_prices_retry_bank.contains_key(&token_address) {
                let entry = token_prices_retry_bank.get_mut(&token_address).unwrap();
                entry.0.push(token_price);
            } else {
                token_prices_retry_bank.insert(token_address, (vec![token_price], 1));
            }
        }
        for market_state in new_failed_market_states {
            let market_token = market_state.market_token; // Store address before move
            if market_states_retry_bank.contains_key(&market_token) {
                let entry = market_states_retry_bank.get_mut(&market_token).unwrap();
                entry.0.push(market_state);
            } else {
                market_states_retry_bank.insert(market_token, (vec![market_state], 1));
            }
        }

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