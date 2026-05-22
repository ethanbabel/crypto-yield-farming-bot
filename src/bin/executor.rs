use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::Utc;
use dotenvy::dotenv;
use ethers::types::Address;
use futures::StreamExt;
use tokio::time;
use tracing::{error, info, instrument, warn};

use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::db::db_manager::DbManager;
use crypto_yield_farming_bot::db::models::execution_control_state::{
    EXECUTION_MODE_STABLE_ONLY, ExecutionControlStateModel,
};
use crypto_yield_farming_bot::db::models::markets::MarketModel;
use crypto_yield_farming_bot::db::models::strategy_runs::StrategyRunModel;
use crypto_yield_farming_bot::db::models::strategy_targets::StrategyTargetModel;
use crypto_yield_farming_bot::execution::engine::ExecutionEngine;
use crypto_yield_farming_bot::execution::types::ExecutionTargets;
use crypto_yield_farming_bot::hedging::dydx_client::DydxClient;
use crypto_yield_farming_bot::logging;
use crypto_yield_farming_bot::wallet::WalletManager;

const STRATEGY_RUN_COMPLETED_CHANNEL: &str = "strategy_run_completed";
const DATA_COLLECTION_COMPLETED_CHANNEL: &str = "data_collection_completed";
const EXECUTION_CONTROL_CHANNEL: &str = "execution_control";
const DEFAULT_HANG_TIMEOUT_SECS: u64 = 3000; // 50 minutes
const DEFAULT_RUN_TIMEOUT_SECS: u64 = 1200; // 20 minutes
const DEFAULT_STABLE_ONLY_TICK_SECS: u64 = 180;
const INIT_RETRY_BASE_SECS: u64 = 5;
const INIT_RETRY_MAX_SECS: u64 = 120;

#[instrument(name = "executor_main")]
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
    pubsub.subscribe(DATA_COLLECTION_COMPLETED_CHANNEL).await?;
    pubsub.subscribe(EXECUTION_CONTROL_CHANNEL).await?;
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
    let stable_only_tick_secs = std::env::var("TRADING_BOT_STABLE_ONLY_TICK_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_STABLE_ONLY_TICK_SECS);

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

    let mut stable_only_tick = time::interval(std::time::Duration::from_secs(stable_only_tick_secs));
    stable_only_tick.tick().await;

    let mut last_executed_strategy_run_id = None::<i32>;
    let mut control_state = db.get_execution_control_state().await?;

    if control_state.mode == EXECUTION_MODE_STABLE_ONLY {
        run_unwind_cycle_with_timeout(
            cfg.clone(),
            db.clone(),
            &control_state,
            last_progress.clone(),
            run_timeout_secs,
        )
        .await;
    }

    info!("Waiting for strategy and execution control signals");
    loop {
        tokio::select! {
            _ = stable_only_tick.tick() => {
                last_progress.store(Utc::now().timestamp() as u64, Ordering::Relaxed);
                control_state = db.get_execution_control_state().await?;
                if control_state.mode == EXECUTION_MODE_STABLE_ONLY {
                    run_unwind_cycle_with_timeout(
                        cfg.clone(),
                        db.clone(),
                        &control_state,
                        last_progress.clone(),
                        run_timeout_secs,
                    ).await;
                }
            }
            maybe_msg = messages.next() => {
                let Some(msg) = maybe_msg else {
                    warn!("Executor pubsub stream ended");
                    break;
                };

                last_progress.store(Utc::now().timestamp() as u64, Ordering::Relaxed);
                control_state = db.get_execution_control_state().await?;
                let channel = msg.get_channel_name().to_string();

                match channel.as_str() {
                    STRATEGY_RUN_COMPLETED_CHANNEL => {
                        handle_strategy_run_signal(
                            cfg.clone(),
                            db.clone(),
                            &control_state,
                            &msg,
                            &mut last_executed_strategy_run_id,
                            last_progress.clone(),
                            run_timeout_secs,
                        ).await?;
                    }
                    DATA_COLLECTION_COMPLETED_CHANNEL => {
                        let payload: String = msg.get_payload().unwrap_or_default();
                        info!(payload = %payload, "Received data collection completion signal");
                        run_observation_cycle_with_timeout(
                            cfg.clone(),
                            db.clone(),
                            last_progress.clone(),
                            run_timeout_secs,
                        ).await;
                    }
                    EXECUTION_CONTROL_CHANNEL => {
                        let payload: String = msg.get_payload().unwrap_or_default();
                        info!(payload = %payload, mode = %control_state.mode, "Received execution control signal");
                        if control_state.mode == EXECUTION_MODE_STABLE_ONLY {
                            run_unwind_cycle_with_timeout(
                                cfg.clone(),
                                db.clone(),
                                &control_state,
                                last_progress.clone(),
                                run_timeout_secs,
                            ).await;
                        }
                    }
                    other => {
                        warn!(channel = %other, "Received message on unexpected channel");
                    }
                }
            }
        }
    }

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

async fn handle_strategy_run_signal(
    cfg: Arc<config::Config>,
    db: Arc<DbManager>,
    control_state: &ExecutionControlStateModel,
    msg: &redis::Msg,
    last_executed_strategy_run_id: &mut Option<i32>,
    last_progress: Arc<AtomicU64>,
    run_timeout_secs: u64,
) -> eyre::Result<()> {
    let payload: String = msg.get_payload().unwrap_or_default();
    info!(payload = %payload, "Received strategy run completion signal");

    let strategy_run_id = match payload.trim().parse::<i32>() {
        Ok(run_id) => run_id,
        Err(e) => {
            warn!(payload = %payload, error = ?e, "Received invalid strategy run completion payload");
            return Ok(());
        }
    };

    if control_state.mode == EXECUTION_MODE_STABLE_ONLY {
        info!(
            strategy_run_id,
            target_stable_symbol = ?control_state.target_stable_symbol,
            "Ignoring strategy run because executor is in stable_only mode"
        );
        return Ok(());
    }

    let strategy_run = match db.get_strategy_run_by_id(strategy_run_id).await? {
        Some(run) => run,
        None => {
            warn!(
                strategy_run_id,
                "Received strategy run signal but no matching strategy run exists in the database"
            );
            return Ok(());
        }
    };

    if strategy_run.strategy_version != cfg.strategy_version {
        warn!(
            strategy_run_id = strategy_run.id,
            strategy_version = %strategy_run.strategy_version,
            configured_version = %cfg.strategy_version,
            "Skipping strategy run with mismatched version"
        );
        return Ok(());
    }

    if let Some(min_strategy_run_id) = control_state.min_strategy_run_id_to_execute {
        if strategy_run.id < min_strategy_run_id {
            info!(
                strategy_run_id = strategy_run.id,
                min_strategy_run_id_to_execute = min_strategy_run_id,
                "Skipping strategy run because it predates the resume gate"
            );
            return Ok(());
        }
    }

    if let Some(last_run_id) = *last_executed_strategy_run_id {
        if strategy_run.id <= last_run_id {
            info!(
                strategy_run_id = strategy_run.id,
                last_executed_strategy_run_id = last_run_id,
                "Skipping stale or duplicate strategy run completion signal"
            );
            return Ok(());
        }
    }

    let executing_run_id = strategy_run.id;
    info!(
        strategy_run_id = executing_run_id,
        "Starting execution cycle for completed strategy run"
    );
    let cycle = execute_strategy_run_cycle(cfg, db, strategy_run, last_progress);
    match time::timeout(std::time::Duration::from_secs(run_timeout_secs), cycle).await {
        Ok(Ok(())) => {
            *last_executed_strategy_run_id = Some(executing_run_id);
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

    Ok(())
}

async fn run_unwind_cycle_with_timeout(
    cfg: Arc<config::Config>,
    db: Arc<DbManager>,
    control_state: &ExecutionControlStateModel,
    last_progress: Arc<AtomicU64>,
    run_timeout_secs: u64,
) {
    let target_stable_symbol = control_state
        .target_stable_symbol
        .clone()
        .unwrap_or_else(|| "USDC".to_string());
    let cycle = execute_unwind_cycle(cfg, db, target_stable_symbol.clone(), last_progress);
    match time::timeout(std::time::Duration::from_secs(run_timeout_secs), cycle).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            error!(
                error = ?e,
                target_stable_symbol = %target_stable_symbol,
                "Stable-only unwind cycle failed"
            );
        }
        Err(_) => {
            error!(
                run_timeout_secs,
                target_stable_symbol = %target_stable_symbol,
                "Stable-only unwind cycle timed out"
            );
        }
    }
}

async fn run_observation_cycle_with_timeout(
    cfg: Arc<config::Config>,
    db: Arc<DbManager>,
    last_progress: Arc<AtomicU64>,
    run_timeout_secs: u64,
) {
    let cycle = execute_observation_cycle(cfg, db, last_progress);
    match time::timeout(std::time::Duration::from_secs(run_timeout_secs), cycle).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            error!(error = ?e, "Observation snapshot cycle failed");
        }
        Err(_) => {
            error!(run_timeout_secs, "Observation snapshot cycle timed out");
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

    let execution_engine =
        ExecutionEngine::new(cfg.clone(), db.clone(), wallet_manager.clone(), dydx_client);

    last_progress.store(Utc::now().timestamp() as u64, Ordering::Relaxed);

    let execution_targets = load_execution_targets_for_run(&db, strategy_run.id).await?;
    info!(
        strategy_run_id = strategy_run.id,
        market_count = execution_targets.market_addresses.len(),
        "Loaded persisted strategy targets"
    );

    last_progress.store(Utc::now().timestamp() as u64, Ordering::Relaxed);

    if let Err(e) = execution_engine
        .run_once_with_existing_strategy_run(&execution_targets, strategy_run.id)
        .await
    {
        if let Err(snapshot_err) = execution_engine
            .record_current_snapshot(
                Some(strategy_run.id),
                "Execution cycle failed; recording best-effort failure snapshot",
            )
            .await
        {
            error!(
                strategy_run_id = strategy_run.id,
                error = ?snapshot_err,
                "Failed to record best-effort failure snapshot"
            );
        }
        return Err(e);
    }

    last_progress.store(Utc::now().timestamp() as u64, Ordering::Relaxed);

    Ok(())
}

async fn execute_observation_cycle(
    cfg: Arc<config::Config>,
    db: Arc<DbManager>,
    last_progress: Arc<AtomicU64>,
) -> eyre::Result<()> {
    last_progress.store(Utc::now().timestamp() as u64, Ordering::Relaxed);

    let mut wallet_manager = WalletManager::new(&cfg)?;
    wallet_manager.load_tokens(&db).await?;
    let wallet_manager = Arc::new(wallet_manager);
    info!("Wallet manager refreshed for observation snapshot");

    last_progress.store(Utc::now().timestamp() as u64, Ordering::Relaxed);

    let dydx_client = DydxClient::new(cfg.clone(), wallet_manager.clone()).await?;
    let dydx_client = Arc::new(tokio::sync::Mutex::new(dydx_client));
    info!("dYdX client refreshed for observation snapshot");

    let execution_engine =
        ExecutionEngine::new(cfg.clone(), db.clone(), wallet_manager.clone(), dydx_client);
    execution_engine
        .record_current_snapshot(None, "data collection completion signal")
        .await?;
    info!("Observation snapshot cycle completed");

    last_progress.store(Utc::now().timestamp() as u64, Ordering::Relaxed);

    Ok(())
}

async fn execute_unwind_cycle(
    cfg: Arc<config::Config>,
    db: Arc<DbManager>,
    target_stable_symbol: String,
    last_progress: Arc<AtomicU64>,
) -> eyre::Result<()> {
    last_progress.store(Utc::now().timestamp() as u64, Ordering::Relaxed);

    let mut wallet_manager = WalletManager::new(&cfg)?;
    wallet_manager.load_tokens(&db).await?;
    let wallet_manager = Arc::new(wallet_manager);
    info!(target_stable_symbol = %target_stable_symbol, "Wallet manager refreshed for unwind");

    last_progress.store(Utc::now().timestamp() as u64, Ordering::Relaxed);

    let dydx_client = DydxClient::new(cfg.clone(), wallet_manager.clone()).await?;
    let dydx_client = Arc::new(tokio::sync::Mutex::new(dydx_client));
    info!(target_stable_symbol = %target_stable_symbol, "dYdX client refreshed for unwind");

    let execution_engine =
        ExecutionEngine::new(cfg.clone(), db.clone(), wallet_manager.clone(), dydx_client);
    execution_engine
        .run_unwind_to_stable(&target_stable_symbol)
        .await?;
    info!(target_stable_symbol = %target_stable_symbol, "Stable-only unwind cycle completed");

    last_progress.store(Utc::now().timestamp() as u64, Ordering::Relaxed);

    Ok(())
}

async fn load_execution_targets_for_run(
    db: &Arc<DbManager>,
    strategy_run_id: i32,
) -> eyre::Result<ExecutionTargets> {
    let targets = db.get_strategy_targets_for_runs(&[strategy_run_id]).await?;
    if targets.is_empty() {
        return Err(eyre::eyre!(
            "strategy_run_id={} has no persisted strategy targets",
            strategy_run_id
        ));
    }

    let market_map: HashMap<i32, MarketModel> = db
        .get_all_markets()
        .await?
        .into_iter()
        .map(|market| (market.id, market))
        .collect();

    let market_addresses = build_market_addresses(&targets, &market_map)?;
    let weights = targets.iter().map(|target| target.target_weight).collect();

    Ok(ExecutionTargets::new(market_addresses, weights))
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
