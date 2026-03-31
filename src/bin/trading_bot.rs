use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::Utc;
use dotenvy::dotenv;
use ethers::types::Address;
use futures::StreamExt;
use ndarray::{Array1, Array2};
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use tokio::time;
use tracing::{error, info, instrument, warn};

use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::db::db_manager::DbManager;
use crypto_yield_farming_bot::db::models::markets::MarketModel;
use crypto_yield_farming_bot::db::models::strategy_runs::StrategyRunModel;
use crypto_yield_farming_bot::db::models::strategy_targets::StrategyTargetModel;
use crypto_yield_farming_bot::db::models::tokens::TokenModel;
use crypto_yield_farming_bot::execution::engine::ExecutionEngine;
use crypto_yield_farming_bot::hedging::dydx_client::DydxClient;
use crypto_yield_farming_bot::logging;
use crypto_yield_farming_bot::strategy::types::PortfolioData;
use crypto_yield_farming_bot::wallet::WalletManager;

const STRATEGY_RUN_COMPLETED_CHANNEL: &str = "strategy_run_completed";
const DEFAULT_HANG_TIMEOUT_SECS: u64 = 3000; // 50 minutes
const DEFAULT_RUN_TIMEOUT_SECS: u64 = 1200; // 20 minutes
const INIT_RETRY_BASE_SECS: u64 = 5;
const INIT_RETRY_MAX_SECS: u64 = 120;

#[instrument(name = "trading_bot_main")]
#[tokio::main]
async fn main() -> eyre::Result<()> {
    dotenv()?;

    if let Err(e) = logging::init_logging(env!("CARGO_BIN_NAME").to_string()) {
        eprintln!("Failed to initialize logging: {}", e);
        return Err(e.into());
    }

    let cfg = config::Config::load().await;
    info!(network_mode = %cfg.network_mode, "Configuration loaded and logging initialized");

    let db = init_db_manager_with_retry(&cfg).await?;
    info!("Database manager initialized");

    let redis_client = redis::Client::open("redis://redis:6379")?;
    let mut pubsub = redis_client.get_async_pubsub().await?;
    pubsub.subscribe(STRATEGY_RUN_COMPLETED_CHANNEL).await?;
    let mut messages = pubsub.on_message();

    let last_progress = Arc::new(AtomicU64::new(Utc::now().timestamp() as u64));
    let hang_timeout_secs = std::env::var("TRADING_BOT_HANG_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_HANG_TIMEOUT_SECS);
    let run_timeout_secs = std::env::var("TRADING_BOT_RUN_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_RUN_TIMEOUT_SECS);

    let watchdog_progress = last_progress.clone();
    tokio::spawn(async move {
        let mut interval = time::interval(std::time::Duration::from_secs(30));
        loop {
            interval.tick().await;
            let last = watchdog_progress.load(Ordering::Relaxed);
            let now = Utc::now().timestamp() as u64;
            if now.saturating_sub(last) > hang_timeout_secs {
                error!(
                    last_progress = last,
                    hang_timeout_secs,
                    "Trading bot appears hung; exiting for restart"
                );
                std::process::exit(1);
            }
        }
    });

    let mut last_executed_strategy_run_id = None::<i32>;

    info!("Waiting for strategy_run_completed signals");
    while let Some(msg) = messages.next().await {
        last_progress.store(Utc::now().timestamp() as u64, Ordering::Relaxed);
        let payload: String = msg.get_payload().unwrap_or_default();
        info!(payload = %payload, "Received strategy run completion signal");

        let latest_run = match db.get_latest_strategy_run().await? {
            Some(run) => run,
            None => {
                warn!("Received strategy run signal but no strategy run exists in the database");
                continue;
            }
        };

        if latest_run.strategy_version != cfg.strategy_version {
            warn!(
                strategy_run_id = latest_run.id,
                strategy_version = %latest_run.strategy_version,
                configured_version = %cfg.strategy_version,
                "Skipping strategy run with mismatched version"
            );
            continue;
        }

        if let Some(last_run_id) = last_executed_strategy_run_id {
            if latest_run.id <= last_run_id {
                info!(
                    strategy_run_id = latest_run.id,
                    last_executed_strategy_run_id = last_run_id,
                    "Skipping stale or duplicate strategy run completion signal"
                );
                continue;
            }
        }

        let executing_run_id = latest_run.id;

        let cycle = execute_strategy_run_cycle(
            cfg.clone(),
            db.clone(),
            latest_run,
            last_progress.clone(),
        );
        match time::timeout(std::time::Duration::from_secs(run_timeout_secs), cycle).await {
            Ok(Ok(())) => {
                last_executed_strategy_run_id = Some(executing_run_id);
            }
            Ok(Err(e)) => {
                error!(error = ?e, strategy_run_id = executing_run_id, "Trading bot cycle failed");
            }
            Err(_) => {
                error!(
                    run_timeout_secs,
                    strategy_run_id = executing_run_id,
                    "Trading bot cycle timed out"
                );
            }
        }
    }

    warn!("Strategy run pubsub stream ended");
    Ok(())
}

async fn init_db_manager_with_retry(cfg: &Arc<config::Config>) -> eyre::Result<Arc<DbManager>> {
    let mut init_attempt = 0u64;
    loop {
        match DbManager::init(cfg).await {
            Ok(db_manager) => return Ok(Arc::new(db_manager)),
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
                time::sleep(std::time::Duration::from_secs(retry_secs)).await;
            }
        }
    }
}

async fn execute_strategy_run_cycle(
    cfg: Arc<config::Config>,
    db: Arc<DbManager>,
    strategy_run: StrategyRunModel,
    last_progress: Arc<AtomicU64>,
) -> eyre::Result<()> {
    last_progress.store(Utc::now().timestamp() as u64, Ordering::Relaxed);

    let mut wallet_manager = WalletManager::new(&cfg)?;
    wallet_manager.load_tokens(&db).await?;
    let wallet_manager = Arc::new(wallet_manager);
    info!(strategy_run_id = strategy_run.id, "Wallet manager refreshed");

    last_progress.store(Utc::now().timestamp() as u64, Ordering::Relaxed);

    let dydx_client = DydxClient::new(cfg.clone(), wallet_manager.clone()).await?;
    let dydx_client = Arc::new(tokio::sync::Mutex::new(dydx_client));
    info!(strategy_run_id = strategy_run.id, "dYdX client refreshed");

    let execution_engine = ExecutionEngine::new(
        cfg.clone(),
        db.clone(),
        wallet_manager.clone(),
        dydx_client,
    );

    last_progress.store(Utc::now().timestamp() as u64, Ordering::Relaxed);

    let portfolio_data = load_portfolio_data_for_run(&db, &strategy_run).await?;
    info!(
        strategy_run_id = strategy_run.id,
        market_count = portfolio_data.market_addresses.len(),
        "Loaded persisted strategy targets"
    );
    portfolio_data.log_portfolio_data();

    last_progress.store(Utc::now().timestamp() as u64, Ordering::Relaxed);

    execution_engine
        .run_once_with_existing_strategy_run(&portfolio_data, strategy_run.id)
        .await?;
    info!(strategy_run_id = strategy_run.id, "Execution cycle completed");

    last_progress.store(Utc::now().timestamp() as u64, Ordering::Relaxed);

    Ok(())
}

async fn load_portfolio_data_for_run(
    db: &Arc<DbManager>,
    strategy_run: &StrategyRunModel,
) -> eyre::Result<PortfolioData> {
    let mut targets = db.get_strategy_targets_for_runs(&[strategy_run.id]).await?;
    if targets.is_empty() {
        return Err(eyre::eyre!(
            "strategy_run_id={} has no persisted strategy targets",
            strategy_run.id
        ));
    }

    let market_map: HashMap<i32, MarketModel> = db
        .get_all_markets()
        .await?
        .into_iter()
        .map(|market| (market.id, market))
        .collect();
    let token_map: HashMap<i32, TokenModel> = db
        .get_all_tokens()
        .await?
        .into_iter()
        .map(|token| (token.id, token))
        .collect();

    targets.sort_by_key(|target| target.market_id);

    let market_addresses = build_market_addresses(&targets, &market_map)?;
    let display_names = build_display_names(&targets, &market_map, &token_map)?;
    let expected_returns = build_expected_returns(&targets);
    let covariance_matrix = build_covariance_matrix(&targets);
    let weights = Array1::from_vec(targets.iter().map(|target| target.target_weight).collect());

    Ok(PortfolioData::new(
        market_addresses,
        display_names,
        expected_returns,
        covariance_matrix,
        weights,
    ))
}

fn build_market_addresses(
    targets: &[StrategyTargetModel],
    market_map: &HashMap<i32, MarketModel>,
) -> eyre::Result<Vec<Address>> {
    targets
        .iter()
        .map(|target| {
            let market = market_map.get(&target.market_id).ok_or_else(|| {
                eyre::eyre!(
                    "strategy target references missing market_id={}",
                    target.market_id
                )
            })?;
            Address::from_str(&market.address).map_err(|e| {
                eyre::eyre!(
                    "failed to parse market address '{}' for market_id={}: {}",
                    market.address,
                    market.id,
                    e
                )
            })
        })
        .collect()
}

fn build_display_names(
    targets: &[StrategyTargetModel],
    market_map: &HashMap<i32, MarketModel>,
    token_map: &HashMap<i32, TokenModel>,
) -> eyre::Result<Vec<String>> {
    targets
        .iter()
        .map(|target| {
            let market = market_map.get(&target.market_id).ok_or_else(|| {
                eyre::eyre!(
                    "strategy target references missing market_id={}",
                    target.market_id
                )
            })?;
            let index_token = token_map.get(&market.index_token_id).ok_or_else(|| {
                eyre::eyre!(
                    "market_id={} references missing index_token_id={}",
                    market.id,
                    market.index_token_id
                )
            })?;
            let long_token = token_map.get(&market.long_token_id).ok_or_else(|| {
                eyre::eyre!(
                    "market_id={} references missing long_token_id={}",
                    market.id,
                    market.long_token_id
                )
            })?;
            let short_token = token_map.get(&market.short_token_id).ok_or_else(|| {
                eyre::eyre!(
                    "market_id={} references missing short_token_id={}",
                    market.id,
                    market.short_token_id
                )
            })?;
            Ok(format!(
                "{}/USD [{} - {}]",
                index_token.symbol, long_token.symbol, short_token.symbol
            ))
        })
        .collect()
}

fn build_expected_returns(targets: &[StrategyTargetModel]) -> Array1<Decimal> {
    let bps_scale = Decimal::from_i32(10000).unwrap();
    Array1::from_vec(
        targets
            .iter()
            .map(|target| target.expected_return_bps.unwrap_or(Decimal::ZERO) / bps_scale)
            .collect(),
    )
}

fn build_covariance_matrix(targets: &[StrategyTargetModel]) -> Array2<Decimal> {
    let n = targets.len();
    let bps_scale = Decimal::from_i32(10000).unwrap();
    let mut covariance = Array2::zeros((n, n));
    for (idx, target) in targets.iter().enumerate() {
        covariance[[idx, idx]] = target.variance_bps.unwrap_or(Decimal::ZERO) / bps_scale;
    }
    covariance
}
