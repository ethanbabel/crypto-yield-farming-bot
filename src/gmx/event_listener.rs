use ethers::{
    types::{Address, U256, H256, Filter},
    providers::{Provider, Ws, StreamExt, Middleware},
    utils::keccak256,
    abi::{encode, Token},
    contract::abigen,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use std::time::Duration;
use eyre::Result;
use tracing::{info, error};

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
    provider: Arc<Provider<Ws>>,
    event_emitter_address: Address,
}

impl GmxEventListener {
    // Initialize the cumulative fees data structure and return the listener
    pub fn init(provider: Arc<Provider<Ws>>, event_emitter_address: Address) -> Self {
        GmxEventListener {
            fees: Arc::new(Mutex::new(HashMap::new())),
            provider,
            event_emitter_address,
        }
    }

    // Start listening for events (to be called in long-running background task)
    pub async fn start_listening(&self) -> Result<()> {
        info!("Starting GMX event listener...");

        let event_emitter = EventEmitter::new(self.event_emitter_address, self.provider.clone());

        // Filter for PositionFeesCollected and SwapFeesCollected events
        let position_fees_collected_hash = string_to_bytes32("PositionFeesCollected");
        let swap_fees_collected_hash = string_to_bytes32("SwapFeesCollected");
        let topic1_vec = vec![
            position_fees_collected_hash,
            swap_fees_collected_hash,
        ];

        // Build the event stream directly from the contract instance
       let event_watcher = event_emitter
            .event::<event_emitter::EventLog1Filter>()
            .topic1(topic1_vec);
        let mut stream = event_watcher.subscribe().await?;
        
        // Process events as they come in
        while let Some(Ok(event)) = stream.next().await {
            let event_name = event.event_name.as_str();
            match event_name {
                "PositionFeesCollected" => { self.process_position_fees_event(event).await; },
                "SwapFeesCollected" => { self.process_swap_fees_event(event).await; },
                _ => {
                    error!("Unknown event type: {}", event_name);
                }
            }
        }

        Ok(())
    }

    // Process PositionFeesCollected event
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
            let liquidation_fee_amount_for_fee_receriver = match event.event_data.uint_items.items.get(n - 1) {
                Some(item) => item.value,
                None => {
                    error!("Missing liquidation_fee_amount_for_fee_receriver at index {}", n - 1);
                    return;
                }
            };
            liquidation_fee_amount - liquidation_fee_amount_for_fee_receriver
        } else {
            U256::zero()
        };

        // Update fees map
        let mut fees_map = self.fees.lock().await;
        let market_fees = fees_map.entry(market_address).or_insert_with(MarketFees::new);
        // Update position_fees
        let position_amount = event.event_data.uint_items.items[0].value;
        *market_fees.position_fees.entry(collateral_token).or_insert(U256::zero()) += position_amount;
        // Update liquidation_fees
        let liquidation_amount = event.event_data.uint_items.items[1].value;
        *market_fees.liquidation_fees.entry(collateral_token).or_insert(U256::zero()) += liquidation_amount;
        // Update borrowing_fees
        let borrowing_amount = event.event_data.uint_items.items[2].value;
        *market_fees.borrowing_fees.entry(collateral_token).or_insert(U256::zero()) += borrowing_amount;
    }

    // Process SwapFeesCollected event
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
        
        // Update fees map
        let mut fees_map = self.fees.lock().await;
        let market_fees = fees_map.entry(market_address).or_insert_with(MarketFees::new);
        *market_fees.swap_fees.entry(token).or_insert(U256::zero()) += fee_amount_for_pool;
    }
}