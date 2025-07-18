use ethers::{
    types::{Address, U256, H256, BlockNumber, Filter, Log},
    providers::{Provider, Http, Middleware},
    contract::{abigen, EthLogDecode},
};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use eyre::Result;
use tracing::{info, error, warn, debug, instrument};

use super::event_listener_utils::{
    string_to_bytes32,
    MarketFees,
};

abigen!(
    EventEmitter,
    "./abis/EventEmitter.json",
);   

// --- GMX Event Fetcher ---
pub struct GmxEventFetcher {
    provider: Arc<Provider<Http>>,
    event_emitter_address: Address,
    last_block_fetched: Option<u64>,
}

impl GmxEventFetcher {
    // Initialize the event fetcher
    #[instrument(skip(provider))]
    pub fn init(provider: Arc<Provider<Http>>, event_emitter_address: Address) -> Self {
        info!("Initializing GMX event fetcher");
        GmxEventFetcher {
            provider,
            event_emitter_address,
            last_block_fetched: None,
        }
    }

    // Helper method to fetch logs with retry logic
    #[instrument(skip(self, filter))]
    async fn fetch_logs_with_retry(&self, filter: &Filter) -> Result<Vec<Log>> {
        let max_retries = 3;
        let mut last_error = None;

        for attempt in 1..=max_retries {
            match self.provider.get_logs(filter).await {
                Ok(logs) => {
                    if attempt > 1 {
                        info!(attempt = attempt, "Successfully fetched logs after retry");
                    }
                    return Ok(logs);
                }
                Err(e) => {
                    last_error = Some(e);
                    if attempt < max_retries {
                        let delay_ms = attempt * 500; // Linear backoff: 500ms, 1000ms, 1500ms
                        warn!(
                            attempt = attempt,
                            delay_ms = delay_ms,
                            error = ?last_error,
                            "Failed to fetch logs, retrying..."
                        );
                        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    }
                }
            }
        }

        error!(
            max_retries = max_retries,
            error = ?last_error,
            "Failed to fetch logs after all retries"
        );
        Err(last_error.unwrap().into())
    }

    // Fetch fees from last block to latest (on-demand)
    #[instrument(skip(self), fields(event_emitter = %self.event_emitter_address))]
    pub async fn fetch_fees(&mut self) -> Result<HashMap<Address, MarketFees>> {
        debug!("Fetching GMX fees");

        // Get current block number
        let current_block = self.provider.get_block_number().await?;
        let current_block_u64 = current_block.as_u64();

        // Handle first time call - just set the block and return empty
        if self.last_block_fetched.is_none() {
            info!(block = current_block_u64, "First time fetch - setting initial block");
            self.last_block_fetched = Some(current_block_u64);
            return Ok(HashMap::new());
        }

        let from_block = self.last_block_fetched.unwrap();
        
        // If we're already at the latest block, return empty
        if from_block >= current_block_u64 {
            debug!("No new blocks to process");
            return Ok(HashMap::new());
        }

        debug!(
            from_block = from_block,
            to_block = current_block_u64,
            blocks_to_process = current_block_u64 - from_block,
            "Fetching events for block range"
        );

        // Filter for PositionFeesCollected and SwapFeesCollected events
        let position_fees_collected_hash = string_to_bytes32("PositionFeesCollected");
        let swap_fees_collected_hash = string_to_bytes32("SwapFeesCollected");
        let topic1_vec = vec![
            position_fees_collected_hash,
            swap_fees_collected_hash,
        ];

        // Create filter for events
        let filter = Filter::new()
            .address(self.event_emitter_address)
            .topic1(topic1_vec)
            .from_block(BlockNumber::Number((from_block + 1).into()))
            .to_block(BlockNumber::Number(current_block_u64.into()));

        // Fetch logs
        let logs = self.fetch_logs_with_retry(&filter).await?;
        debug!(logs_count = logs.len(), "Retrieved logs");

        // Process logs and build fees map
        let mut fees_map = HashMap::new();
        let mut events_processed = 0u64;

        for log in logs {
            // Try to decode as EventLog1Filter (which contains the event_name)
            if let Ok(decoded_log) = event_emitter::EventLog1Filter::decode_log(&log.clone().into()) {
                let event_name = decoded_log.event_name.as_str();
                match event_name {
                    "PositionFeesCollected" => {
                        self.process_position_fees_event(&decoded_log, &mut fees_map);
                        events_processed += 1;
                    },
                    "SwapFeesCollected" => {
                        self.process_swap_fees_event(&decoded_log, &mut fees_map);
                        events_processed += 1;
                    },
                    _ => {
                        warn!(event_name = event_name, "Unknown event type received");
                    }
                }
            } else {
                warn!("Failed to decode event log");
            }
        }

        // Update last processed block
        self.last_block_fetched = Some(current_block_u64);

        info!(
            events_processed = events_processed,
            markets_updated = fees_map.len(),
            "Completed fee fetching"
        );

        Ok(fees_map)
    }

    // Process PositionFeesCollected event
    #[instrument(skip(self, event, fees_map), fields(event_name = "PositionFeesCollected"))]
    fn process_position_fees_event(&self, event: &event_emitter::EventLog1Filter, fees_map: &mut HashMap<Address, MarketFees>) {
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
    #[instrument(skip(self, event, fees_map), fields(event_name = "SwapFeesCollected"))]
    fn process_swap_fees_event(&self, event: &event_emitter::EventLog1Filter, fees_map: &mut HashMap<Address, MarketFees>) {
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

    // Getter for last block fetched (for persistence/debugging)
    pub fn get_last_block_fetched(&self) -> Option<u64> {
        self.last_block_fetched
    }

    // Setter for last block fetched (for initialization from persistence)
    pub fn set_last_block_fetched(&mut self, block: u64) {
        self.last_block_fetched = Some(block);
    }
}
