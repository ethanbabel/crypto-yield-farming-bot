use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::logging;
use crypto_yield_farming_bot::db::{
    self,
    models::{
        token_prices::RawTokenPriceModel,
        market_states::RawMarketStateModel,
        tokens::RawTokenModel,
        markets::RawMarketModel,
        dydx_perps::RawDydxPerpModel,
        dydx_perp_states::RawDydxPerpStateModel,
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

#[instrument(skip(token_prices_tx, market_states_tx, new_token_tx, new_market_tx, redis_connection), fields(stream_name, entry_count))]
async fn process_stream_entries(
    stream_name: &str,
    stream_entries: &[redis::streams::StreamId],
    token_prices_tx: &mpsc::Sender<RawTokenPriceModel>,
    market_states_tx: &mpsc::Sender<RawMarketStateModel>,
    new_token_tx: &mpsc::Sender<RawTokenModel>,
    new_market_tx: &mpsc::Sender<RawMarketModel>,
    dydx_perps_tx: &mpsc::Sender<RawDydxPerpModel>,
    dydx_perp_states_tx: &mpsc::Sender<RawDydxPerpStateModel>,
    last_ids: &mut HashMap<String, String>,
    redis_connection: &mut redis::aio::MultiplexedConnection,
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
                        if let Ok(raw_token_price_model) = serde_json::from_str::<RawTokenPriceModel>(text) {
                            debug!(token_address = raw_token_price_model.token_address, "Deserialized token price");
                            if let Err(e) = token_prices_tx.send(raw_token_price_model).await {
                                error!(error = ?e, "Token price channel closed");
                                return Err(eyre::eyre!("Token price channel closed"));
                            }
                        } else {
                            error!(data = %text, "Failed to deserialize token price data");
                        }
                    },
                    "market_states" => {
                        if let Ok(raw_market_state_model) = serde_json::from_str::<RawMarketStateModel>(text) {
                            debug!(market_address = raw_market_state_model.market_address, "Deserialized market state");
                            if let Err(e) = market_states_tx.send(raw_market_state_model).await {
                                error!(error = ?e, "Market state channel closed");
                                return Err(eyre::eyre!("Market state channel closed"));
                            }
                        } else {
                            error!(data = %text, "Failed to deserialize market state data");
                        }
                    },
                    "new_tokens" => {
                        if let Ok(raw_new_token_model) = serde_json::from_str::<RawTokenModel>(text) {
                            debug!(token_symbol = %raw_new_token_model.symbol, "Deserialized new token");
                            if let Err(e) = new_token_tx.send(raw_new_token_model).await {
                                error!(error = ?e, "New token channel closed");
                                return Err(eyre::eyre!("New token channel closed"));
                            }
                        } else {
                            error!(data = %text, "Failed to deserialize new token data");
                        }
                    },
                    "new_markets" => {
                        if let Ok(raw_new_market_model) = serde_json::from_str::<RawMarketModel>(text) {
                            debug!(market_address = %raw_new_market_model.address, "Deserialized new market");
                            if let Err(e) = new_market_tx.send(raw_new_market_model).await {
                                error!(error = ?e, "New market channel closed");
                                return Err(eyre::eyre!("New market channel closed"));
                            }
                        } else {
                            error!(data = %text, "Failed to deserialize new market data");
                        }
                    },
                    "dydx_perps" => {
                        if let Ok(raw_dydx_perp_model) = serde_json::from_str::<RawDydxPerpModel>(text) {
                            debug!(ticker = %raw_dydx_perp_model.ticker, "Deserialized dYdX perp");
                            if let Err(e) = dydx_perps_tx.send(raw_dydx_perp_model).await {
                                error!(error = ?e, "dYdX perps channel closed");
                                return Err(eyre::eyre!("dYdX perps channel closed"));
                            }
                        } else {
                            error!(data = %text, "Failed to deserialize dYdX perp data");
                        }
                    },
                    "dydx_perp_states" => {
                        if let Ok(raw_dydx_perp_state_model) = serde_json::from_str::<RawDydxPerpStateModel>(text) {
                            debug!(ticker = %raw_dydx_perp_state_model.ticker, "Deserialized dYdX perp state");
                            if let Err(e) = dydx_perp_states_tx.send(raw_dydx_perp_state_model).await {
                                error!(error = ?e, "dYdX perp states channel closed");
                                return Err(eyre::eyre!("dYdX perp states channel closed"));
                            }
                        } else {
                            error!(data = %text, "Failed to deserialize dYdX perp state data");
                        }
                    },
                    _ => {
                        warn!(stream_name = %stream_name, "Unknown stream");
                    }
                }
            }
        }
        last_ids.insert(stream_name.to_string(), stream_id.id.clone());
        
        // Persist the last processed ID to Redis
        let key = format!("data_recorder:last_id:{}", stream_name);
        let _: () = redis_connection.set(&key, &stream_id.id).await?;
    }
    
    Ok(())
}

#[instrument(name = "data_recorder_main")]
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
    let mut db = db::db_manager::DbManager::init(&cfg).await?;

    // Create Redis client
    let redis_client = redis::Client::open("redis://redis:6379")?;
    let mut redis_connection = redis_client.get_multiplexed_async_connection().await?;

    // Clone Redis client for the spawned task
    let redis_client_for_task = redis_client.clone();
    
    // Create channels for batching
    let (token_prices_tx, mut token_prices_rx) = mpsc::channel::<RawTokenPriceModel>(1000);
    let (market_states_tx, mut market_states_rx) = mpsc::channel::<RawMarketStateModel>(1000);
    let (new_token_tx, mut new_token_rx) = mpsc::channel::<RawTokenModel>(100);
    let (new_market_tx, mut new_market_rx) = mpsc::channel::<RawMarketModel>(100);
    let (dydx_perps_tx, mut dydx_perps_rx) = mpsc::channel::<RawDydxPerpModel>(100);
    let (dydx_perp_states_tx, mut dydx_perp_states_rx) = mpsc::channel::<RawDydxPerpStateModel>(1000);

    info!("Starting database writer task and waiting for coordination signals");

    // Spawn database writer task
    tokio::spawn(async move {
        // Create PubSub connection inside the task
        let pubsub_client = redis_client_for_task.clone();
        let mut pubsub = pubsub_client.get_async_pubsub().await.unwrap();
        pubsub.subscribe("data_collection_starting").await.unwrap();
        info!("Subscribed to data_collection_starting channel");
        
        // Create publish connection for completion signals
        let mut publish_connection = redis_client_for_task.get_multiplexed_async_connection().await.unwrap();
        
        let mut token_prices_batch = Vec::new();
        let mut market_states_batch = Vec::new();
        let mut new_token_batch = Vec::new();
        let mut new_market_batch = Vec::new();
        let mut new_dydx_perp_batch = Vec::new();
        let mut dydx_perp_states_batch = Vec::new();
        let mut message_stream = pubsub.on_message();

        let mut markets_retry_bank: HashMap<String, (RawMarketModel, u32)> = HashMap::new();
        let mut token_prices_retry_bank: HashMap<String, (Vec<RawTokenPriceModel>, u32)> = HashMap::new();
        let mut market_states_retry_bank: HashMap<String, (Vec<RawMarketStateModel>, u32)> = HashMap::new();
        let mut dydx_perps_retry_bank: HashMap<String, (RawDydxPerpModel, u32)> = HashMap::new();
        let mut dydx_perp_states_retry_bank: HashMap<String, (Vec<RawDydxPerpStateModel>, u32)> = HashMap::new();
        
        // Count-based coordination state
        let mut waiting_for_flush = false;
        let mut expected_token_prices = None::<usize>;
        let mut expected_market_states = None::<usize>;
        let mut expected_dydx_perp_states = None::<usize>;
        let mut token_prices_processed_since_signal = 0usize;
        let mut market_states_processed_since_signal = 0usize;
        let mut dydx_perp_states_processed_since_signal = 0usize;
        
        loop {
            tokio::select! {
                // Collect token prices
                Some(raw_token_price) = token_prices_rx.recv() => {
                    match db.convert_raw_token_price_to_new_token_price(raw_token_price.clone()).await {
                        Ok(Some(token_price)) => {
                            token_prices_batch.push(token_price);
                            if waiting_for_flush {
                                token_prices_processed_since_signal += 1;
                            }
                        },
                        Ok(None) => { // Add to retry bank
                            let entry = token_prices_retry_bank.entry(raw_token_price.token_address.clone()).or_insert((Vec::new(), 0));
                            entry.0.push(raw_token_price.clone());
                            entry.1 += 1;
                            if entry.1 > 10 {
                                error!(token_address = raw_token_price.token_address, "Exceeded 10 retries for token price conversion, dropping entry");
                                token_prices_retry_bank.remove(&raw_token_price.token_address);
                            } else {
                                info!(token_address = raw_token_price.token_address, retry_count = entry.1, "Added token price to retry bank");
                            }
                            if waiting_for_flush {
                                token_prices_processed_since_signal += 1;
                            }
                        },
                        Err(e) => {
                            error!(error = ?e, token_address = raw_token_price.token_address, "Failed to convert raw token price to new token price");
                        }
                    }     

                    // Safety flush if batch gets large
                    if token_prices_batch.len() >= 200 {
                        if let Err(e) = db.insert_token_prices(std::mem::take(&mut token_prices_batch)).await {
                            error!(error = ?e, "Failed to insert token prices batch");
                        } else {
                            info!("Flushed large token prices batch to database (safety flush)");
                        }
                    }
                }
                // Collect market states
                Some(raw_market_state) = market_states_rx.recv() => {
                    match db.convert_raw_market_state_to_new_market_state(raw_market_state.clone()).await {
                        Ok(Some(market_state)) => {
                            market_states_batch.push(market_state);
                            if waiting_for_flush {
                                market_states_processed_since_signal += 1;
                            }
                        },
                        Ok(None) => { // Add to retry bank
                            let entry = market_states_retry_bank.entry(raw_market_state.market_address.clone()).or_insert((Vec::new(), 0));
                            entry.0.push(raw_market_state.clone());
                            entry.1 += 1;
                            if entry.1 > 10 {
                                error!(market_address = %raw_market_state.market_address, "Exceeded 10 retries for market state conversion, dropping entry");
                                market_states_retry_bank.remove(&raw_market_state.market_address);
                            } else {
                                info!(market_address = %raw_market_state.market_address, retry_count = entry.1, "Added market state to retry bank");
                            }
                            if waiting_for_flush {
                                market_states_processed_since_signal += 1;
                            }
                        },
                        Err(e) => {
                            error!(error = ?e, market_address = %raw_market_state.market_address, "Failed to convert raw market state to new market state");
                        }
                    }

                    // Safety flush if batch gets large
                    if market_states_batch.len() >= 200 {
                        if let Err(e) = db.insert_market_states(std::mem::take(&mut market_states_batch)).await {
                            error!(error = ?e, "Failed to insert market states batch");
                        } else {
                            info!("Flushed large market states batch to database (safety flush)");
                        }
                    }
                }
                // Collect new tokens
                Some(raw_new_token) = new_token_rx.recv() => {
                    match db.convert_raw_token_to_new_token(raw_new_token.clone()).await {
                        Ok(new_token) => {
                            new_token_batch.push(new_token);
                            debug!(batch_size = new_token_batch.len(), "Added new token to batch");
                        },
                        Err(e) => {
                            error!(error = ?e, token_address = %raw_new_token.address, "Failed to convert raw token to new token");
                        }
                    }
                }
                // Collect new markets
                Some(raw_new_market) = new_market_rx.recv() => {
                    match db.convert_raw_market_to_new_market(raw_new_market.clone()).await {
                        Ok(Some(new_market)) => {
                            new_market_batch.push(new_market);
                            debug!(batch_size = new_market_batch.len(), "Added new market to batch");
                        },
                        Ok(None) => { // Add to retry bank
                            let entry = markets_retry_bank.entry(raw_new_market.address.clone()).or_insert((raw_new_market.clone(), 0));
                            entry.1 += 1;
                            if entry.1 > 10 {
                                error!(market_address = %raw_new_market.address, "Exceeded 10 retries for new market conversion, dropping entry");
                                markets_retry_bank.remove(&raw_new_market.address);
                            } else {
                                info!(market_address = %raw_new_market.address, retry_count = entry.1, "Added new market to retry bank");
                            }
                        },
                        Err(e) => {
                            error!(error = ?e, market_address = %raw_new_market.address, "Failed to convert raw market to new market");
                        }
                    }
                }
                // Collect new dYdX perps
                Some(raw_dydx_perp) = dydx_perps_rx.recv() => {
                    match db.convert_raw_dydx_perp_to_new_dydx_perp(raw_dydx_perp.clone()).await {
                        Ok(Some(new_perp)) => {
                            new_dydx_perp_batch.push(new_perp);
                            debug!(batch_size = new_dydx_perp_batch.len(), "Added dYdX perp to batch");
                        },
                        Ok(None) => { // Add to retry bank
                            let entry = dydx_perps_retry_bank.entry(raw_dydx_perp.ticker.clone()).or_insert((raw_dydx_perp.clone(), 0));
                            entry.1 += 1;
                            if entry.1 > 10 {
                                error!(ticker = %raw_dydx_perp.ticker, "Exceeded 10 retries for new dYdX perp conversion, dropping entry");
                                dydx_perps_retry_bank.remove(&raw_dydx_perp.ticker);
                            } else {
                                info!(ticker = %raw_dydx_perp.ticker, retry_count = entry.1, "Added dYdX perp to retry bank");
                            }
                        },
                        Err(e) => {
                            error!(error = ?e, ticker = %raw_dydx_perp.ticker, "Failed to convert raw dYdX perp");
                        }
                    }
                }
                // Collect dYdX perp states
                Some(raw_dydx_perp_state) = dydx_perp_states_rx.recv() => {
                    match db.convert_raw_dydx_perp_state_to_new_dydx_perp_state(raw_dydx_perp_state.clone()).await {
                        Ok(Some(dydx_perp_state)) => {
                            dydx_perp_states_batch.push(dydx_perp_state);
                            if waiting_for_flush {
                                dydx_perp_states_processed_since_signal += 1;
                            }
                        },
                        Ok(None) => { // Add to retry bank
                            let entry = dydx_perp_states_retry_bank.entry(raw_dydx_perp_state.ticker.clone()).or_insert((Vec::new(), 0));
                            entry.0.push(raw_dydx_perp_state.clone());
                            entry.1 += 1;
                            if entry.1 > 10 {
                                error!(ticker = %raw_dydx_perp_state.ticker, "Exceeded 10 retries for dYdX perp state conversion, dropping entry");
                                dydx_perp_states_retry_bank.remove(&raw_dydx_perp_state.ticker);
                            } else {
                                info!(ticker = %raw_dydx_perp_state.ticker, retry_count = entry.1, "Added dYdX perp state to retry bank");
                            }
                            if waiting_for_flush {
                                dydx_perp_states_processed_since_signal += 1;
                            }
                        },
                        Err(e) => {
                            error!(error = ?e, ticker = %raw_dydx_perp_state.ticker, "Failed to convert raw dYdX perp state");
                        }
                    }
                }
                // PubSub signal - set coordination expectations
                Some(message) = message_stream.next() => {
                    let channel: String = message.get_channel_name().to_string();
                    let payload: String = message.get_payload().unwrap_or_default();
                    
                    if channel == "data_collection_starting" {
                        // Parse the payload: "starting:token_count:market_count:dydx_perp_count:dydx_perp_state_count"
                        let parts: Vec<&str> = payload.split(':').collect();
                        if parts.len() >= 3 && parts[0] == "starting" {
                            if let (Ok(token_price_count), Ok(market_state_count), Ok(dydx_perp_state_count)) = (parts[1].parse::<usize>(), parts[2].parse::<usize>(), parts[3].parse::<usize>()) {
                                debug!(token_price_count, market_state_count, dydx_perp_state_count, "Received data collection starting signal");
                                
                                expected_token_prices = Some(token_price_count);
                                expected_market_states = Some(market_state_count);
                                expected_dydx_perp_states = Some(dydx_perp_state_count);
                                waiting_for_flush = true;
                            } else {
                                error!(payload = %payload, "Failed to parse token_price/market_state/dydx_perp_state counts from payload");
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
                                expected_token_prices.is_some() && 
                                expected_market_states.is_some() &&
                                expected_dydx_perp_states.is_some() &&
                                token_prices_processed_since_signal >= expected_token_prices.unwrap() && 
                                market_states_processed_since_signal >= expected_market_states.unwrap() &&
                                dydx_perp_states_processed_since_signal >= expected_dydx_perp_states.unwrap() => {

                    info!(
                        token_prices_processed = token_prices_batch.len(),
                        market_states_processed = market_states_batch.len(),
                        dydx_perp_states_processed = dydx_perp_states_batch.len(),
                        expected_token_prices = expected_token_prices.unwrap(),
                        expected_market_states = expected_market_states.unwrap(),
                        expected_dydx_perp_states = expected_dydx_perp_states.unwrap(),
                        "All token_prices, market_states, and dydx_perp_states expectations met, performing coordination flush"
                    );
                    
                    // Flush ALL current batches (whatever we have accumulated)
                    let token_prices_count = token_prices_batch.len();
                    let market_states_count = market_states_batch.len();
                    let new_token_count = new_token_batch.len();
                    let new_market_count = new_market_batch.len();
                    let new_dydx_perp_count = new_dydx_perp_batch.len();
                    let dydx_perp_state_count = dydx_perp_states_batch.len();
                    
                    if !token_prices_batch.is_empty() {
                        if let Err(e) = db.insert_token_prices(std::mem::take(&mut token_prices_batch)).await {
                            error!(error = ?e, "Failed to insert token prices");
                        } else {
                            info!(count = token_prices_count, "Coordination flush: inserted token prices");
                        }
                    }
                    if !market_states_batch.is_empty() {
                        if let Err(e) = db.insert_market_states(std::mem::take(&mut market_states_batch)).await {
                            error!(error = ?e, "Failed to insert market states");
                        } else {
                            info!(count = market_states_count, "Coordination flush: inserted market states");
                        }
                    }
                    if !dydx_perp_states_batch.is_empty() {
                        if let Err(e) = db.insert_dydx_perp_states(std::mem::take(&mut dydx_perp_states_batch)).await {
                            error!(error = ?e, "Failed to insert dYdX perp states");
                        } else {
                            info!(count = dydx_perp_state_count, "Coordination flush: inserted dYdX perp states");
                        }
                    }
                    
                    // Insert new tokens, markets, perps if any exist
                    if !new_token_batch.is_empty() {
                        if let Err(e) = db.insert_tokens(std::mem::take(&mut new_token_batch)).await {
                            error!(error = ?e, "Failed to insert new tokens");
                        } else {
                            info!(count = new_token_count, "Coordination flush: inserted new tokens");
                        }
                    }
                    if !new_market_batch.is_empty() {
                        if let Err(e) = db.insert_markets(std::mem::take(&mut new_market_batch)).await {
                            error!(error = ?e, "Failed to insert new markets");
                        } else {
                            info!(count = new_market_count, "Coordination flush: inserted new markets");
                        }
                    }
                    if !new_dydx_perp_batch.is_empty() {
                        let mut inserted = 0usize;
                        for perp in std::mem::take(&mut new_dydx_perp_batch) {
                            if db.insert_dydx_perp(&perp).await.is_ok() {
                                inserted += 1;
                            }
                        }
                        info!(count = inserted, expected = new_dydx_perp_count, "Coordination flush: inserted dYdX perps");
                    }
                    
                    // After inserting new tokens and markets, retry failed conversions since new foreign key IDs might now be available
                    if !token_prices_retry_bank.is_empty()
                        || !markets_retry_bank.is_empty()
                        || !market_states_retry_bank.is_empty()
                        || !dydx_perps_retry_bank.is_empty()
                        || !dydx_perp_states_retry_bank.is_empty()
                    {
                        info!(
                            token_prices_retrying = token_prices_retry_bank.len(),
                            markets_retrying = markets_retry_bank.len(),
                            market_states_retrying = market_states_retry_bank.len(),
                            dydx_perps_retrying = dydx_perps_retry_bank.len(),
                            dydx_perp_states_retrying = dydx_perp_states_retry_bank.len(),
                            "Retrying failed conversions after coordination flush"
                        );
                    }
                    
                    // Retry token prices
                    let mut token_prices_retried = 0;
                    let mut token_addresses_to_remove = Vec::new();
                    for (address, (raw_token_prices, retry_count)) in token_prices_retry_bank.iter_mut() {
                        let mut fail_in_batch = false;
                        let mut items_to_keep = Vec::new();
                        for raw_token_price in raw_token_prices.drain(..) {
                            match db.convert_raw_token_price_to_new_token_price(raw_token_price.clone()).await {
                                Ok(Some(token_price)) => {
                                    token_prices_batch.push(token_price);
                                    token_prices_retried += 1;
                                },
                                Ok(None) => {
                                    // Still failing, keep for next retry
                                    items_to_keep.push(raw_token_price);
                                    fail_in_batch = true;
                                },
                                Err(e) => {
                                    error!(error = ?e, token_address = %raw_token_price.token_address, "Error retrying token price conversion");
                                    items_to_keep.push(raw_token_price);
                                    fail_in_batch = true;
                                }
                            }
                        }
                        *raw_token_prices = items_to_keep;
                        if !fail_in_batch {
                            token_addresses_to_remove.push(address.clone());
                        } else {
                            *retry_count += 1;
                            if *retry_count > 10 {
                                error!(token_address = %address, "Exceeded 10 retries for token price conversion after coordination flush, dropping entry");
                                token_addresses_to_remove.push(address.clone());
                            }
                        }
                    }
                    for address in token_addresses_to_remove {
                        token_prices_retry_bank.remove(&address);
                    }

                    // Retry new markets
                    let mut markets_to_remove = Vec::new();
                    let mut markets_retried = 0;
                    for (address, (raw_new_market, retry_count)) in markets_retry_bank.iter_mut() {
                        match db.convert_raw_market_to_new_market(raw_new_market.clone()).await {
                            Ok(Some(new_market)) => {
                                new_market_batch.push(new_market);
                                markets_to_remove.push(address.clone());
                                markets_retried += 1;
                            },
                            Ok(None) => {
                                // Still failing, keep for next retry
                                *retry_count += 1;
                                if *retry_count > 10 {
                                    error!(market_address = %address, "Exceeded 10 retries for new market conversion after coordination flush, dropping entry");
                                    markets_to_remove.push(address.clone());
                                }
                            },
                            Err(e) => {
                                error!(error = ?e, market_address = %raw_new_market.address, "Error retrying new market conversion");
                                *retry_count += 1;
                                if *retry_count > 10 {
                                    error!(market_address = %address, "Exceeded 10 retries for new market conversion after coordination flush, dropping entry");
                                    markets_to_remove.push(address.clone());
                                }
                            }
                        }
                    }
                    for address in markets_to_remove {
                        markets_retry_bank.remove(&address);
                    }
                    
                    // Retry market states
                    let mut market_states_to_remove = Vec::new();
                    let mut market_states_retried = 0;
                    for (address, (raw_market_states, retry_count)) in market_states_retry_bank.iter_mut() {
                        let mut fail_in_batch = false;
                        let mut items_to_keep = Vec::new();
                        for raw_market_state in raw_market_states.drain(..) {
                            match db.convert_raw_market_state_to_new_market_state(raw_market_state.clone()).await {
                                Ok(Some(market_state)) => {
                                    market_states_batch.push(market_state);
                                    market_states_retried += 1;
                                },
                                Ok(None) => {
                                    // Still failing, keep for next retry
                                    items_to_keep.push(raw_market_state);
                                    fail_in_batch = true;
                                },
                                Err(e) => {
                                    error!(error = ?e, market_address = %raw_market_state.market_address, "Error retrying market state conversion");
                                    items_to_keep.push(raw_market_state);
                                    fail_in_batch = true;
                                }
                            }
                        }
                        *raw_market_states = items_to_keep;
                        if !fail_in_batch {
                            market_states_to_remove.push(address.clone());
                        }
                        else {
                            *retry_count += 1;
                            if *retry_count > 10 {
                                error!(market_address = %address, "Exceeded 10 retries for market state conversion after coordination flush, dropping entry");
                                market_states_to_remove.push(address.clone());
                            }
                        }
                    }
                    for address in market_states_to_remove {
                        market_states_retry_bank.remove(&address);
                    }

                    // Retry dYdX perps
                    let mut dydx_perps_to_remove = Vec::new();
                    let mut dydx_perps_retried = 0;
                    for (ticker, (raw_perp, retry_count)) in dydx_perps_retry_bank.iter_mut() {
                        match db.convert_raw_dydx_perp_to_new_dydx_perp(raw_perp.clone()).await {
                            Ok(Some(new_perp)) => {
                                new_dydx_perp_batch.push(new_perp);
                                dydx_perps_to_remove.push(ticker.clone());
                                dydx_perps_retried += 1;
                            },
                            Ok(None) => {
                                // Still failing, keep for next retry
                                *retry_count += 1;
                                if *retry_count > 10 {
                                    error!(ticker = %ticker, "Exceeded 10 retries for dYdX perp conversion after coordination flush, dropping entry");
                                    dydx_perps_to_remove.push(ticker.clone());
                                }
                            },
                            Err(e) => {
                                error!(error = ?e, ticker = %ticker, "Error retrying dYdX perp conversion");
                                *retry_count += 1;
                                if *retry_count > 10 {
                                    error!(ticker = %ticker, "Exceeded 10 retries for dYdX perp conversion after coordination flush, dropping entry");
                                    dydx_perps_to_remove.push(ticker.clone());
                                }
                            }
                        }
                    }
                    for ticker in dydx_perps_to_remove {
                        dydx_perps_retry_bank.remove(&ticker);
                    }

                    // Retry dYdX perp states
                    let mut dydx_perp_states_to_remove = Vec::new();
                    let mut dydx_perp_states_retried = 0;
                    for (ticker, (raw_states, retry_count)) in dydx_perp_states_retry_bank.iter_mut() {
                        let mut fail_in_batch = false;
                        let mut items_to_keep = Vec::new();
                        for raw_state in raw_states.drain(..) {
                            match db.convert_raw_dydx_perp_state_to_new_dydx_perp_state(raw_state.clone()).await {
                                Ok(Some(state)) => {
                                    dydx_perp_states_batch.push(state);
                                    dydx_perp_states_retried += 1;
                                },
                                Ok(None) => {
                                    // Still failing, keep for next retry
                                    items_to_keep.push(raw_state);
                                    fail_in_batch = true;
                                },
                                Err(e) => {
                                    error!(error = ?e, ticker = %raw_state.ticker, "Error retrying dYdX perp state conversion");
                                    items_to_keep.push(raw_state);
                                    fail_in_batch = true;
                                }
                            }
                        }
                        *raw_states = items_to_keep;
                        if !fail_in_batch {
                            dydx_perp_states_to_remove.push(ticker.clone());
                        } else {
                            *retry_count += 1;
                            if *retry_count > 10 {
                                error!(ticker = %ticker, "Exceeded 10 retries for dYdX perp state conversion after coordination flush, dropping entry");
                                dydx_perp_states_to_remove.push(ticker.clone());
                            }
                        }
                    }
                    for ticker in dydx_perp_states_to_remove {
                        dydx_perp_states_retry_bank.remove(&ticker);
                    }
                    
                    // Insert any items that were successfully retried
                    if !token_prices_batch.is_empty() {
                        if let Err(e) = db.insert_token_prices(std::mem::take(&mut token_prices_batch)).await {
                            error!(error = ?e, "Failed to insert retried token prices");
                        } else {
                            debug!(count = token_prices_batch.len(), "Inserted retried token prices");
                        }
                    }
                    if !new_market_batch.is_empty() {
                        if let Err(e) = db.insert_markets(std::mem::take(&mut new_market_batch)).await {
                            error!(error = ?e, "Failed to insert retried new markets");
                        } else {
                            debug!(count = new_market_batch.len(), "Inserted retried new markets");
                        }
                    }
                    if !market_states_batch.is_empty() {
                        if let Err(e) = db.insert_market_states(std::mem::take(&mut market_states_batch)).await {
                            error!(error = ?e, "Failed to insert retried market states");
                        } else {
                            debug!(count = market_states_batch.len(), "Inserted retried market states");
                        }
                    }
                    if !new_dydx_perp_batch.is_empty() {
                        let mut inserted = 0usize;
                        for perp in std::mem::take(&mut new_dydx_perp_batch) {
                            if db.insert_dydx_perp(&perp).await.is_ok() {
                                inserted += 1;
                            }
                        }
                        debug!(count = inserted, "Inserted retried dYdX perps");
                    }
                    if !dydx_perp_states_batch.is_empty() {
                        if let Err(e) = db.insert_dydx_perp_states(std::mem::take(&mut dydx_perp_states_batch)).await {
                            error!(error = ?e, "Failed to insert retried dYdX perp states");
                        } else {
                            debug!(count = dydx_perp_states_batch.len(), "Inserted retried dYdX perp states");
                        }
                    }
                    
                    if token_prices_retried > 0
                        || market_states_retried > 0
                        || markets_retried > 0
                        || dydx_perps_retried > 0
                        || dydx_perp_states_retried > 0
                    {
                        info!(
                            token_prices_retried,
                            market_states_retried,
                            markets_retried,
                            dydx_perps_retried,
                            dydx_perp_states_retried,
                            "Successfully retried failed conversions after coordination flush"
                        );
                    }
                    if !token_prices_retry_bank.is_empty()
                        || !markets_retry_bank.is_empty()
                        || !market_states_retry_bank.is_empty()
                        || !dydx_perps_retry_bank.is_empty()
                        || !dydx_perp_states_retry_bank.is_empty()
                    {
                        warn!(
                            token_prices_still_failing = token_prices_retry_bank.len(),
                            markets_still_failing = markets_retry_bank.len(),
                            market_states_still_failing = market_states_retry_bank.len(),
                            dydx_perps_still_failing = dydx_perps_retry_bank.len(),
                            dydx_perp_states_still_failing = dydx_perp_states_retry_bank.len(),
                            "Some entries still failing after coordination flush and retry attempt"
                        );
                    }
                    
                    // Publish completion signal with actual processed counts
                    let completion_message = format!("completed:{}:{}:{}",
                        token_prices_processed_since_signal,
                        market_states_processed_since_signal,
                        dydx_perp_states_processed_since_signal
                    );
                    let _: Result<(), _> = publish_connection.publish("data_collection_completed", completion_message).await;
                    info!(
                        token_prices_flushed = token_prices_count,
                        market_states_flushed = market_states_count,
                        dydx_perp_states_flushed = dydx_perp_state_count,
                        "Published data collection completed signal"
                    );
                    
                    // Reset coordination state after successful flush
                    waiting_for_flush = false;
                    expected_token_prices = None;
                    expected_market_states = None;
                    expected_dydx_perp_states = None;
                    token_prices_processed_since_signal = 0;
                    market_states_processed_since_signal = 0;
                    dydx_perp_states_processed_since_signal = 0;
                }
            }
        }
    });

    // Perpetual loop to listen for new stream entries
    let stream_options = StreamReadOptions::default().block(0).count(10);
    
    // Load last processed IDs from Redis, or use "$" for latest if not found
    let mut last_ids = HashMap::new();
    for stream_name in [
        "token_prices",
        "market_states",
        "new_tokens",
        "new_markets",
        "dydx_perps",
        "dydx_perp_states",
    ] {
        let key = format!("data_recorder:last_id:{}", stream_name);
        let last_id: Option<String> = redis_connection.get(&key).await.unwrap_or(None);
        let id = last_id.unwrap_or_else(|| "0".to_string()); 
        last_ids.insert(stream_name.to_string(), id);
        info!(stream = %stream_name, last_id = %last_ids[stream_name], "Loaded last processed ID");
    }

    info!("Starting Redis stream listener");
    loop {
        // Use explicit stream names and IDs for xread_options
        let reply: StreamReadReply = redis_connection
            .xread_options(
                &[
                    "token_prices",
                    "market_states",
                    "new_tokens",
                    "new_markets",
                    "dydx_perps",
                    "dydx_perp_states",
                ],
                &[
                    &last_ids["token_prices"],
                    &last_ids["market_states"],
                    &last_ids["new_tokens"],
                    &last_ids["new_markets"],
                    &last_ids["dydx_perps"],
                    &last_ids["dydx_perp_states"],
                ],
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
                &token_prices_tx,
                &market_states_tx,
                &new_token_tx,
                &new_market_tx,
                &dydx_perps_tx,
                &dydx_perp_states_tx,
                &mut last_ids,
                &mut redis_connection,
            ).await {
                error!(error = ?e, stream_name = %stream_name, "Failed to process stream entries");
                return Err(e);
            }
        }

        sleep(Duration::from_millis(100)).await;
    }
}
