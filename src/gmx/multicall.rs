use std::collections::HashMap;
use ethers::prelude::*;
use eyre::Result;
use tracing::{debug, error, instrument};

use crate::config::Config;
use super::reader_utils::{MarketProps, MarketPrices, MarketInfo, MarketPoolValueInfoProps};
use super::reader::PnlFactorType;

/// Batch data structure containing all market data fetched via multicall
#[derive(Debug, Clone)]
pub struct BatchMarketData {
    pub market_infos: HashMap<Address, MarketInfo>,
    pub gm_prices_min: HashMap<Address, (I256, MarketPoolValueInfoProps)>,
    pub gm_prices_max: HashMap<Address, (I256, MarketPoolValueInfoProps)>,
    pub open_interest_long: HashMap<Address, U256>,
    pub open_interest_short: HashMap<Address, U256>,
    pub open_interest_tokens_long: HashMap<Address, U256>,
    pub open_interest_tokens_short: HashMap<Address, U256>,
}

impl BatchMarketData {
    pub fn new() -> Self {
        Self {
            market_infos: HashMap::new(),
            gm_prices_min: HashMap::new(),
            gm_prices_max: HashMap::new(),
            open_interest_long: HashMap::new(),
            open_interest_short: HashMap::new(),
            open_interest_tokens_long: HashMap::new(),
            open_interest_tokens_short: HashMap::new(),
        }
    }
}

/// Fetch all market data using multicall batches
#[instrument(skip(config, markets), fields(market_count = markets.len()))]
pub async fn fetch_all_market_data_batch(
    config: &Config,
    markets: &[(MarketProps, MarketPrices)],
) -> Result<BatchMarketData> {
    debug!(market_count = markets.len(), "Starting batch fetch of all market data");
    
    let mut batch_data = BatchMarketData::new();
    
    // Batch 1: Market info
    debug!("Fetching market info batch");
    batch_data.market_infos = super::reader::get_market_info_batch(config, markets).await
        .map_err(|e| {
            error!("Failed to fetch market info batch: {}", e);
            e
        })?;
    
    // Batch 2: Market token prices (min and max)
    debug!("Fetching market token price batch");
    let (min_prices, max_prices) = super::reader::get_market_token_price_batch(config, markets, PnlFactorType::Deposit).await
        .map_err(|e| {
            error!("Failed to fetch market token price batch: {}", e);
            e
        })?;
    batch_data.gm_prices_min = min_prices;
    batch_data.gm_prices_max = max_prices;
    
    // Extract just market props for open interest calls
    let market_props: Vec<MarketProps> = markets.iter()
        .map(|(props, _)| props.clone())
        .collect();
    
    // Batch 3: Open interest
    debug!("Fetching open interest batch");
    let (long_oi, short_oi) = super::datastore::get_open_interest_batch(config, &market_props).await
        .map_err(|e| {
            error!("Failed to fetch open interest batch: {}", e);
            e
        })?;
    batch_data.open_interest_long = long_oi;
    batch_data.open_interest_short = short_oi;
    
    // Batch 4: Open interest in tokens
    debug!("Fetching open interest in tokens batch");
    let (long_oi_tokens, short_oi_tokens) = super::datastore::get_open_interest_in_tokens_batch(config, &market_props).await
        .map_err(|e| {
            error!("Failed to fetch open interest in tokens batch: {}", e);
            e
        })?;
    batch_data.open_interest_tokens_long = long_oi_tokens;
    batch_data.open_interest_tokens_short = short_oi_tokens;
    
    debug!(
        market_count = markets.len(),
        market_infos = batch_data.market_infos.len(),
        gm_prices_min = batch_data.gm_prices_min.len(),
        gm_prices_max = batch_data.gm_prices_max.len(),
        open_interest_long = batch_data.open_interest_long.len(),
        open_interest_short = batch_data.open_interest_short.len(),
        "Batch fetch of all market data completed"
    );
    
    Ok(batch_data)
}
