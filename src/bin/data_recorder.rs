use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::logging;
use crypto_yield_farming_bot::db::{
    self,
    models::{
        token_prices::NewTokenPriceModel,
        market_states::NewMarketStateModel,
    }
};

use tracing;
use dotenvy::dotenv;
use redis::AsyncCommands;
use redis::streams::{StreamReadOptions, StreamReadReply};
use std::collections::HashMap;
use tokio::time::{sleep, Duration, interval};
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> eyre::Result<()> {
    // Load environment variables from .env file
    dotenv()?;

    // Initialize logging
    logging::init_logging();
    logging::set_panic_hook();

    // Load configuration (including provider)
    let cfg = config::Config::load().await;
    tracing::info!(network_mode = %cfg.network_mode, "Loaded configuration and initialized logging");

    // Initialize database manager
    let db = db::db_manager::DbManager::init(&cfg).await?;

    // Create Redis client
    let redis_client = redis::Client::open("redis://redis:6379")?;
    let mut redis_connection = redis_client.get_multiplexed_async_connection().await?;

    // Create channels for batching
    let (token_tx, mut token_rx) = mpsc::channel::<NewTokenPriceModel>(1000);
    let (market_tx, mut market_rx) = mpsc::channel::<NewMarketStateModel>(1000);

    // Wait a bit to let data_collector start and begin producing data
    tracing::info!("Waiting 90 seconds to offset from data_collector...");
    sleep(Duration::from_secs(90)).await;
    tracing::info!("Starting database writer task and Redis stream processing");

    // Spawn database writer task
    tokio::spawn(async move {
        let mut token_batch = Vec::new();
        let mut market_batch = Vec::new();
        let mut flush_interval = interval(Duration::from_secs(300)); // Same as data_collector frequency
        
        loop {
            tokio::select! {
                // Collect tokens
                Some(token_price) = token_rx.recv() => {
                    token_batch.push(token_price);
                    // Flush immediately if batch gets large
                    if token_batch.len() >= 100 {
                        if let Err(e) = db.insert_token_prices(std::mem::take(&mut token_batch)).await {
                            tracing::error!("Failed to insert token prices batch: {:?}", e);
                        } else {
                            tracing::info!("Flushed large token prices batch to database");
                        }
                    }
                }
                // Collect markets  
                Some(market_state) = market_rx.recv() => {
                    market_batch.push(market_state);
                    // Flush immediately if batch gets large
                    if market_batch.len() >= 100 {
                        if let Err(e) = db.insert_market_states(std::mem::take(&mut market_batch)).await {
                            tracing::error!("Failed to insert market states batch: {:?}", e);
                        } else {
                            tracing::info!("Flushed large market states batch to database");
                        }
                    }
                }
                // Periodic flush
                _ = flush_interval.tick() => {
                    if !token_batch.is_empty() {
                        let count = token_batch.len();
                        if let Err(e) = db.insert_token_prices(std::mem::take(&mut token_batch)).await {
                            tracing::error!("Failed to insert token prices: {:?}", e);
                        } else {
                            tracing::info!("Periodic flush: inserted {} token prices", count);
                        }
                    }
                    if !market_batch.is_empty() {
                        let count = market_batch.len();
                        if let Err(e) = db.insert_market_states(std::mem::take(&mut market_batch)).await {
                            tracing::error!("Failed to insert market states: {:?}", e);
                        } else {
                            tracing::info!("Periodic flush: inserted {} market states", count);
                        }
                    }
                }
            }
        }
    });

    // Perpetual loop to listen for new stream entries
    let stream_options = StreamReadOptions::default().block(0).count(10);
    let mut last_ids = HashMap::from([
        ("token_prices".to_string(), "0".to_string()),
        ("market_states".to_string(), "0".to_string()),
    ]);

    loop {
        // Use explicit stream names and IDs for xread_options
        let reply: StreamReadReply = redis_connection
            .xread_options(
                &["token_prices", "market_states"],
                &[&last_ids["token_prices"], &last_ids["market_states"]],
                &stream_options,
            )
            .await?;

        // Iterate over reply.keys and reply.streams together
        for stream_key in reply.keys {
            let stream_name = stream_key.key.as_str();
            let stream_entries = stream_key.ids;
            for stream_id in stream_entries {
                let data = &stream_id.map;
                // Use redis::Value::BulkString for the payload
                if let Some(redis::Value::BulkString(payload)) = data.get("data") {
                    // Try to convert payload to string for printing
                    if let Ok(text) = std::str::from_utf8(payload) {
                        // Deserialize based on stream name
                        match stream_name {
                            "token_prices" => {
                                if let Ok(token_price_model) = serde_json::from_str::<NewTokenPriceModel>(text) {
                                    tracing::debug!("Deserialized token price: {:?}", token_price_model);
                                    if let Err(_) = token_tx.send(token_price_model).await {
                                        tracing::error!("Token price channel closed");
                                        return Ok(());
                                    }
                                } else {
                                    tracing::error!("Failed to deserialize token price data: {}", text);
                                }
                            },
                            "market_states" => {
                                if let Ok(market_state_model) = serde_json::from_str::<NewMarketStateModel>(text) {
                                    tracing::debug!("Deserialized market state: {:?}", market_state_model);
                                    if let Err(_) = market_tx.send(market_state_model).await {
                                        tracing::error!("Market state channel closed");
                                        return Ok(());
                                    }
                                } else {
                                    tracing::error!("Failed to deserialize market state data: {}", text);
                                }
                            },
                            _ => {
                                tracing::warn!("Unknown stream: {}", stream_name);
                            }
                        }
                    }
                }
                last_ids.insert(stream_name.to_string(), stream_id.id.clone());
            }
        }

        sleep(Duration::from_millis(100)).await;
    }
}