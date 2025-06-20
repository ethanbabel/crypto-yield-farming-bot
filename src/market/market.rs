use std::collections::HashMap;
use std::fmt;
use std::time::SystemTime;
use ethers::types::{Address, I256, U256};
use ethers::utils::to_checksum;
use eyre::Result;
use rust_decimal::Decimal;
use tracing::warn;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::config::Config;
use crate::gmx::{
    reader_utils::{MarketPrices, MarketProps},
    reader,
    datastore,
    event_listener_utils::MarketFees,
};
use crate::token::token::AssetToken;
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
    pub async fn fetch_market_data(&mut self, config: &Config, market_fees: &MarketFees) -> Result<()> {
        let market_prices = self.market_prices().await;
        if let Some(prices) = market_prices {
            // Fetch market props for gmx
            let market_props = self.market_props().await;
            // Get read locks for tokens
            let index_token = self.index_token.read().await.clone();
            let long_token = self.long_token.read().await.clone();
            let short_token = self.short_token.read().await.clone();
            // Fetch market info
            let market_info = Some(reader::get_market_info(config, self.market_token, prices.clone()).await?);
            // Fetch pool info for deposit and withdrawal
            let (gm_price_min, pool_info_min) = reader::get_market_token_price(
                config, market_props.clone(), prices.clone(), reader::PnlFactorType::Deposit, false
            ).await?;
            let (gm_price_max, pool_info_max) = reader::get_market_token_price(
                config, market_props.clone(), prices.clone(), reader::PnlFactorType::Deposit, true
            ).await?;

            // Set borrowing factor per second
            self.borrowing_factor_per_second = Some(
                market_utils::BorrowingFactorPerSecond {
                    longs: u256_to_decimal_scaled(market_info.as_ref().map_or(U256::zero(), |m| m.borrowing_factor_per_second_for_longs)),
                    shorts: u256_to_decimal_scaled(market_info.as_ref().map_or(U256::zero(), |m| m.borrowing_factor_per_second_for_shorts)),
                }
            );

            // If pool_supply is zero, set has_supply to false
            self.has_supply =  pool_info_min.pool_value != I256::zero() || pool_info_max.pool_value != I256::zero();

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

            // Fetch long and short open interest
            let long_open_interest = datastore::get_open_interest(config, market_props.clone(), true).await?;
            let short_open_interest = datastore::get_open_interest(config, market_props.clone(), false).await?;

            // Fetch long and short open interest in tokens
            let long_open_interest_in_tokens = datastore::get_open_interest_in_tokens(config, market_props.clone(), true).await?;
            let short_open_interest_in_tokens = datastore::get_open_interest_in_tokens(config, market_props.clone(), false).await?;

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
            for (token_address, volume) in &market_fees.swap_volume {
                if let Some(token) = token_map.get(token_address) {
                    let token = token.read().await;
                    self.volume.swap += u256_to_decimal_scaled_decimals(*volume, token.decimals) * token.last_mid_price_usd.unwrap();
                } else {
                    warn!("Market {} has swap volume for unknown token {}", self.market_token, to_checksum(token_address, None));
                }
            }
            self.volume.trading += u256_to_decimal_scaled(market_fees.trading_volume);

            let mut fee_types = [
                (&market_fees.position_fees, &mut self.cumulative_fees.position_fees),
                (&market_fees.liquidation_fees, &mut self.cumulative_fees.liquidation_fees),
                (&market_fees.swap_fees, &mut self.cumulative_fees.swap_fees),
                (&market_fees.borrowing_fees, &mut self.cumulative_fees.borrowing_fees),
            ];

            for (fee_map, field) in fee_types.iter_mut() {
                for (token_address, fee) in fee_map.iter() {
                    if let Some(token) = token_map.get(token_address) {
                        let token = token.read().await;
                        let fee_val = u256_to_decimal_scaled_decimals(*fee, token.decimals) * token.last_mid_price_usd.unwrap();
                        **field += fee_val;
                        self.cumulative_fees.total_fees += fee_val;
                    } else {
                        warn!("Market {} has fee for unknown token {}", self.market_token, to_checksum(token_address, None));
                    }
                }
            }

            // Update the timestamp of the last update
            self.updated_at = Some(SystemTime::now());

            Ok(())
        } else {
            eyre::bail!("Fetch market prices before fetching market data for market {}", self);
        }
    }
}
