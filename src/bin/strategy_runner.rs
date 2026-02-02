use chrono::{Duration, Utc};
use dotenvy::dotenv;
use futures::StreamExt;
use redis::AsyncCommands;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use std::sync::Arc;
use tokio::time;
use tracing::{error, info, instrument, warn};

use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::db::db_manager::DbManager;
use crypto_yield_farming_bot::db::models::strategy_runs::NewStrategyRunModel;
use crypto_yield_farming_bot::db::models::strategy_targets::NewStrategyTargetModel;
use crypto_yield_farming_bot::hedging::dydx_client::DydxClient;
use crypto_yield_farming_bot::logging;
use crypto_yield_farming_bot::strategy::engine;
use crypto_yield_farming_bot::strategy::types::PortfolioData;
use crypto_yield_farming_bot::wallet::WalletManager;

const DATA_COLLECTION_COMPLETED_CHANNEL: &str = "data_collection_completed";
const STRATEGY_RUN_COMPLETED_CHANNEL: &str = "strategy_run_completed";

#[instrument(name = "strategy_runner_main")]
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

    // Initialize database manager
    let db_manager = DbManager::init(&cfg).await?;
    let mut db_manager = Arc::new(db_manager);
    info!("Database manager initialized");

    // Initialize and load wallet manager
    let mut wallet_manager = WalletManager::new(&cfg)?;
    wallet_manager.load_tokens(&db_manager).await?;
    let wallet_manager = Arc::new(wallet_manager);
    info!("Wallet manager initialized");

    // Initialize dydx client
    let dydx_client = DydxClient::new(cfg.clone(), wallet_manager.clone()).await?;
    let dydx_client = Arc::new(tokio::sync::Mutex::new(dydx_client));
    info!("dYdX client initialized");

    // Create Redis client
    let redis_client = redis::Client::open("redis://redis:6379")?;
    let mut pubsub = redis_client.get_async_pubsub().await?;
    pubsub.subscribe(DATA_COLLECTION_COMPLETED_CHANNEL).await?;
    let mut messages = pubsub.on_message();
    let mut publish_conn = redis_client.get_multiplexed_async_connection().await?;

    // Get the timestamp of the last strategy run
    let mut last_run_time = db_manager.get_latest_strategy_run().await?.map(|run| run.timestamp);

    info!("Waiting for data_collection_completed signals");
    loop {
        let now = Utc::now();
        let ready_at = last_run_time.map(|t| t + Duration::minutes(30));

        if let Some(ready_at) = ready_at {
            if now < ready_at {
                let wait = (ready_at - now).to_std().unwrap_or_else(|_| std::time::Duration::from_secs(0));
                tokio::select! {
                    msg = messages.next() => {
                        if let Some(msg) = msg {
                            let payload: String = msg.get_payload().unwrap_or_default();
                            info!(payload = %payload, "Received data collection completion signal");
                            if let Some(last_run_time) = last_run_time {
                                let elapsed = Utc::now() - last_run_time;
                                if elapsed < Duration::minutes(30) {
                                    info!(elapsed_minutes = %elapsed.num_minutes(), "Skipping strategy run due to cadence");
                                }
                            }
                        } else {
                            warn!("Data collection pubsub stream ended");
                            break;
                        }
                    }
                    _ = time::sleep(wait) => {
                        continue;
                    }
                }
                continue;
            }
        }

        match time::timeout(std::time::Duration::from_secs(600), messages.next()).await {
            Ok(Some(msg)) => { // 30 minutes have passed and we received a data collection completed signal
                let payload: String = msg.get_payload().unwrap_or_default();
                info!(payload = %payload, "Received data collection completion signal");

                let run_started_at = Utc::now();
                let portfolio_data = engine::run_strategy_engine(db_manager.clone(), dydx_client.clone()).await?;
                info!(
                    "Strategy engine completed with {} markets",
                    portfolio_data.market_addresses.len()
                );
                portfolio_data.log_portfolio_data();

                if let Err(e) = record_strategy_run(&mut db_manager, &cfg, run_started_at, &portfolio_data).await {
                    error!(error = ?e, "Failed to record strategy run");
                } else {
                    let _: () = publish_conn
                        .publish(STRATEGY_RUN_COMPLETED_CHANNEL, run_started_at.to_rfc3339())
                        .await
                        .unwrap_or(());
                }

                last_run_time = Some(run_started_at);
            }
            Ok(None) => {
                warn!("Data collection pubsub stream ended");
                break;
            }
            Err(_) => {
                error!("Did not receive data collection completion signal within 10 minutes after cadence window");
                break;
            }
        }
    }

    Ok(())
}

async fn record_strategy_run(
    db_manager: &mut Arc<DbManager>,
    cfg: &config::Config,
    run_started_at: chrono::DateTime<Utc>,
    portfolio_data: &PortfolioData,
) -> eyre::Result<i32> {
    if let Some(db_manager_mut) = Arc::get_mut(db_manager) {
        db_manager_mut.refresh_id_maps().await?;
    }

    let total_weight = portfolio_data.weights.sum();
    let portfolio_return = portfolio_data.weights.dot(&portfolio_data.expected_returns);
    let portfolio_variance = portfolio_data
        .weights
        .dot(&portfolio_data.covariance_matrix.dot(&portfolio_data.weights));
    let portfolio_volatility = portfolio_variance.sqrt().unwrap_or(Decimal::ZERO);
    let sharpe = if portfolio_volatility > Decimal::ZERO {
        portfolio_return / portfolio_volatility
    } else {
        Decimal::ZERO
    };

    let run = NewStrategyRunModel {
        timestamp: run_started_at,
        strategy_version: cfg.strategy_version.clone(),
        total_weight,
        expected_return_bps: portfolio_return * Decimal::from_f64(10000.0).unwrap(),
        volatility_bps: portfolio_volatility * Decimal::from_f64(10000.0).unwrap(),
        sharpe,
    };
    let run_id = db_manager.insert_strategy_run(&run).await?;

    for (i, market) in portfolio_data.market_addresses.iter().enumerate() {
        if let Some(market_id) = db_manager.market_id_map.get(market).cloned() {
            let mkt_return = portfolio_data.expected_returns[i];
            let mkt_variance = portfolio_data.covariance_matrix[[i, i]];
            let target = NewStrategyTargetModel {
                strategy_run_id: run_id,
                market_id,
                target_weight: portfolio_data.weights[i],
                expected_return_bps: mkt_return * Decimal::from_f64(10000.0).unwrap(),
                variance_bps: mkt_variance * Decimal::from_f64(10000.0).unwrap(),
            };
            db_manager.insert_strategy_target(&target).await?;
        }
    }

    Ok(run_id)
}
