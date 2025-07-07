use ethers::{
    types::{Address, U256, H256},
    providers::{Provider, Ws, StreamExt},
    contract::abigen,
    middleware::Middleware,
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use eyre::Result;
use tracing::{info, error, warn, debug, instrument};

use super::event_listener_utils::{
    // u256_to_decimal_scaled,
    // u256_to_decimal_scaled_decimals,
    string_to_bytes32,
    MarketFees,
    CumulativeFeesMap,
};


abigen!(
    EventEmitter,
    "./abis/EventEmitter.json",
);   

// --- GMX Event Listener ---
pub struct GmxEventListener {
    pub fees: CumulativeFeesMap,
    ws_url: String,
    event_emitter_address: Address,
}

impl GmxEventListener {
    // Initialize the cumulative fees data structure and return the listener
    #[instrument(skip(ws_url))]
    pub fn init(ws_url: String, event_emitter_address: Address) -> Self {
        info!("Initializing GMX event listener");
        GmxEventListener {
            fees: Arc::new(Mutex::new(HashMap::new())),
            ws_url,
            event_emitter_address,
        }
    }

    // Start listening for events (to be called in long-running background task)
    #[instrument(skip(self), fields(event_emitter = %self.event_emitter_address))]
    pub async fn start_listening(&self) -> Result<()> {
        info!("Starting GMX event listener");

        // Filter for PositionFeesCollected and SwapFeesCollected events
        let position_fees_collected_hash = string_to_bytes32("PositionFeesCollected");
        let swap_fees_collected_hash = string_to_bytes32("SwapFeesCollected");
        let topic1_vec = vec![
            position_fees_collected_hash,
            swap_fees_collected_hash,
        ];

        // Track ping task handle for cleanup
        let mut ping_handle: Option<tokio::task::JoinHandle<()>> = None;

        loop {
            // Clean up previous ping task if it exists
            if let Some(handle) = ping_handle.take() {
                handle.abort();
                debug!("Aborted previous ping task");
            }

            // Create a new provider and event emitter on each reconnect
            let provider = match Provider::<Ws>::connect(&self.ws_url).await {
                Ok(p) => Arc::new(p),
                Err(e) => {
                    error!(?e, "Failed to connect to WebSocket provider");
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    continue;
                }
            };

            // Create health signaling channel
            let (health_tx, mut health_rx) = tokio::sync::mpsc::channel(1);

            // Start connection health monitor
            let ping_provider = provider.clone();
            ping_handle = Some(tokio::spawn(async move {
                let mut interval = tokio::time::interval(Duration::from_secs(300));

                loop {
                    interval.tick().await;
                    match ping_provider.get_block_number().await {
                        Ok(_) => {
                            debug!("Connection healthy");
                        }
                        Err(e) => {
                            warn!(?e, "Connection ping failed. Connection declared unhealthy - signaling reconnect");
                            let _ = health_tx.send(()).await;
                            break; // Exit ping task cleanly
                        }
                    }
                }
                debug!("Health monitor task exiting");
            }));

            let event_emitter = EventEmitter::new(self.event_emitter_address, provider.clone());
            let event_watcher = event_emitter
                .event::<event_emitter::EventLog1Filter>()
                .topic1(topic1_vec.clone());

            match event_watcher.subscribe().await {
                Ok(mut stream) => {
                    info!("Event stream subscribed, processing events");
                    let mut events_processed = 0u64;
                    
                    loop {
                        tokio::select! {
                            // Process events with timeout
                            event_result = tokio::time::timeout(Duration::from_secs(300), stream.next()) => {
                                match event_result {
                                    Ok(Some(Ok(event))) => {
                                        let event_name = event.event_name.as_str();
                                        match event_name {
                                            "PositionFeesCollected" => {
                                                self.process_position_fees_event(event).await;
                                                events_processed += 1;
                                            },
                                            "SwapFeesCollected" => {
                                                self.process_swap_fees_event(event).await;
                                                events_processed += 1;
                                            },
                                            _ => {
                                                warn!(event_name = event_name, "Unknown event type received");
                                            }
                                        }

                                        if events_processed % 100 == 0 {
                                            debug!(events_processed = events_processed, "Event processing milestone");
                                        }
                                    },
                                    Ok(Some(Err(e))) => {
                                        error!(?e, "Stream error - reconnecting immediately");
                                        break; // Exit to reconnection loop
                                    },
                                    Ok(None) => {
                                        warn!("Stream ended - reconnecting");
                                        break; // Exit to reconnection loop
                                    },
                                    Err(_) => {
                                        debug!("No events in 300s - continuing");
                                        // Continue - let health monitor decide if connection is dead
                                    }
                                }
                            }
                            // Listen for health monitor signals
                            _ = health_rx.recv() => {
                                warn!("Health monitor signaled connection failure - reconnecting");
                                break; // Exit to reconnection loop
                            }
                        }
                    }
                    warn!("Event stream ended, reconnecting...");
                }
                Err(e) => {
                    error!(?e, "Failed to subscribe to GMX event stream, retrying in 10s...");
                    tokio::time::sleep(Duration::from_secs(10)).await;
                }
            }

            // Brief pause before reconnecting
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }

    // Process PositionFeesCollected event
    #[instrument(skip(self, event), fields(event_name = "PositionFeesCollected"))]
    async fn process_position_fees_event(&self, event: event_emitter::EventLog1Filter) {
        let market_address = match event.event_data.address_items.items.get(0) {
            Some(item) => Address::from(H256::from(item.value)),
            None => {
                error!("Missing market_address at index 0");
                return;
            }
        };
        let collateral_token = match event.event_data.address_items.items.get(1) {
            Some(item) => Address::from(H256::from(item.value)),
            None => {
                error!("Missing collateral_token at index 1");
                return;
            }
        };

        let trade_size_usd = match event.event_data.uint_items.items.get(2) {
            Some(item) => item.value,
            None => {
                error!("Missing trade_size_usd at index 2");
                return;
            }
        };
        let borrowing_fee_amount = match event.event_data.uint_items.items.get(10) {
            Some(item) => item.value,
            None => {
                error!("Missing borrowing_fee_amount at index 10");
                return;
            }
        };
        let borrowing_fee_amount_for_fee_receriver = match event.event_data.uint_items.items.get(12) {
            Some(item) => item.value,
            None => {
                error!("Missing borrowing_fee_amount_for_fee_receriver at index 12");
                return;
            }
        };
        let borrowing_fee_amount_for_pool = borrowing_fee_amount - borrowing_fee_amount_for_fee_receriver;
        let position_fee_amount_for_pool = match event.event_data.uint_items.items.get(18) {
            Some(item) => item.value,
            None => {
                error!("Missing position_fee_amount_for_pool at index 18");
                return;
            }
        };
        let liquidation_fee_items_array_lengths = vec![26, 28, 32, 34];
        let n = event.event_data.uint_items.items.len();
        let liquidation_fee_amount_for_pool = if liquidation_fee_items_array_lengths.contains(&n) {
            let liquidation_fee_amount = match event.event_data.uint_items.items.get(n - 3) {
                Some(item) => item.value,
                None => {
                    error!("Missing liquidation_fee_amount at index {}", n - 3);
                    return;
                }
            };
            let liquidation_fee_amount_for_fee_receiver = match event.event_data.uint_items.items.get(n - 1) {
                Some(item) => item.value,
                None => {
                    error!("Missing liquidation_fee_amount_for_fee_receiver at index {}", n - 1);
                    return;
                }
            };
            liquidation_fee_amount - liquidation_fee_amount_for_fee_receiver
        } else {
            U256::zero()
        };

        // Update fees map
        let mut fees_map = self.fees.lock().await;
        let market_fees = fees_map.entry(market_address).or_insert_with(MarketFees::new);
        // Update position_fees
        *market_fees.position_fees.entry(collateral_token).or_insert(U256::zero()) +=  position_fee_amount_for_pool;
        // Update liquidation_fees
        *market_fees.liquidation_fees.entry(collateral_token).or_insert(U256::zero()) += liquidation_fee_amount_for_pool;
        // Update borrowing_fees
        *market_fees.borrowing_fees.entry(collateral_token).or_insert(U256::zero()) += borrowing_fee_amount_for_pool;
        // Update trading volume
        market_fees.trading_volume += trade_size_usd;
        
        debug!(
            market = %market_address,
            collateral_token = %collateral_token,
            position_fee = %position_fee_amount_for_pool,
            borrowing_fee = %borrowing_fee_amount_for_pool,
            liquidation_fee = %liquidation_fee_amount_for_pool,
            trade_size_usd = %trade_size_usd,
            "Position fees event processed"
        );
    }

    // Process SwapFeesCollected event
    #[instrument(skip(self, event), fields(event_name = "SwapFeesCollected"))]
    async fn process_swap_fees_event(&self, event: event_emitter::EventLog1Filter) {
        let market_address = match event.event_data.address_items.items.get(1) {
            Some(item) => Address::from(H256::from(item.value)),
            None => {
                error!("Missing market_address at index 1");
                return;
            }
        };
        let token = match event.event_data.address_items.items.get(2) {
            Some(item) => Address::from(H256::from(item.value)),
            None => {
                error!("Missing token at index 2");
                return;
            }
        };

        let fee_amount_for_pool = match event.event_data.uint_items.items.get(2) {
            Some(item) => item.value,
            None => {
                error!("Missing fee_amount_for_pool at index 2");
                return;
            }
        };
        let amount_after_fees = match event.event_data.uint_items.items.get(3) {
            Some(item) => item.value,
            None => {
                error!("Missing amount_after_fees at index 3");
                return;
            }
        };
        
        // Update fees map
        let mut fees_map = self.fees.lock().await;
        let market_fees = fees_map.entry(market_address).or_insert_with(MarketFees::new);
        *market_fees.swap_fees.entry(token).or_insert(U256::zero()) += fee_amount_for_pool;
        *market_fees.swap_volume.entry(token).or_insert(U256::zero()) += amount_after_fees;
        
        debug!(
            market = %market_address,
            token = %token,
            fee_amount = %fee_amount_for_pool,
            volume = %amount_after_fees,
            "Swap fees event processed"
        );
    }
}