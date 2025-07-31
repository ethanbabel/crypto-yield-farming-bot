use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use reqwest_retry::{RetryTransientMiddleware, policies::ExponentialBackoff};
use governor::{Quota, DefaultDirectRateLimiter};
use nonzero_ext::*;
use std::time::Duration;
use std::str::FromStr;
use std::sync::Arc;
use tracing::{warn, instrument};
use url::Url;
use ethers::types::{Address, Bytes, U256};
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use eyre::Result;

use super::types::{QuoteRequest, QuoteResponse, ParaSwapQuoteResponse};
use crate::config::Config;

const PARASWAP_BASE_URL: &str = "https://api.paraswap.io";

struct ParaswapRateLimiter {
    rate_limiter: Arc<DefaultDirectRateLimiter>,
}

impl reqwest_ratelimit::RateLimiter for ParaswapRateLimiter {
    async fn acquire_permit(&self) {
        self.rate_limiter.until_ready().await;
    }
}

#[derive(Debug, Clone)]
pub struct ParaSwapClient {
    http_client: ClientWithMiddleware,
    base_url: String,
    chain_id: u64,
    taker_address: Address,
}

impl ParaSwapClient {
    pub fn new(taker_address: Address, config: &Config) -> Self {
        let reqwest_client = reqwest_middleware::reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("Failed to create HTTP client");

        let retry_policy = ExponentialBackoff::builder()
            .retry_bounds(Duration::from_millis(500), Duration::from_millis(1000))
            .build_with_max_retries(3);

        let rate_limiter = ParaswapRateLimiter {
            rate_limiter: Arc::new(DefaultDirectRateLimiter::direct(Quota::per_second(nonzero!(1u32)))),
        };

        let http_client = ClientBuilder::new(reqwest_client)
            .with(RetryTransientMiddleware::new_with_policy(retry_policy))
            .with(reqwest_ratelimit::all(rate_limiter))
            .build();

        Self {
            http_client,
            base_url: PARASWAP_BASE_URL.to_string(),
            chain_id: config.chain_id,
            taker_address,
        }
    }

    /// Get quote for specific buy amount (ParaSwap's native feature)
    #[instrument(skip(self))]
    pub async fn get_quote(&self, request: &QuoteRequest) -> Result<QuoteResponse> {
        let amount = if request.side == "BUY" {
            self.decimal_to_u256_str(request.amount, request.to_token_decimals)
        } else {
            self.decimal_to_u256_str(request.amount, request.from_token_decimals)
        };
        let url = Url::parse(&format!("{}/swap", self.base_url))?;
        let params = [
            ("srcToken", format!("{:?}", request.from_token)),
            ("srcDecimals", request.from_token_decimals.to_string()),
            ("destToken", format!("{:?}", request.to_token)),
            ("destDecimals", request.to_token_decimals.to_string()),
            ("amount", amount),
            ("side", request.side.clone()),
            ("slippage", (request.slippage_tolerance * Decimal::from_f64(100.0).unwrap()).round().to_string()),
            ("userAddress", format!("{:?}", self.taker_address)),
            ("network", self.chain_id.to_string()),
            ("version", "6.2".to_string()), 
        ];

        let response = self.http_client.get(url).query(&params).send().await?.error_for_status()?;

        let quote_response: ParaSwapQuoteResponse = response.json().await?;
        tracing::debug!(?quote_response, "Received quote response from ParaSwap");
        let price_route = quote_response.price_route;

        Ok(QuoteResponse {
            from_token: Address::from_str(&price_route.src_token)?,
            to_token: Address::from_str(&price_route.dest_token)?,
            from_amount: self.u256_str_to_decimal(&price_route.src_amount, request.from_token_decimals),
            to_amount: self.u256_str_to_decimal(&price_route.dest_amount, request.to_token_decimals),
            from_amount_usd: Decimal::from_str(&price_route.src_usd)?,
            to_amount_usd: Decimal::from_str(&price_route.dest_usd)?,
            to_contract: Address::from_str(&quote_response.tx_params.to)?,
            transaction_data: Bytes::from_str(&quote_response.tx_params.data)?,
            value: U256::from_dec_str(&quote_response.tx_params.value)?,
        })
    }
       

    /// Convert decimal amount to wei (U256)
    fn decimal_to_u256_str(&self, amount: Decimal, decimals: u8) -> String {
        let amount_str = amount.to_string();
        let formatted = ethers::utils::parse_units(&amount_str, decimals as usize).unwrap_or_else(|_| {
            warn!("Failed to parse decimal value: {}", amount_str);
            ethers::utils::ParseUnits::U256(U256::zero())
        });
        formatted.to_string()
    }

    /// Convert wei string to decimal
    fn u256_str_to_decimal(&self, u256_str: &str, decimals: u8) -> Decimal {
        let u256 = U256::from_dec_str(u256_str).unwrap_or_else(|_| {
            warn!("Failed to parse U256 value: {}", u256_str);
            U256::zero()
        });
        let formatted = ethers::utils::format_units(u256, decimals as usize).unwrap_or_else(|_| {
            warn!("Failed to format U256 value: {}", u256);
            "0".to_string()
        });
        Decimal::from_str(&formatted).unwrap_or(Decimal::ZERO)
    }
}