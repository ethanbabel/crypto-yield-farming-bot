use std::collections::HashMap;
use std::fmt;
use std::time::SystemTime;
use ethers::types::{Address, I256, U256};
use ethers::utils::to_checksum;
use eyre::Result;
use rust_decimal::Decimal;
use tracing::{warn, instrument, debug, error};
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::Config;
use crate::gmx::{
    reader_utils::{MarketPrices, MarketProps, MarketInfo, MarketPoolValueInfoProps},
    reader,
    datastore,
    event_listener_utils::MarketFees,
};
use crate::data_ingestion::token::token::AssetToken;
use super::market_utils::{
    self,
    i256_to_decimal_scaled, 
    // i256_to_decimal_scaled_decimals,
    u256_to_decimal_scaled,
    u256_to_decimal_scaled_decimals
};

#[derive(Debug)]
pub struct Market {
    // --- Market properties & data ---
    pub market_token: Address,
    pub index_token: Arc<RwLock<AssetToken>>,
    pub long_token: Arc<RwLock<AssetToken>>,
    pub short_token: Arc<RwLock<AssetToken>>,
    pub borrowing_factor_per_second: Option<market_utils::BorrowingFactorPerSecond>,
    pub has_supply: bool, // Indicates if the market has supply (won't for swap or deprecated markets), default is true
    pub pnl: Option<market_utils::Pnl>,
    pub token_pool: Option<market_utils::TokenPool>,
    pub gm_token_price: Option<market_utils::GmTokenPrice>,
    pub open_interest: Option<market_utils::OpenInterest>,
    pub current_utilization: Option<Decimal>,
    pub volume: market_utils::Volume,
    pub cumulative_fees: market_utils::CumulativeFees,

    // Timestamp of the last market data update
    pub updated_at: Option<SystemTime>,  
}

impl fmt::Display for Market {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let index = self.index_token.try_read().map(|t| t.symbol.clone()).unwrap_or("?".to_string());
        let long = self.long_token.try_read().map(|t| t.symbol.clone()).unwrap_or("?".to_string());
        let short = self.short_token.try_read().map(|t| t.symbol.clone()).unwrap_or("?".to_string());
        write!(
            f,
            "{}/USD [{} - {}], Total Fees: {}",
            index,
            long,
            short,
            if self.cumulative_fees.total_fees == Decimal::ZERO {
                "N/A".to_string()
            } else {
                format!("${:.5}", self.cumulative_fees.total_fees)
            }
        )
    }
}

impl Market {
    // Construct a MarketProps struct from the latest price data
    pub async fn market_props(&self) -> MarketProps {
        MarketProps {
            market_token: self.market_token,
            index_token: self.index_token.read().await.address,
            long_token: self.long_token.read().await.address,
            short_token: self.short_token.read().await.address,
        }
    }

    // Construct a MarketPrices struct from the latest price data
    pub async fn market_prices(&self) -> Option<MarketPrices> {
        Some(MarketPrices {
            index_token_price: self.index_token.read().await.price_props()?,
            long_token_price: self.long_token.read().await.price_props()?,
            short_token_price: self.short_token.read().await.price_props()?,
        })
    }

    // Fetch market info and pool info for the market
    #[instrument(skip(self, config, market_fees), fields(
        on_close = true,
        market_token = %self.market_token
    ))]
    pub async fn fetch_market_data(&mut self, config: &Config, market_fees: &MarketFees) -> Result<()> {
        let market_prices = self.market_prices().await;
        if let Some(prices) = market_prices {
            debug!("Market prices available, proceeding with data fetch");
            // Fetch market props for gmx
            let market_props = self.market_props().await;
            // Get read locks for tokens
            let index_token = self.index_token.read().await.clone();
            let long_token = self.long_token.read().await.clone();
            let short_token = self.short_token.read().await.clone();
            
            debug!(
                index_symbol = %index_token.symbol,
                long_symbol = %long_token.symbol,
                short_symbol = %short_token.symbol,
                "Token information loaded"
            );
            
            // Fetch market info and pool data
            let (market_info, gm_price_min, pool_info_min, gm_price_max, pool_info_max) = 
                self.fetch_market_info_and_pool_data(config, market_props.clone(), prices.clone()).await?;

            debug!("Market info and pool info fetched successfully");

            // Set borrowing factor per second
            self.borrowing_factor_per_second = Some(
                market_utils::BorrowingFactorPerSecond {
                    longs: u256_to_decimal_scaled(market_info.borrowing_factor_per_second_for_longs),
                    shorts: u256_to_decimal_scaled(market_info.borrowing_factor_per_second_for_shorts),
                }
            );

            // If pool_supply is zero, set has_supply to false
            let has_supply =  pool_info_min.pool_value != I256::zero() || pool_info_max.pool_value != I256::zero();
            self.has_supply = has_supply;
            
            debug!(
                has_supply = has_supply,
                pool_value_min = %pool_info_min.pool_value,
                pool_value_max = %pool_info_max.pool_value,
                "Supply status determined"
            );

            // Set GM token price
            self.gm_token_price = Some(
                market_utils::GmTokenPrice {
                    min: i256_to_decimal_scaled(gm_price_min),
                    max: i256_to_decimal_scaled(gm_price_max),
                    mid: i256_to_decimal_scaled((gm_price_min + gm_price_max) / I256::from(2)),
                }
            );

            // Set pnl
            self.pnl = Some(
                market_utils::Pnl {
                    long: i256_to_decimal_scaled(pool_info_min.long_pnl + pool_info_max.long_pnl) / Decimal::from(2),
                    short: i256_to_decimal_scaled(pool_info_min.short_pnl + pool_info_max.short_pnl) / Decimal::from(2),
                    net: i256_to_decimal_scaled(pool_info_min.net_pnl + pool_info_max.net_pnl) / Decimal::from(2),
                }
            );

            // Set token pool info
            self.token_pool = Some(
                market_utils::TokenPool {
                    long_token_amount: u256_to_decimal_scaled_decimals(pool_info_min.long_token_amount, long_token.decimals),
                    short_token_amount: u256_to_decimal_scaled_decimals(pool_info_min.short_token_amount, short_token.decimals),
                    long_token_usd: u256_to_decimal_scaled(pool_info_min.long_token_usd + pool_info_max.long_token_usd) / Decimal::from(2),
                    short_token_usd: u256_to_decimal_scaled(pool_info_min.short_token_usd + pool_info_max.short_token_usd) / Decimal::from(2),
                }
            );

            // Fetch open interest data
            let (long_open_interest, short_open_interest, long_open_interest_in_tokens, short_open_interest_in_tokens) = 
                self.fetch_open_interest_data(config, market_props.clone()).await?;

            // Set open interest
            self.open_interest = Some(
                market_utils::OpenInterest {
                    long: u256_to_decimal_scaled(long_open_interest),
                    short: u256_to_decimal_scaled(short_open_interest),
                    long_via_tokens: u256_to_decimal_scaled_decimals(long_open_interest_in_tokens, index_token.decimals) * index_token.last_mid_price_usd.unwrap(),
                    short_via_tokens: u256_to_decimal_scaled_decimals(short_open_interest_in_tokens, index_token.decimals) * index_token.last_mid_price_usd.unwrap(),
                }
            );

            // Calculate utilization
            let total_open_interest = self.open_interest.as_ref().unwrap().long + self.open_interest.as_ref().unwrap().short;
            let total_pool_liquidity = self.token_pool.as_ref().unwrap().long_token_usd + self.token_pool.as_ref().unwrap().short_token_usd;
            if total_pool_liquidity > Decimal::ZERO {
                self.current_utilization = Some(total_open_interest / total_pool_liquidity);
            } 

            // Build a map from address to token for efficient lookup
            let token_map: HashMap<Address, Arc<RwLock<AssetToken>>> = [
                (index_token.address, self.index_token.clone()),
                (long_token.address, self.long_token.clone()),
                (short_token.address, self.short_token.clone()),
            ].into_iter().collect();

            // Set volume and cumulative fees
            let mut swap_volume_total = Decimal::ZERO;
            for (token_address, volume) in &market_fees.swap_volume {
                if let Some(token) = token_map.get(token_address) {
                    let token = token.read().await;
                    let volume_usd = u256_to_decimal_scaled_decimals(*volume, token.decimals) * token.last_mid_price_usd.unwrap();
                    self.volume.swap += volume_usd;
                    swap_volume_total += volume_usd;
                } else {
                    warn!(
                        market = %self.market_token,
                        token = %to_checksum(token_address, None),
                        "Market has swap volume for unknown token"
                    );
                }
            }
            let trading_volume_usd = u256_to_decimal_scaled(market_fees.trading_volume);
            self.volume.trading += trading_volume_usd;

            debug!(
                swap_volume = %swap_volume_total,
                trading_volume = %trading_volume_usd,
                "Volume data updated"
            );

            let mut fee_types = [
                (&market_fees.position_fees, &mut self.cumulative_fees.position_fees),
                (&market_fees.liquidation_fees, &mut self.cumulative_fees.liquidation_fees),
                (&market_fees.swap_fees, &mut self.cumulative_fees.swap_fees),
                (&market_fees.borrowing_fees, &mut self.cumulative_fees.borrowing_fees),
            ];

            let mut total_fees_added = Decimal::ZERO;
            for (fee_map, field) in fee_types.iter_mut() {
                for (token_address, fee) in fee_map.iter() {
                    if let Some(token) = token_map.get(token_address) {
                        let token = token.read().await;
                        let fee_val = u256_to_decimal_scaled_decimals(*fee, token.decimals) * token.last_mid_price_usd.unwrap();
                        **field += fee_val;
                        self.cumulative_fees.total_fees += fee_val;
                        total_fees_added += fee_val;
                    } else {
                        warn!(
                            market = %self.market_token,
                            token = %to_checksum(token_address, None),
                            "Market has fee for unknown token"
                        );
                    }
                }
            }

            debug!(
                total_fees_added = %total_fees_added,
                cumulative_total = %self.cumulative_fees.total_fees,
                "Fee data updated"
            );

            // Update the timestamp of the last update
            self.updated_at = Some(SystemTime::now());

            debug!("Market data fetch completed successfully");
            Ok(())
        } else {
            error!("Market prices not available for market {}", self.market_token);
            eyre::bail!("Fetch market prices before fetching market data for market {}", self);
        }
    }

    // Helper method to fetch market info and pool data with retry logic
    async fn fetch_market_info_and_pool_data(
        &self,
        config: &Config,
        market_props: MarketProps,
        prices: MarketPrices,
    ) -> Result<(MarketInfo, I256, MarketPoolValueInfoProps, I256, MarketPoolValueInfoProps)> {
        const MAX_RETRIES: u32 = 3;
        let mut last_error = None;

        for attempt in 1..=MAX_RETRIES {
            match self.try_fetch_market_info_and_pool_data(config, market_props.clone(), prices.clone()).await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    last_error = Some(e);
                    if attempt < MAX_RETRIES {
                        let delay_ms = attempt * 500; // Linear backoff: 500ms, 1000ms
                        warn!(
                            market = %self.market_token,
                            attempt = attempt,
                            delay_ms = delay_ms,
                            error = %last_error.as_ref().unwrap(),
                            "Failed to fetch market info and pool data, retrying after delay"
                        );
                        tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms as u64)).await;
                    }
                }
            }
        }

        error!(
            market = %self.market_token,
            attempts = MAX_RETRIES,
            error = %last_error.as_ref().unwrap(),
            "Failed to fetch market info and pool data after all retries"
        );
        Err(last_error.unwrap())
    }

    // Internal method that performs the actual market info and pool data fetch
    async fn try_fetch_market_info_and_pool_data(
        &self,
        config: &Config,
        market_props: MarketProps,
        prices: MarketPrices,
    ) -> Result<(MarketInfo, I256, MarketPoolValueInfoProps, I256, MarketPoolValueInfoProps)> {
        let market_info = reader::get_market_info(config, self.market_token, prices.clone()).await?;
        let (gm_price_min, pool_info_min) = reader::get_market_token_price(
            config, market_props.clone(), prices.clone(), reader::PnlFactorType::Deposit, false
        ).await?;
        let (gm_price_max, pool_info_max) = reader::get_market_token_price(
            config, market_props.clone(), prices.clone(), reader::PnlFactorType::Deposit, true
        ).await?;

        Ok((market_info, gm_price_min, pool_info_min, gm_price_max, pool_info_max))
    }

    // Helper method to fetch open interest data with retry logic
    async fn fetch_open_interest_data(
        &self,
        config: &Config,
        market_props: MarketProps,
    ) -> Result<(U256, U256, U256, U256)> {
        const MAX_RETRIES: u32 = 3;
        let mut last_error = None;

        for attempt in 1..=MAX_RETRIES {
            match self.try_fetch_open_interest_data(config, market_props.clone()).await {
                Ok(result) => return Ok(result),
                Err(e) => {
                    last_error = Some(e);
                    if attempt < MAX_RETRIES {
                        let delay_ms = attempt * 500; // Linear backoff: 500ms, 1000ms
                        warn!(
                            market = %self.market_token,
                            attempt = attempt,
                            delay_ms = delay_ms,
                            error = %last_error.as_ref().unwrap(),
                            "Failed to fetch open interest data, retrying after delay"
                        );
                        tokio::time::sleep(tokio::time::Duration::from_millis(delay_ms as u64)).await;
                    }
                }
            }
        }

        error!(
            market = %self.market_token,
            attempts = MAX_RETRIES,
            error = %last_error.as_ref().unwrap(),
            "Failed to fetch open interest data after all retries"
        );
        Err(last_error.unwrap())
    }

    // Internal method that performs the actual open interest data fetch
    async fn try_fetch_open_interest_data(
        &self,
        config: &Config,
        market_props: MarketProps,
    ) -> Result<(U256, U256, U256, U256)> {
        let long_open_interest = datastore::get_open_interest(config, market_props.clone(), true).await?;
        let short_open_interest = datastore::get_open_interest(config, market_props.clone(), false).await?;
        let long_open_interest_in_tokens = datastore::get_open_interest_in_tokens(config, market_props.clone(), true).await?;
        let short_open_interest_in_tokens = datastore::get_open_interest_in_tokens(config, market_props.clone(), false).await?;

        Ok((long_open_interest, short_open_interest, long_open_interest_in_tokens, short_open_interest_in_tokens))
    }

    /// Zero out tracked fields (for each data collection cycle).
    pub fn zero_out_tracked_fields(&mut self) {
        self.borrowing_factor_per_second = None;
        self.pnl = None;
        self.token_pool = None;
        self.gm_token_price = None;
        self.open_interest = None;
        self.current_utilization = None;
        self.volume = market_utils::Volume::new();
        self.cumulative_fees = market_utils::CumulativeFees::new();
        self.updated_at = None;
    }
}
