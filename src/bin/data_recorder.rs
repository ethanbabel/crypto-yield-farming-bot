use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::logging;
use crypto_yield_farming_bot::db::{
    self,
    models::{
        token_prices::NewTokenPriceModel,
        market_states::NewMarketStateModel,
    }
};

use tracing::{self, info, debug, error, warn, instrument};
use dotenvy::dotenv;
use redis::AsyncCommands;
use redis::streams::{StreamReadOptions, StreamReadReply};
use futures::StreamExt;
use std::collections::HashMap;
use tokio::time::{sleep, Duration};
use tokio::sync::mpsc;

#[instrument(skip(token_tx, market_tx), fields(stream_name, entry_count))]
async fn process_stream_entries(
    stream_name: &str,
    stream_entries: &[redis::streams::StreamId],
    token_tx: &mpsc::Sender<NewTokenPriceModel>,
    market_tx: &mpsc::Sender<NewMarketStateModel>,
    last_ids: &mut HashMap<String, String>,
) -> eyre::Result<()> {
    tracing::Span::current().record("stream_name", stream_name);
    tracing::Span::current().record("entry_count", stream_entries.len());
    
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
                            debug!(token_id = token_price_model.token_id, "Deserialized token price");
                            if let Err(e) = token_tx.send(token_price_model).await {
                                error!(error = ?e, "Token price channel closed");
                                return Err(eyre::eyre!("Token price channel closed"));
                            }
                        } else {
                            error!(data = %text, "Failed to deserialize token price data");
                        }
                    },
                    "market_states" => {
                        if let Ok(market_state_model) = serde_json::from_str::<NewMarketStateModel>(text) {
                            debug!(market_id = market_state_model.market_id, "Deserialized market state");
                            if let Err(e) = market_tx.send(market_state_model).await {
                                error!(error = ?e, "Market state channel closed");
                                return Err(eyre::eyre!("Market state channel closed"));
                            }
                        } else {
                            error!(data = %text, "Failed to deserialize market state data");
                        }
                    },
                    _ => {
                        warn!(stream_name = %stream_name, "Unknown stream");
                    }
                }
            }
        }
        last_ids.insert(stream_name.to_string(), stream_id.id.clone());
    }
    
    Ok(())
}

#[instrument]
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
    info!(network_mode = %cfg.network_mode, "Loaded configuration and initialized logging");

    // Initialize database manager
    let db = db::db_manager::DbManager::init(&cfg).await?;

    // Create Redis client
    let redis_client = redis::Client::open("redis://redis:6379")?;
    let mut redis_connection = redis_client.get_multiplexed_async_connection().await?;
    
    // Create channels for batching
    let (token_tx, mut token_rx) = mpsc::channel::<NewTokenPriceModel>(1000);
    let (market_tx, mut market_rx) = mpsc::channel::<NewMarketStateModel>(1000);

    info!("Starting database writer task and waiting for coordination signals");

    // Spawn database writer task
    tokio::spawn(async move {
        // Create PubSub connection inside the task
        let pubsub_client = redis::Client::open("redis://redis:6379").unwrap();
        let mut pubsub = pubsub_client.get_async_pubsub().await.unwrap();
        pubsub.subscribe("data_collection_starting").await.unwrap();
        info!("Subscribed to data_collection_starting channel");        
        let mut token_batch = Vec::new();
        let mut market_batch = Vec::new();
        let mut message_stream = pubsub.on_message();
        
        // Count-based coordination state
        let mut waiting_for_flush = false;
        let mut expected_tokens = None::<usize>;
        let mut expected_markets = None::<usize>;
        let mut tokens_processed_since_signal = 0usize;
        let mut markets_processed_since_signal = 0usize;
        
        loop {
            tokio::select! {
                // Collect tokens
                Some(token_price) = token_rx.recv() => {
                    token_batch.push(token_price);
                    tokens_processed_since_signal += 1; 
                    
                    // Safety flush if batch gets large
                    if token_batch.len() >= 200 {
                        if let Err(e) = db.insert_token_prices(std::mem::take(&mut token_batch)).await {
                            error!(error = ?e, "Failed to insert token prices batch");
                        } else {
                            info!("Flushed large token prices batch to database (safety flush)");
                        }
                    }
                }
                // Collect markets  
                Some(market_state) = market_rx.recv() => {
                    market_batch.push(market_state);
                    markets_processed_since_signal += 1; 
                    
                    // Safety flush if batch gets large
                    if market_batch.len() >= 200 {
                        if let Err(e) = db.insert_market_states(std::mem::take(&mut market_batch)).await {
                            error!(error = ?e, "Failed to insert market states batch");
                        } else {
                            info!("Flushed large market states batch to database (safety flush)");
                        }
                    }
                }
                // PubSub signal - set coordination expectations
                Some(message) = message_stream.next() => {
                    let channel: String = message.get_channel_name().to_string();
                    let payload: String = message.get_payload().unwrap_or_default();
                    
                    if channel == "data_collection_starting" {
                        // Parse the payload: "starting:token_count:market_count"
                        let parts: Vec<&str> = payload.split(':').collect();
                        if parts.len() == 3 && parts[0] == "starting" {
                            if let (Ok(token_count), Ok(market_count)) = (parts[1].parse::<usize>(), parts[2].parse::<usize>()) {
                                debug!(token_count, market_count, "Received data collection starting signal");
                                
                                expected_tokens = Some(token_count);
                                expected_markets = Some(market_count);
                                waiting_for_flush = true;
                            } else {
                                error!(payload = %payload, "Failed to parse token/market counts from payload");
                            }
                        } else {
                            warn!(channel = %channel, payload = %payload, "Received unexpected message format");
                        }
                    } else {
                        warn!(channel = %channel, "Received message on unexpected channel");
                    }
                }
                // Coordination flush - check if expected counts are met or exceeded
                _ = async {}, if waiting_for_flush && 
                                expected_tokens.is_some() && 
                                expected_markets.is_some() &&
                                tokens_processed_since_signal >= expected_tokens.unwrap() && 
                                markets_processed_since_signal >= expected_markets.unwrap() => {
                    debug!(
                        tokens_processed = tokens_processed_since_signal, 
                        markets_processed = markets_processed_since_signal,
                        "Processed expected counts, performing coordination flush"
                    );
                    
                    // Flush all batches (may contain more than expected if catching up from offline)
                    if !token_batch.is_empty() {
                        let count = token_batch.len();
                        if let Err(e) = db.insert_token_prices(std::mem::take(&mut token_batch)).await {
                            error!(error = ?e, "Failed to insert token prices");
                        } else {
                            info!(count, "Coordination flush: inserted token prices");
                        }
                    }
                    if !market_batch.is_empty() {
                        let count = market_batch.len();
                        if let Err(e) = db.insert_market_states(std::mem::take(&mut market_batch)).await {
                            error!(error = ?e, "Failed to insert market states");
                        } else {
                            info!(count, "Coordination flush: inserted market states");
                        }
                    }
                    
                    // Reset coordination state after successful flush
                    waiting_for_flush = false;
                    expected_tokens = None;
                    expected_markets = None;
                    tokens_processed_since_signal = 0;
                    markets_processed_since_signal = 0;
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

    info!("Starting Redis stream listener");
    loop {
        // Use explicit stream names and IDs for xread_options
        let reply: StreamReadReply = redis_connection
            .xread_options(
                &["token_prices", "market_states"],
                &[&last_ids["token_prices"], &last_ids["market_states"]],
                &stream_options,
            )
            .await?;

        debug!(stream_count = reply.keys.len(), "Received stream entries");

        // Iterate over reply.keys and reply.streams together
        for stream_key in reply.keys {
            let stream_name = stream_key.key.as_str();
            let stream_entries = stream_key.ids;
            
            if let Err(e) = process_stream_entries(
                stream_name,
                &stream_entries,
                &token_tx,
                &market_tx,
                &mut last_ids,
            ).await {
                error!(error = ?e, stream_name = %stream_name, "Failed to process stream entries");
                return Err(e);
            }
        }

        sleep(Duration::from_millis(100)).await;
    }
}