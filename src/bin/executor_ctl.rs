use chrono::Utc;
use dotenvy::dotenv;
use eyre::{Result, eyre};
use redis::AsyncCommands;
use std::sync::Arc;
use tracing::info;

use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::db::db_manager::DbManager;
use crypto_yield_farming_bot::db::models::execution_control_events::NewExecutionControlEventModel;
use crypto_yield_farming_bot::db::models::execution_control_state::{
    EXECUTION_MODE_NORMAL, EXECUTION_MODE_STABLE_ONLY, NewExecutionControlStateModel,
};
use crypto_yield_farming_bot::logging;

const EXECUTION_CONTROL_CHANNEL: &str = "execution_control";
const DEFAULT_HOST_REDIS_URL: &str = "redis://127.0.0.1:6379";

/*
Usage:
cargo run --bin executor_ctl -- status

cargo run --bin executor_ctl -- \
    stay-stable \
    --target USDC \
    --reason "manual unwind before maintenance" \
    --updated-by ec2-user

cargo run --bin executor_ctl -- \
    resume-normal \
    --reason "maintenance complete; wait for next fresh strategy run" \
    --updated-by ec2-user

Notes:
- `status` prints the current durable execution control state from Postgres.
- `stay-stable` switches the executor into `stable_only` mode, records an audit event,
  and publishes a Redis wake-up so the running executor begins unwinding immediately.
- `resume-normal` switches the executor back to `normal` mode, records an audit event,
  and sets `min_strategy_run_id_to_execute` so the executor waits for the next fresh
  strategy run before re-entering positions.
- `--target` defaults to `USDC`.
- `--updated-by` defaults to the local `USER` env var, then falls back to `executor_ctl`.
- `REDIS_URL` may be overridden if needed, but the default host-side publish target is
  `redis://127.0.0.1:6379`.
*/

#[tokio::main]
async fn main() -> Result<()> {
    dotenv()?;

    if let Err(e) = logging::init_logging(env!("CARGO_BIN_NAME").to_string()) {
        eprintln!("Failed to initialize logging: {}", e);
        return Err(e.into());
    }

    let cfg = config::Config::load().await;
    let db = Arc::new(DbManager::init(&cfg).await?);

    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        return Err(eyre!(
            "usage: executor_ctl <status|stay-stable|resume-normal> [options]"
        ));
    }

    match args[0].as_str() {
        "status" => print_status(&db).await?,
        "stay-stable" => stay_stable(&db, &args[1..]).await?,
        "resume-normal" => resume_normal(&db, &args[1..]).await?,
        other => {
            return Err(eyre!(
                "unknown command '{}'; expected status, stay-stable, or resume-normal",
                other
            ));
        }
    }

    Ok(())
}

async fn print_status(db: &Arc<DbManager>) -> Result<()> {
    let state = db.get_execution_control_state().await?;
    println!("mode={}", state.mode);
    println!(
        "target_stable_symbol={}",
        state.target_stable_symbol.as_deref().unwrap_or("")
    );
    println!(
        "min_strategy_run_id_to_execute={}",
        state
            .min_strategy_run_id_to_execute
            .map(|v| v.to_string())
            .unwrap_or_default()
    );
    println!("updated_at={}", state.updated_at);
    println!(
        "updated_by={}",
        state.updated_by.as_deref().unwrap_or_default()
    );
    println!("reason={}", state.reason.as_deref().unwrap_or_default());
    Ok(())
}

async fn stay_stable(db: &Arc<DbManager>, args: &[String]) -> Result<()> {
    let target_stable_symbol =
        get_flag_value(args, "--target").unwrap_or_else(|| "USDC".to_string());
    let reason = get_flag_value(args, "--reason");
    let operator = resolve_operator(args);
    let now = Utc::now();

    let new_state = NewExecutionControlStateModel {
        mode: EXECUTION_MODE_STABLE_ONLY.to_string(),
        target_stable_symbol: Some(target_stable_symbol.clone()),
        min_strategy_run_id_to_execute: None,
        updated_at: now,
        updated_by: Some(operator.clone()),
        reason: reason.clone(),
    };
    let persisted = db.upsert_execution_control_state(&new_state).await?;

    let event = NewExecutionControlEventModel {
        timestamp: now,
        event_type: "set_mode".to_string(),
        mode: persisted.mode.clone(),
        target_stable_symbol: persisted.target_stable_symbol.clone(),
        min_strategy_run_id_to_execute: persisted.min_strategy_run_id_to_execute,
        updated_by: persisted.updated_by.clone(),
        reason: persisted.reason.clone(),
    };
    db.insert_execution_control_event(&event).await?;

    publish_control_signal("stay-stable").await?;
    info!(
        target_stable_symbol = %target_stable_symbol,
        updated_by = %operator,
        "Execution control state set to stable_only"
    );
    Ok(())
}

async fn resume_normal(db: &Arc<DbManager>, args: &[String]) -> Result<()> {
    let reason = get_flag_value(args, "--reason");
    let operator = resolve_operator(args);
    let latest_run_id = db.get_latest_strategy_run().await?.map(|run| run.id);
    let min_strategy_run_id_to_execute = latest_run_id.map(|run_id| run_id + 1);
    let now = Utc::now();

    let new_state = NewExecutionControlStateModel {
        mode: EXECUTION_MODE_NORMAL.to_string(),
        target_stable_symbol: None,
        min_strategy_run_id_to_execute,
        updated_at: now,
        updated_by: Some(operator.clone()),
        reason: reason.clone(),
    };
    let persisted = db.upsert_execution_control_state(&new_state).await?;

    let event = NewExecutionControlEventModel {
        timestamp: now,
        event_type: "set_mode".to_string(),
        mode: persisted.mode.clone(),
        target_stable_symbol: persisted.target_stable_symbol.clone(),
        min_strategy_run_id_to_execute: persisted.min_strategy_run_id_to_execute,
        updated_by: persisted.updated_by.clone(),
        reason: persisted.reason.clone(),
    };
    db.insert_execution_control_event(&event).await?;

    publish_control_signal("resume-normal").await?;
    info!(
        min_strategy_run_id_to_execute = ?min_strategy_run_id_to_execute,
        updated_by = %operator,
        "Execution control state set to normal"
    );
    Ok(())
}

async fn publish_control_signal(action: &str) -> Result<()> {
    let redis_url =
        std::env::var("REDIS_URL").unwrap_or_else(|_| DEFAULT_HOST_REDIS_URL.to_string());
    let redis_client = redis::Client::open(redis_url.as_str())?;
    let mut conn = redis_client.get_multiplexed_async_connection().await?;
    let payload = format!("{}:{}", action, Utc::now().to_rfc3339());
    let _: () = conn.publish(EXECUTION_CONTROL_CHANNEL, payload).await?;
    Ok(())
}

fn get_flag_value(args: &[String], flag: &str) -> Option<String> {
    args.windows(2)
        .find(|window| window[0] == flag)
        .map(|window| window[1].clone())
}

fn resolve_operator(args: &[String]) -> String {
    get_flag_value(args, "--updated-by")
        .or_else(|| std::env::var("USER").ok())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "executor_ctl".to_string())
}
