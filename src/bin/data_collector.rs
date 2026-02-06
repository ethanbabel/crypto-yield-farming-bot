use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::data_ingestion::market::market_registry;
use crypto_yield_farming_bot::data_ingestion::token::token_registry;
use crypto_yield_farming_bot::db::db_manager::DbManager;
use crypto_yield_farming_bot::db::models::{
    dydx_perp_states::RawDydxPerpStateModel, dydx_perps::RawDydxPerpModel,
    market_states::RawMarketStateModel, markets::RawMarketModel, token_prices::RawTokenPriceModel,
    tokens::RawTokenModel,
};
use crypto_yield_farming_bot::gmx::event_fetcher::GmxEventFetcher;
use crypto_yield_farming_bot::hedging::dydx_client::DydxClient;
use crypto_yield_farming_bot::logging;
use crypto_yield_farming_bot::wallet::WalletManager;

use chrono::Utc;
use dotenvy::dotenv;
use redis::streams::StreamMaxlen;
use redis::AsyncCommands;
use rust_decimal::Decimal;
use std::collections::HashSet;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::interval;
use tracing::{debug, error, info, instrument};

const DEFAULT_HANG_TIMEOUT_SECS: u64 = 600; // 10m - data collector should run every 5 minutes so after 10m+ assume it's hung
const INIT_RETRY_BASE_SECS: u64 = 5;
const INIT_RETRY_MAX_SECS: u64 = 120;

#[instrument(name = "data_collector_main")]
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
    info!(network_mode = %cfg.network_mode, "Configuration loaded and logging initialized");

    // Initialize token registry
    let mut token_registry = token_registry::AssetTokenRegistry::new(&cfg);
    info!("Asset token registry initialized");

    // Initialize market registry
    let mut market_registry = market_registry::MarketRegistry::new(&cfg);
    info!("Market registry initialized");

    // Initialize database manager (for wallet manager token refresh) with retry
    let mut init_attempt = 0u64;
    let db = loop {
        match DbManager::init(&cfg).await {
            Ok(db) => break db,
            Err(e) => {
                init_attempt += 1;
                let retry_secs =
                    (INIT_RETRY_BASE_SECS.saturating_mul(init_attempt)).min(INIT_RETRY_MAX_SECS);
                error!(
                    error = ?e,
                    init_attempt,
                    retry_secs,
                    "Failed to initialize database manager, retrying"
                );
                tokio::time::sleep(Duration::from_secs(retry_secs)).await;
            }
        }
    };
    let db = Arc::new(db);
    info!("Database manager initialized");

    // Create Redis client
    let redis_client = redis::Client::open("redis://redis:6379")?;
    let mut redis_connection = redis_client.get_multiplexed_async_connection().await?;
    info!("Redis connection established");

    // Initialize the GMX event fetcher
    let mut event_fetcher = GmxEventFetcher::init(Arc::clone(&cfg.alchemy_provider), cfg.gmx_eventemitter);
    info!("GMX event fetcher initialized");

    // Track known dYdX perp tickers
    let mut known_dydx_perps: HashSet<String> = HashSet::new();

    let last_progress = Arc::new(AtomicU64::new(Utc::now().timestamp() as u64));
    let hang_timeout_secs = std::env::var("DATA_COLLECTOR_HANG_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_HANG_TIMEOUT_SECS);
    let watchdog_progress = last_progress.clone();
    tokio::spawn(async move {
        let mut interval = interval(Duration::from_secs(30));
        loop {
            interval.tick().await;
            let last = watchdog_progress.load(Ordering::Relaxed);
            let now = Utc::now().timestamp() as u64;
            if now.saturating_sub(last) > hang_timeout_secs {
                error!(
                    last_progress = last,
                    hang_timeout_secs, 
                    "Data collector appears hung; exiting for restart"
                );
                std::process::exit(1);
            }
        }
    });

    // Periodically update markets and save to database
    let mut ticker = interval(Duration::from_secs(300));
    info!("Starting main data collection loop with 300s interval");

    loop {
        ticker.tick().await;
        last_progress.store(Utc::now().timestamp() as u64, Ordering::Relaxed);
        info!("Data collection cycle started");
        let cycle_start = Utc::now();

        // Repopulate the market registry and get new tokens/markets
        let (new_tokens, new_market_addresses) =
            match market_registry.repopulate(&cfg, &mut token_registry).await {
                Ok(result) => result,
                Err(e) => {
                    error!(?e, "Failed to repopulate market registry");
                    continue;
                }
            };

        // If we found new tokens or markets, send them to Redis streams
        if !new_tokens.is_empty() || !new_market_addresses.is_empty() {
            last_progress.store(Utc::now().timestamp() as u64, Ordering::Relaxed);
            info!(
                new_token_count = new_tokens.len(),
                new_market_count = new_market_addresses.len(),
                new_tokens = ?new_tokens.iter().map(|t| t.symbol.clone()).collect::<Vec<_>>(),
                new_markets = ?new_market_addresses,
                "Detected new tokens/markets"
            );

            // Prepare and serialize new tokens directly from domain objects
            if !new_tokens.is_empty() {
                for token in &new_tokens {
                    let raw_token_model = RawTokenModel::from(token);
                    if let Ok(serialized) = serde_json::to_string(&raw_token_model) {
                        let _: () = redis_connection.xadd_maxlen("new_tokens", StreamMaxlen::Approx(1000), "*", &[("data", serialized)]).await?;
                    }
                    debug!(
                        token_address = %raw_token_model.address,
                        token_symbol = %raw_token_model.symbol,
                        "New token model serialized and sent through Redis"
                    );
                }
            }

            // Get full market data for new market addresses and prepare models
            if !new_market_addresses.is_empty() {
                for &market_address in &new_market_addresses {
                    if let Some(market) = market_registry.get_market(&market_address) {
                        let raw_market_model = RawMarketModel::from_async(market).await;
                        if let Ok(serialized) = serde_json::to_string(&raw_market_model) {
                            let _: () = redis_connection.xadd_maxlen("new_markets", StreamMaxlen::Approx(1000), "*", &[("data", serialized)]).await?;
                        }
                        debug!(
                            market_address = %raw_market_model.address,
                            "New market model serialized and sent through Redis"
                        );
                    }
                }
            }
        }

        // Fetch Asset Token price data from GMX
        last_progress.store(Utc::now().timestamp() as u64, Ordering::Relaxed);
        if let Err(e) = token_registry.update_all_gmx_prices().await {
            error!(?e, "Failed to update asset token prices from GMX");
            continue;
        }
        debug!("Asset token prices updated from GMX");

        // Fetch GMX fees
        last_progress.store(Utc::now().timestamp() as u64, Ordering::Relaxed);
        let fees_snapshot = match event_fetcher.fetch_fees().await {
            Ok(fees) => fees,
            Err(e) => {
                error!(?e, "Failed to fetch GMX fees");
                continue;
            }
        };
        debug!(fee_markets = fees_snapshot.len(), "Fee snapshot captured");

        // Update market data
        last_progress.store(Utc::now().timestamp() as u64, Ordering::Relaxed);
        if let Err(e) = market_registry
            .update_all_market_data(Arc::clone(&cfg), &fees_snapshot)
            .await
        {
            error!(?e, "Failed to update market data");
            continue;
        }

        // Get token_price models and serialize directly
        last_progress.store(Utc::now().timestamp() as u64, Ordering::Relaxed);
        let updated_tokens = token_registry.updated_tokens(cycle_start).await;
        let mut raw_token_prices = Vec::new();
        for token_arc in updated_tokens {
            let token = token_arc.read().await;
            if token.updated_at.is_some()
                && token.last_min_price_usd.is_some()
                && token.last_max_price_usd.is_some()
                && token.last_mid_price_usd.is_some()
            {
                raw_token_prices.push(RawTokenPriceModel::from(&*token));
            }
        }
        info!(
            new_token_prices_count = raw_token_prices.len(),
            "Raw token price models prepared"
        );

        // Get market_state models and serialize directly
        let updated_markets = market_registry.updated_markets(cycle_start);
        let mut raw_market_states = Vec::new();
        for market in updated_markets {
            if market.updated_at.is_some() {
                raw_market_states.push(RawMarketStateModel::from(market));
            }
        }
        info!(
            new_market_states_count = raw_market_states.len(),
            "Raw market state models prepared"
        );

        // Fetch dYdX perp markets and build perp state models
        last_progress.store(Utc::now().timestamp() as u64, Ordering::Relaxed);
        let mut raw_dydx_perps = Vec::new();
        let mut raw_dydx_perp_states = Vec::new();

        let mut wallet_manager = match WalletManager::new(&cfg) {
            Ok(manager) => manager,
            Err(e) => {
                error!(error = ?e, "Failed to initialize wallet manager");
                continue;
            }
        };
        if let Err(e) = wallet_manager.load_tokens(&db).await {
            error!(error = ?e, "Failed to load wallet tokens from database");
            continue;
        }
        let wallet_manager = Arc::new(wallet_manager);
        let dydx_client = match DydxClient::new(cfg.clone(), wallet_manager).await {
            Ok(client) => client,
            Err(e) => {
                error!(error = ?e, "Failed to initialize dYdX client");
                continue;
            }
        };
        info!("dYdX client initialized");

        let token_perp_map = match dydx_client.get_token_perp_map().await {
            Ok(map) => map,
            Err(e) => {
                error!(?e, "Failed to fetch dYdX token perp map");
                continue;
            }
        };

        let mut seen_perp_state_tickers: HashSet<String> = HashSet::new();
        for (token_symbol, market_opt) in token_perp_map {
            let Some(market) = market_opt else {
                continue;
            };
            let ticker = market.ticker.0.clone();

            if !known_dydx_perps.contains(&ticker) {
                raw_dydx_perps.push(RawDydxPerpModel {
                    token_symbol: token_symbol.clone(),
                    ticker: ticker.clone(),
                });
                known_dydx_perps.insert(ticker.clone());
            }

            if !seen_perp_state_tickers.insert(ticker.clone()) {
                continue;
            }

            raw_dydx_perp_states.push(RawDydxPerpStateModel {
                ticker,
                timestamp: cycle_start,
                funding_rate: Decimal::from_str(&market.next_funding_rate.to_plain_string()).ok(),
                initial_margin_fraction: Decimal::from_str(
                    &market.initial_margin_fraction.to_plain_string(),
                )
                .ok(),
                maintenance_margin_fraction: Decimal::from_str(
                    &market.maintenance_margin_fraction.to_plain_string(),
                )
                .ok(),
                oracle_price: market
                    .oracle_price
                    .as_ref()
                    .and_then(|p| Decimal::from_str(&p.to_plain_string()).ok()),
                open_interest: Decimal::from_str(&market.open_interest.to_plain_string()).ok(),
            });
        }

        info!(
            new_dydx_perp_state_count = raw_dydx_perp_states.len(),
            "Raw dYdX perp models prepared"
        );

        if !raw_dydx_perps.is_empty() {
            last_progress.store(Utc::now().timestamp() as u64, Ordering::Relaxed);
            for perp in raw_dydx_perps.iter() {
                if let Ok(serialized) = serde_json::to_string(perp) {
                    let _: () = redis_connection
                        .xadd_maxlen("dydx_perps", StreamMaxlen::Approx(1000), "*", &[("data", serialized)])
                        .await?;
                }
            }
            info!(
                new_dydx_perps_sent = raw_dydx_perps.len(),
                "Detected and sent new dYdX perp models. Serialized and sent through Redis"
            );
        }

        // Serialize token_price, market_state, and dYdX perp models
        let serialized_token_prices: Vec<String> = raw_token_prices
            .iter()
            .filter_map(|tp| serde_json::to_string(tp).ok())
            .collect();
        let serialized_market_states: Vec<String> = raw_market_states
            .iter()
            .filter_map(|ms| serde_json::to_string(ms).ok())
            .collect();
        let serialized_dydx_perp_states: Vec<String> = raw_dydx_perp_states
            .iter()
            .filter_map(|state| serde_json::to_string(state).ok())
            .collect();

        // Send token_price and market_state models to redis
        let token_count = serialized_token_prices.len();
        let market_count = serialized_market_states.len();
        let dydx_perp_state_count = serialized_dydx_perp_states.len();

        // Publish pre-emptive coordination event with expected counts
        let message = format!(
            "starting:{}:{}:{}",
            token_count, market_count, dydx_perp_state_count
        );
        let _: () = redis_connection.publish("data_collection_starting", message).await?;
        debug!(
            token_count = token_count,
            market_count = market_count,
            dydx_perp_state_count = dydx_perp_state_count,
            "Published data collection coordination event"
        );

        for tp in serialized_token_prices {
            last_progress.store(Utc::now().timestamp() as u64, Ordering::Relaxed);
            let _: () = redis_connection.xadd_maxlen("token_prices", StreamMaxlen::Approx(1000), "*", &[("data", tp)]).await?;
        }
        for ms in serialized_market_states {
            last_progress.store(Utc::now().timestamp() as u64, Ordering::Relaxed);
            let _: () = redis_connection.xadd_maxlen("market_states", StreamMaxlen::Approx(1000), "*", &[("data", ms)]).await?;
        }
        for state in serialized_dydx_perp_states {
            last_progress.store(Utc::now().timestamp() as u64, Ordering::Relaxed);
            let _: () = redis_connection.xadd_maxlen("dydx_perp_states", StreamMaxlen::Approx(1000), "*", &[("data", state)]).await?;
        }

        info!(
            token_count = token_count,
            market_count = market_count,
            dydx_perp_state_count = dydx_perp_state_count,
            "Data collection cycle completed"
        );

        // Zero out tracked fields for all markets at the end of the data collection loop
        market_registry.zero_all_tracked_fields();
    }
}
