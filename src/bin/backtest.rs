use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDate, Utc};
use dotenvy::dotenv;
use eyre::{Context, Result, eyre};
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use serde::Serialize;

use crypto_yield_farming_bot::config;
use crypto_yield_farming_bot::db::db_manager::DbManager;
use crypto_yield_farming_bot::db::models::dydx_perp_states::DydxPerpStateModel;
use crypto_yield_farming_bot::db::models::dydx_perps::DydxPerpModel;
use crypto_yield_farming_bot::db::models::market_states::MarketStateModel;
use crypto_yield_farming_bot::db::models::markets::MarketModel;
use crypto_yield_farming_bot::db::models::strategy_runs::StrategyRunModel;
use crypto_yield_farming_bot::db::models::strategy_targets::StrategyTargetModel;
use crypto_yield_farming_bot::db::models::token_prices::TokenPriceModel;
use crypto_yield_farming_bot::db::models::tokens::TokenModel;
use crypto_yield_farming_bot::hedging::hedge_utils;
use crypto_yield_farming_bot::logging;
use tracing::{info, warn};

/*
Usage:
cargo run --bin backtest -- \
    --strategy-version v1 \
    --start 2025-01-01 \
    --end 2025-03-01 \
    --initial-capital 10000 \
    --output-dir ./backtest_output/v1_jan_to_mar_2025
*/

#[derive(Debug, Clone)]
struct CliArgs {
    strategy_version: String,
    start: Option<DateTime<Utc>>,
    end: Option<DateTime<Utc>>,
    initial_capital: Decimal,
    output_dir: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct MarketContext {
    market_address: String,
    display_name: String,
    long_token_id: i32,
    long_token_symbol: String,
    short_token_id: i32,
    short_token_symbol: String,
    short_is_stable: bool,
    long_is_stable: bool,
    dydx_perp_id: Option<i32>,
    dydx_ticker: Option<String>,
}

#[derive(Debug, Clone)]
struct MarketWindowResult {
    run_id: i32,
    strategy_version: String,
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
    market_id: i32,
    market_address: String,
    market_display_name: String,
    long_token_symbol: String,
    short_token_symbol: String,
    target_weight_raw: Decimal,
    target_weight_normalized: Decimal,
    package_capital_usd: Decimal,
    hedge_exposure_fraction: Decimal,
    desired_hedge_notional_usd: Decimal,
    actual_hedge_notional_usd: Decimal,
    required_hedge_margin_usd: Decimal,
    gm_capital_usd: Decimal,
    gm_entry_price: Decimal,
    gm_exit_price: Decimal,
    gm_token_quantity: Decimal,
    gm_pnl_usd: Decimal,
    perp_ticker: Option<String>,
    perp_entry_price: Decimal,
    perp_exit_price: Decimal,
    hedge_quantity: Decimal,
    hedge_mtm_pnl_usd: Decimal,
    funding_pnl_usd: Decimal,
    fee_attribution_usd: Decimal,
    long_token_entry_price_usd: Decimal,
    long_token_exit_price_usd: Decimal,
    long_token_return_pct: Decimal,
    short_token_entry_price_usd: Decimal,
    short_token_exit_price_usd: Decimal,
    short_token_return_pct: Decimal,
    gm_return_pct: Decimal,
    perp_return_pct: Decimal,
    entry_pool_value_usd: Decimal,
    entry_pool_share: Decimal,
    entry_long_token_amount_est: Decimal,
    entry_short_token_amount_est: Decimal,
    entry_long_token_exposure_usd_est: Decimal,
    entry_short_token_exposure_usd_est: Decimal,
    est_long_token_price_move_pnl_usd: Decimal,
    est_short_token_price_move_pnl_usd: Decimal,
    est_total_collateral_price_move_pnl_usd: Decimal,
    long_token_price_move_plus_hedge_mtm_pnl_usd: Decimal,
    gm_pnl_minus_est_collateral_price_move_pnl_usd: Decimal,
    notes: String,
}

#[derive(Debug, Clone)]
struct WindowResult {
    run_id: i32,
    strategy_version: String,
    window_start: DateTime<Utc>,
    window_end: DateTime<Utc>,
    start_nav_usd: Decimal,
    end_nav_usd: Decimal,
    gm_pnl_usd: Decimal,
    hedge_mtm_pnl_usd: Decimal,
    funding_pnl_usd: Decimal,
    fee_attribution_usd: Decimal,
    window_return_pct: Decimal,
    active_market_count: usize,
    target_count: usize,
}

#[derive(Debug, Serialize)]
struct SummaryConfig {
    strategy_version: String,
    start: Option<String>,
    end: Option<String>,
    initial_capital_usd: String,
    fee_mode: String,
    hedge_exposure_rule: String,
    hedge_leverage_rule: String,
    funding_rate_unit: String,
    price_selection_rule: String,
}

#[derive(Debug, Serialize)]
struct SummaryMetrics {
    window_count: usize,
    first_window_start: Option<String>,
    last_window_end: Option<String>,
    initial_nav_usd: String,
    final_nav_usd: String,
    total_pnl_usd: String,
    total_return_pct: String,
    annualized_return_pct: String,
    annualized_volatility_pct: String,
    sharpe: String,
    max_drawdown_pct: String,
    gm_pnl_usd: String,
    hedge_mtm_pnl_usd: String,
    funding_pnl_usd: String,
    fee_attribution_usd: String,
}

#[derive(Debug, Serialize)]
struct SummaryOutput {
    generated_at_utc: String,
    config: SummaryConfig,
    metrics: SummaryMetrics,
    warning_count: usize,
    warnings: Vec<String>,
    output_files: HashMap<String, String>,
}

#[derive(Debug)]
struct BacktestArtifacts {
    summary_json: PathBuf,
    windows_csv: PathBuf,
    market_windows_csv: PathBuf,
}

#[derive(Debug)]
struct HedgeResult {
    desired_hedge_notional_usd: Decimal,
    actual_hedge_notional_usd: Decimal,
    required_hedge_margin_usd: Decimal,
    gm_capital_usd: Decimal,
    perp_ticker: Option<String>,
    perp_entry_price: Decimal,
    perp_exit_price: Decimal,
    hedge_quantity: Decimal,
    hedge_mtm_pnl_usd: Decimal,
    funding_pnl_usd: Decimal,
    notes: Vec<String>,
    warnings: Vec<String>,
}

#[derive(Debug)]
struct TargetSimulation {
    market_result: Option<MarketWindowResult>,
    gm_pnl_usd: Decimal,
    hedge_mtm_pnl_usd: Decimal,
    funding_pnl_usd: Decimal,
    fee_attribution_usd: Decimal,
    active_market: bool,
    warnings: Vec<String>,
}

#[derive(Debug, Default)]
struct MarketDiagnostics {
    long_token_entry_price_usd: Decimal,
    long_token_exit_price_usd: Decimal,
    long_token_return_pct: Decimal,
    short_token_entry_price_usd: Decimal,
    short_token_exit_price_usd: Decimal,
    short_token_return_pct: Decimal,
    gm_return_pct: Decimal,
    perp_return_pct: Decimal,
    entry_pool_value_usd: Decimal,
    entry_pool_share: Decimal,
    entry_long_token_amount_est: Decimal,
    entry_short_token_amount_est: Decimal,
    entry_long_token_exposure_usd_est: Decimal,
    entry_short_token_exposure_usd_est: Decimal,
    est_long_token_price_move_pnl_usd: Decimal,
    est_short_token_price_move_pnl_usd: Decimal,
    est_total_collateral_price_move_pnl_usd: Decimal,
    long_token_price_move_plus_hedge_mtm_pnl_usd: Decimal,
    gm_pnl_minus_est_collateral_price_move_pnl_usd: Decimal,
}

impl TargetSimulation {
    fn skipped_with_warning(warning: String) -> Self {
        Self {
            market_result: None,
            gm_pnl_usd: Decimal::ZERO,
            hedge_mtm_pnl_usd: Decimal::ZERO,
            funding_pnl_usd: Decimal::ZERO,
            fee_attribution_usd: Decimal::ZERO,
            active_market: false,
            warnings: vec![warning],
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenv().ok();

    if let Err(e) = logging::init_logging(env!("CARGO_BIN_NAME").to_string()) {
        eprintln!("Failed to initialize logging: {}", e);
        return Err(e.into());
    }

    let args = CliArgs::parse()?;
    validate_args(&args)?;
    info!(
        strategy_version = %args.strategy_version,
        start = ?args.start,
        end = ?args.end,
        initial_capital_usd = %args.initial_capital,
        "Starting backtest"
    );

    let cfg = config::Config::load().await;
    let db_manager = DbManager::init(&cfg)
        .await
        .context("Failed to initialize database manager")?;

    let output_dir = resolve_output_dir(&args)?;
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("Failed to create output directory: {}", output_dir.display()))?;

    let mut warnings = Vec::new();

    let runs = db_manager
        .get_strategy_runs_for_version(&args.strategy_version, args.start, args.end)
        .await
        .context("Failed to fetch strategy runs")?;
    if runs.is_empty() {
        return Err(eyre!(
            "No strategy runs found for version '{}' in the requested range",
            args.strategy_version
        ));
    }
    info!(run_count = runs.len(), "Loaded strategy runs");

    let run_ids: Vec<i32> = runs.iter().map(|r| r.id).collect();
    let targets = db_manager
        .get_strategy_targets_for_runs(&run_ids)
        .await
        .context("Failed to fetch strategy targets")?;
    let targets_by_run = group_targets_by_run(targets);

    let markets = db_manager
        .get_all_markets()
        .await
        .context("Failed to fetch markets")?;
    let tokens = db_manager
        .get_all_tokens()
        .await
        .context("Failed to fetch tokens")?;
    let perps = db_manager
        .get_all_dydx_perps()
        .await
        .context("Failed to fetch dYdX perps")?;

    let market_map: HashMap<i32, MarketModel> = markets.into_iter().map(|m| (m.id, m)).collect();
    let token_map: HashMap<i32, TokenModel> = tokens.into_iter().map(|t| (t.id, t)).collect();

    let mut perps_by_token_id: HashMap<i32, &DydxPerpModel> = HashMap::new();
    let mut perps_by_ticker: HashMap<String, &DydxPerpModel> = HashMap::new();
    for perp in &perps {
        perps_by_token_id.insert(perp.token_id, perp);
        perps_by_ticker.insert(perp.ticker.clone(), perp);
    }

    let windows = build_windows(&runs, args.end);
    if windows.is_empty() {
        return Err(eyre!(
            "No rebalance windows available after applying requested boundaries"
        ));
    }

    let first_window_start = windows
        .first()
        .map(|(run_idx, _)| runs[*run_idx].timestamp)
        .unwrap();
    let last_window_end = windows.last().map(|(_, end)| *end).unwrap();

    let relevant_market_ids = collect_relevant_market_ids(&windows, &runs, &targets_by_run);

    let market_state_series = db_manager
        .get_market_state_series_for_markets(&relevant_market_ids, first_window_start, last_window_end)
        .await
        .context("Failed to fetch market state series")?;

    let market_contexts = build_market_contexts(
        &relevant_market_ids,
        &market_map,
        &token_map,
        &perps_by_token_id,
        &perps_by_ticker,
        &mut warnings,
    );

    let relevant_perp_ids: Vec<i32> = market_contexts
        .values()
        .filter_map(|ctx| ctx.dydx_perp_id)
        .collect();

    let relevant_token_ids = collect_relevant_token_ids(&market_contexts);

    let perp_state_series = db_manager
        .get_dydx_perp_state_series(&relevant_perp_ids, first_window_start, last_window_end)
        .await
        .context("Failed to fetch dYdX perp state series")?;

    let token_price_series = db_manager
        .get_token_price_series(&relevant_token_ids, first_window_start, last_window_end)
        .await
        .context("Failed to fetch token price series")?;

    let mut current_nav = args.initial_capital;
    let mut nav_points = Vec::new();
    nav_points.push((first_window_start, current_nav));

    let mut window_results = Vec::new();
    let mut market_results = Vec::new();
    let stable_short_hedge_fraction = Decimal::from_i32(1).unwrap() / Decimal::from_i32(2).unwrap();
    let total_windows = windows.len();
    let final_window_time = last_window_end;
    let mut next_progress_milestone_pct = 10usize;

    for (window_idx, (run_index, window_end)) in windows.into_iter().enumerate() {
        let cur_iteration_num = window_idx + 1;
        let percentage_complete = (cur_iteration_num as f64 / total_windows as f64) * 100.0;
        while next_progress_milestone_pct <= 100
            && percentage_complete + f64::EPSILON >= next_progress_milestone_pct as f64
        {
            info!(
                "Backtest progress: {:.1}% (window {} of {}) - current time: {}, final time: {}",
                percentage_complete,
                cur_iteration_num,
                total_windows,
                window_end.to_rfc3339(),
                final_window_time.to_rfc3339()
            );
            next_progress_milestone_pct += 25;
        }

        let run = &runs[run_index];
        let Some(run_targets) = targets_by_run.get(&run.id) else {
            warnings.push(format!(
                "run_id={} at {} has no strategy targets; skipping window",
                run.id,
                run.timestamp.to_rfc3339()
            ));
            continue;
        };

        let start_nav = current_nav;
        let mut window_gm_pnl = Decimal::ZERO;
        let mut window_hedge_mtm_pnl = Decimal::ZERO;
        let mut window_funding_pnl = Decimal::ZERO;
        let mut window_fee_attr = Decimal::ZERO;
        let mut active_markets = 0usize;

        let positive_weight_sum: Decimal = run_targets
            .iter()
            .map(|t| t.target_weight.max(Decimal::ZERO))
            .sum();

        if positive_weight_sum <= Decimal::ZERO {
            warnings.push(format!(
                "run_id={} at {} has non-positive total target weight; skipping window",
                run.id,
                run.timestamp.to_rfc3339()
            ));
            continue;
        }

        for target in run_targets {
            let raw_weight = target.target_weight.max(Decimal::ZERO);
            let normalized_weight = if positive_weight_sum > Decimal::ZERO {
                raw_weight / positive_weight_sum
            } else {
                Decimal::ZERO
            };
            let package_capital = start_nav * normalized_weight;
            if package_capital <= Decimal::ZERO {
                continue;
            }
            let simulation = simulate_target_window(
                run,
                target,
                window_end,
                normalized_weight,
                package_capital,
                stable_short_hedge_fraction,
                &market_contexts,
                &market_state_series,
                &perp_state_series,
                &token_price_series,
            );

            warnings.extend(simulation.warnings);

            if simulation.active_market {
                active_markets += 1;
            }

            window_gm_pnl += simulation.gm_pnl_usd;
            window_hedge_mtm_pnl += simulation.hedge_mtm_pnl_usd;
            window_funding_pnl += simulation.funding_pnl_usd;
            window_fee_attr += simulation.fee_attribution_usd;

            if let Some(market_result) = simulation.market_result {
                market_results.push(market_result);
            }
        }

        let total_pnl = window_gm_pnl + window_hedge_mtm_pnl + window_funding_pnl;
        current_nav = (current_nav + total_pnl).max(Decimal::ZERO);

        let window_return_pct = if start_nav > Decimal::ZERO {
            (current_nav / start_nav - Decimal::ONE) * Decimal::from_i32(100).unwrap()
        } else {
            Decimal::ZERO
        };

        window_results.push(WindowResult {
            run_id: run.id,
            strategy_version: run.strategy_version.clone(),
            window_start: run.timestamp,
            window_end,
            start_nav_usd: start_nav,
            end_nav_usd: current_nav,
            gm_pnl_usd: window_gm_pnl,
            hedge_mtm_pnl_usd: window_hedge_mtm_pnl,
            funding_pnl_usd: window_funding_pnl,
            fee_attribution_usd: window_fee_attr,
            window_return_pct,
            active_market_count: active_markets,
            target_count: run_targets.len(),
        });

        nav_points.push((window_end, current_nav));
    }

    if window_results.is_empty() {
        return Err(eyre!(
            "No windows were simulated after validation; cannot produce backtest output"
        ));
    }

    let artifacts = write_outputs(
        &output_dir,
        &window_results,
        &market_results,
        &warnings,
        &args,
        &runs,
        &nav_points,
    )?;
    info!("Backtest artifacts written");

    log_summary(&window_results, &warnings, &artifacts, &args);

    Ok(())
}

impl CliArgs {
    fn parse() -> Result<Self> {
        let mut strategy_version = None::<String>;
        let mut start = None::<DateTime<Utc>>;
        let mut end = None::<DateTime<Utc>>;
        let mut initial_capital = Decimal::from_i32(10000).unwrap();
        let mut output_dir = None::<PathBuf>;

        let mut iter = std::env::args().skip(1);
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--strategy-version" => {
                    let value = iter
                        .next()
                        .ok_or_else(|| eyre!("Missing value for --strategy-version"))?;
                    strategy_version = Some(value);
                }
                "--start" => {
                    let value = iter.next().ok_or_else(|| eyre!("Missing value for --start"))?;
                    start = Some(parse_datetime_utc(&value, false)?);
                }
                "--end" => {
                    let value = iter.next().ok_or_else(|| eyre!("Missing value for --end"))?;
                    end = Some(parse_datetime_utc(&value, true)?);
                }
                "--initial-capital" => {
                    let value = iter
                        .next()
                        .ok_or_else(|| eyre!("Missing value for --initial-capital"))?;
                    initial_capital = Decimal::from_str(&value)
                        .map_err(|_| eyre!("Invalid decimal for --initial-capital: {}", value))?;
                }
                "--output-dir" => {
                    let value = iter
                        .next()
                        .ok_or_else(|| eyre!("Missing value for --output-dir"))?;
                    output_dir = Some(PathBuf::from(value));
                }
                "--help" | "-h" => {
                    print_help();
                    std::process::exit(0);
                }
                _ => {
                    return Err(eyre!("Unknown argument: {}", arg));
                }
            }
        }

        let strategy_version = strategy_version.ok_or_else(|| {
            eyre!("--strategy-version is required (example: --strategy-version v1)")
        })?;

        Ok(Self {
            strategy_version,
            start,
            end,
            initial_capital,
            output_dir,
        })
    }
}

fn print_help() {
    println!(
        "backtest usage:\n\
         cargo run --bin backtest -- \\\n         --strategy-version <version> \\\n         [--start <YYYY-MM-DD|RFC3339>] \\\n         [--end <YYYY-MM-DD|RFC3339>] \\\n         [--initial-capital <decimal>] \\\n         [--output-dir <path>]"
    );
}

fn validate_args(args: &CliArgs) -> Result<()> {
    if args.initial_capital <= Decimal::ZERO {
        return Err(eyre!("--initial-capital must be > 0"));
    }

    if let (Some(start), Some(end)) = (args.start, args.end) {
        if start >= end {
            return Err(eyre!("--start must be strictly earlier than --end"));
        }
    }

    Ok(())
}

fn parse_datetime_utc(input: &str, end_of_day_if_date: bool) -> Result<DateTime<Utc>> {
    if let Ok(parsed) = DateTime::parse_from_rfc3339(input) {
        return Ok(parsed.with_timezone(&Utc));
    }

    if let Ok(date) = NaiveDate::parse_from_str(input, "%Y-%m-%d") {
        let naive_dt = if end_of_day_if_date {
            date.and_hms_opt(23, 59, 59)
        } else {
            date.and_hms_opt(0, 0, 0)
        }
        .ok_or_else(|| eyre!("Invalid date value: {}", input))?;

        return Ok(DateTime::<Utc>::from_naive_utc_and_offset(naive_dt, Utc));
    }

    Err(eyre!(
        "Invalid datetime '{}'. Use RFC3339 (e.g. 2026-02-01T00:00:00Z) or YYYY-MM-DD",
        input
    ))
}

fn resolve_output_dir(args: &CliArgs) -> Result<PathBuf> {
    if let Some(dir) = &args.output_dir {
        return Ok(dir.clone());
    }

    let now = Utc::now().format("%Y%m%d_%H%M%S").to_string();
    let version_safe = args
        .strategy_version
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect::<String>();

    Ok(PathBuf::from(format!(
        "backtest_output/{}_{}",
        version_safe, now
    )))
}

fn group_targets_by_run(
    targets: Vec<StrategyTargetModel>,
) -> HashMap<i32, Vec<StrategyTargetModel>> {
    let mut grouped: HashMap<i32, Vec<StrategyTargetModel>> = HashMap::new();
    for target in targets {
        grouped.entry(target.strategy_run_id).or_default().push(target);
    }
    grouped
}

fn build_windows(
    runs: &[StrategyRunModel],
    requested_end: Option<DateTime<Utc>>,
) -> Vec<(usize, DateTime<Utc>)> {
    let mut windows = Vec::new();

    for (i, run) in runs.iter().enumerate() {
        let next_time = runs.get(i + 1).map(|r| r.timestamp);

        let window_end = match (requested_end, next_time) {
            (Some(end), Some(next)) => end.min(next),
            (Some(end), None) => end,
            (None, Some(next)) => next,
            (None, None) => continue,
        };

        if window_end > run.timestamp {
            windows.push((i, window_end));
        }
    }

    windows
}

fn collect_relevant_market_ids(
    windows: &[(usize, DateTime<Utc>)],
    runs: &[StrategyRunModel],
    targets_by_run: &HashMap<i32, Vec<StrategyTargetModel>>,
) -> Vec<i32> {
    let mut seen = std::collections::BTreeSet::new();

    for (run_index, _) in windows {
        let run_id = runs[*run_index].id;
        if let Some(targets) = targets_by_run.get(&run_id) {
            for target in targets {
                seen.insert(target.market_id);
            }
        }
    }

    seen.into_iter().collect()
}

fn collect_relevant_token_ids(market_contexts: &HashMap<i32, MarketContext>) -> Vec<i32> {
    let mut seen = std::collections::BTreeSet::new();

    for ctx in market_contexts.values() {
        seen.insert(ctx.long_token_id);
        seen.insert(ctx.short_token_id);
    }

    seen.into_iter().collect()
}

fn build_market_contexts(
    market_ids: &[i32],
    market_map: &HashMap<i32, MarketModel>,
    token_map: &HashMap<i32, TokenModel>,
    perps_by_token_id: &HashMap<i32, &DydxPerpModel>,
    perps_by_ticker: &HashMap<String, &DydxPerpModel>,
    warnings: &mut Vec<String>,
) -> HashMap<i32, MarketContext> {
    let mut out = HashMap::new();

    for market_id in market_ids {
        let Some(market) = market_map.get(market_id) else {
            warnings.push(format!("market_id={} not found in markets table", market_id));
            continue;
        };

        let Some(long_token) = token_map.get(&market.long_token_id) else {
            warnings.push(format!(
                "market_id={} long_token_id={} not found in tokens table",
                market.id, market.long_token_id
            ));
            continue;
        };

        let Some(short_token) = token_map.get(&market.short_token_id) else {
            warnings.push(format!(
                "market_id={} short_token_id={} not found in tokens table",
                market.id, market.short_token_id
            ));
            continue;
        };

        let Some(index_token) = token_map.get(&market.index_token_id) else {
            warnings.push(format!(
                "market_id={} index_token_id={} not found in tokens table",
                market.id, market.index_token_id
            ));
            continue;
        };

        let display_name = format!(
            "{}/USD [{} - {}]",
            index_token.symbol, long_token.symbol, short_token.symbol
        );

        let short_is_stable = hedge_utils::STABLE_COINS.contains(&short_token.symbol.as_str());
        let long_is_stable = hedge_utils::STABLE_COINS.contains(&long_token.symbol.as_str());

        let (perp_id, ticker) = if long_is_stable {
            (None, None)
        } else if let Some(perp) = perps_by_token_id.get(&long_token.id) {
            (Some(perp.id), Some(perp.ticker.clone()))
        } else {
            let fallback_ticker = hedge_utils::get_dydx_perp_ticker(&long_token.symbol);
            match perps_by_ticker.get(&fallback_ticker) {
                Some(perp) => (Some(perp.id), Some(perp.ticker.clone())),
                None => (None, Some(fallback_ticker)),
            }
        };

        out.insert(
            *market_id,
            MarketContext {
                market_address: market.address.clone(),
                display_name,
                long_token_id: long_token.id,
                long_token_symbol: long_token.symbol.clone(),
                short_token_id: short_token.id,
                short_token_symbol: short_token.symbol.clone(),
                short_is_stable,
                long_is_stable,
                dydx_perp_id: perp_id,
                dydx_ticker: ticker,
            },
        );
    }

    out
}

fn simulate_target_window(
    run: &StrategyRunModel,
    target: &StrategyTargetModel,
    window_end: DateTime<Utc>,
    normalized_weight: Decimal,
    package_capital: Decimal,
    stable_short_hedge_fraction: Decimal,
    market_contexts: &HashMap<i32, MarketContext>,
    market_state_series: &HashMap<i32, Vec<MarketStateModel>>,
    perp_state_series: &HashMap<i32, Vec<DydxPerpStateModel>>,
    token_price_series: &HashMap<i32, Vec<TokenPriceModel>>,
) -> TargetSimulation {
    let Some(ctx) = market_contexts.get(&target.market_id) else {
        return TargetSimulation::skipped_with_warning(format!(
            "run_id={} market_id={} missing context; treating allocation as cash",
            run.id, target.market_id
        ));
    };

    let Some(market_series) = market_state_series.get(&target.market_id) else {
        return TargetSimulation::skipped_with_warning(format!(
            "run_id={} market_id={} has no market state series; treating allocation as cash",
            run.id, target.market_id
        ));
    };

    let mut notes = Vec::new();
    let mut warnings = Vec::new();

    let hedge_exposure_fraction = if ctx.short_is_stable {
        stable_short_hedge_fraction
    } else {
        Decimal::ONE
    };

    let mut gm_entry_price = Decimal::ZERO;
    let mut gm_exit_price = Decimal::ZERO;
    let mut gm_qty = Decimal::ZERO;
    let mut gm_pnl = Decimal::ZERO;

    let mut hedge_result = default_hedge_result(package_capital, ctx.dydx_ticker.clone());

    let start_market_state = latest_at_or_before_market(market_series, run.timestamp);
    let end_market_state = latest_at_or_before_market(market_series, window_end);

    if let (Some(start_state), Some(end_state)) = (start_market_state, end_market_state) {
        gm_entry_price = start_state.gm_price_mid.unwrap_or(Decimal::ZERO);
        gm_exit_price = end_state.gm_price_mid.unwrap_or(Decimal::ZERO);

        if gm_entry_price > Decimal::ZERO {
            if !ctx.long_is_stable {
                let desired_hedge_notional = package_capital * hedge_exposure_fraction;
                hedge_result = compute_hedge_result(
                    run,
                    target,
                    window_end,
                    package_capital,
                    desired_hedge_notional,
                    ctx,
                    perp_state_series,
                );
                notes.extend(hedge_result.notes.iter().cloned());
                warnings.extend(hedge_result.warnings.iter().cloned());
            }

            gm_qty = hedge_result.gm_capital_usd / gm_entry_price;
            gm_pnl = gm_qty * (gm_exit_price - gm_entry_price);
        } else {
            notes.push("GM entry price is non-positive; market package held as cash".to_string());
            warnings.push(format!(
                "run_id={} market_id={} has non-positive GM entry price at {}",
                run.id,
                target.market_id,
                run.timestamp.to_rfc3339()
            ));
        }
    } else {
        notes.push("Missing market start/end state; market package held as cash".to_string());
        warnings.push(format!(
            "run_id={} market_id={} missing market start/end state in window {} -> {}",
            run.id,
            target.market_id,
            run.timestamp.to_rfc3339(),
            window_end.to_rfc3339()
        ));
    }

    let fee_attr = calculate_fee_attribution(
        gm_qty,
        states_in_window_market(market_series, run.timestamp, window_end),
    );

    let diagnostics = compute_market_diagnostics(
        run,
        target,
        window_end,
        ctx,
        start_market_state,
        end_market_state,
        gm_qty,
        gm_entry_price,
        gm_exit_price,
        gm_pnl,
        &hedge_result,
        token_price_series,
        &mut notes,
        &mut warnings,
    );

    let market_result = MarketWindowResult {
        run_id: run.id,
        strategy_version: run.strategy_version.clone(),
        window_start: run.timestamp,
        window_end,
        market_id: target.market_id,
        market_address: ctx.market_address.clone(),
        market_display_name: ctx.display_name.clone(),
        long_token_symbol: ctx.long_token_symbol.clone(),
        short_token_symbol: ctx.short_token_symbol.clone(),
        target_weight_raw: target.target_weight,
        target_weight_normalized: normalized_weight,
        package_capital_usd: package_capital,
        hedge_exposure_fraction,
        desired_hedge_notional_usd: hedge_result.desired_hedge_notional_usd,
        actual_hedge_notional_usd: hedge_result.actual_hedge_notional_usd,
        required_hedge_margin_usd: hedge_result.required_hedge_margin_usd,
        gm_capital_usd: hedge_result.gm_capital_usd,
        gm_entry_price,
        gm_exit_price,
        gm_token_quantity: gm_qty,
        gm_pnl_usd: gm_pnl,
        perp_ticker: hedge_result.perp_ticker,
        perp_entry_price: hedge_result.perp_entry_price,
        perp_exit_price: hedge_result.perp_exit_price,
        hedge_quantity: hedge_result.hedge_quantity,
        hedge_mtm_pnl_usd: hedge_result.hedge_mtm_pnl_usd,
        funding_pnl_usd: hedge_result.funding_pnl_usd,
        fee_attribution_usd: fee_attr,
        long_token_entry_price_usd: diagnostics.long_token_entry_price_usd,
        long_token_exit_price_usd: diagnostics.long_token_exit_price_usd,
        long_token_return_pct: diagnostics.long_token_return_pct,
        short_token_entry_price_usd: diagnostics.short_token_entry_price_usd,
        short_token_exit_price_usd: diagnostics.short_token_exit_price_usd,
        short_token_return_pct: diagnostics.short_token_return_pct,
        gm_return_pct: diagnostics.gm_return_pct,
        perp_return_pct: diagnostics.perp_return_pct,
        entry_pool_value_usd: diagnostics.entry_pool_value_usd,
        entry_pool_share: diagnostics.entry_pool_share,
        entry_long_token_amount_est: diagnostics.entry_long_token_amount_est,
        entry_short_token_amount_est: diagnostics.entry_short_token_amount_est,
        entry_long_token_exposure_usd_est: diagnostics.entry_long_token_exposure_usd_est,
        entry_short_token_exposure_usd_est: diagnostics.entry_short_token_exposure_usd_est,
        est_long_token_price_move_pnl_usd: diagnostics.est_long_token_price_move_pnl_usd,
        est_short_token_price_move_pnl_usd: diagnostics.est_short_token_price_move_pnl_usd,
        est_total_collateral_price_move_pnl_usd: diagnostics.est_total_collateral_price_move_pnl_usd,
        long_token_price_move_plus_hedge_mtm_pnl_usd: diagnostics.long_token_price_move_plus_hedge_mtm_pnl_usd,
        gm_pnl_minus_est_collateral_price_move_pnl_usd: diagnostics.gm_pnl_minus_est_collateral_price_move_pnl_usd,
        notes: notes.join(" | "),
    };

    TargetSimulation {
        market_result: Some(market_result),
        gm_pnl_usd: gm_pnl,
        hedge_mtm_pnl_usd: hedge_result.hedge_mtm_pnl_usd,
        funding_pnl_usd: hedge_result.funding_pnl_usd,
        fee_attribution_usd: fee_attr,
        active_market: gm_qty > Decimal::ZERO,
        warnings,
    }
}

fn default_hedge_result(package_capital: Decimal, perp_ticker: Option<String>) -> HedgeResult {
    HedgeResult {
        desired_hedge_notional_usd: Decimal::ZERO,
        actual_hedge_notional_usd: Decimal::ZERO,
        required_hedge_margin_usd: Decimal::ZERO,
        gm_capital_usd: package_capital,
        perp_ticker,
        perp_entry_price: Decimal::ZERO,
        perp_exit_price: Decimal::ZERO,
        hedge_quantity: Decimal::ZERO,
        hedge_mtm_pnl_usd: Decimal::ZERO,
        funding_pnl_usd: Decimal::ZERO,
        notes: Vec::new(),
        warnings: Vec::new(),
    }
}

fn compute_hedge_result(
    run: &StrategyRunModel,
    target: &StrategyTargetModel,
    window_end: DateTime<Utc>,
    package_capital: Decimal,
    desired_hedge_notional: Decimal,
    ctx: &MarketContext,
    perp_state_series: &HashMap<i32, Vec<DydxPerpStateModel>>,
) -> HedgeResult {
    let mut result = default_hedge_result(package_capital, ctx.dydx_ticker.clone());
    result.desired_hedge_notional_usd = desired_hedge_notional;

    let Some(perp_id) = ctx.dydx_perp_id else {
        result
            .notes
            .push("No mapped dYdX perp; hedge PnL set to zero".to_string());
        result.warnings.push(format!(
            "run_id={} market_id={} long token {} has no dYdX perp mapping",
            run.id, target.market_id, ctx.long_token_symbol
        ));
        return result;
    };

    let Some(perp_series) = perp_state_series.get(&perp_id) else {
        result
            .notes
            .push("No perp state series available; hedge PnL set to zero".to_string());
        result.warnings.push(format!(
            "run_id={} market_id={} has no perp series for perp_id={}",
            run.id, target.market_id, perp_id
        ));
        return result;
    };

    let start_perp_state = latest_at_or_before_perp(perp_series, run.timestamp);
    let end_perp_state = latest_at_or_before_perp(perp_series, window_end);
    let (Some(perp_start), Some(perp_end)) = (start_perp_state, end_perp_state) else {
        result
            .notes
            .push("Missing perp start/end state; hedge PnL set to zero".to_string());
        result.warnings.push(format!(
            "run_id={} market_id={} missing perp start/end state in window {} -> {}",
            run.id,
            target.market_id,
            run.timestamp.to_rfc3339(),
            window_end.to_rfc3339()
        ));
        return result;
    };

    result.perp_entry_price = perp_start.oracle_price.unwrap_or(Decimal::ZERO);
    result.perp_exit_price = perp_end.oracle_price.unwrap_or(Decimal::ZERO);
    if result.perp_entry_price <= Decimal::ZERO {
        result
            .notes
            .push("Perp entry oracle price is non-positive; no hedge applied".to_string());
        return result;
    }

    let leverage = derive_effective_leverage(perp_start);
    if leverage <= Decimal::ZERO {
        result
            .notes
            .push("Perp leverage is zero; no hedge applied".to_string());
        return result;
    }

    let desired_margin = desired_hedge_notional / leverage;
    if desired_margin <= package_capital {
        result.required_hedge_margin_usd = desired_margin;
        result.actual_hedge_notional_usd = desired_hedge_notional;
    } else {
        result.required_hedge_margin_usd = package_capital;
        result.actual_hedge_notional_usd = package_capital * leverage;
        result.notes.push(format!(
            "Required hedge margin exceeded package capital; capped hedge notional to {}",
            result.actual_hedge_notional_usd
        ));
    }

    result.gm_capital_usd = (package_capital - result.required_hedge_margin_usd).max(Decimal::ZERO);
    result.hedge_quantity = -(result.actual_hedge_notional_usd / result.perp_entry_price);
    result.hedge_mtm_pnl_usd = result.hedge_quantity * (result.perp_exit_price - result.perp_entry_price);
    result.funding_pnl_usd = calculate_funding_pnl(
        result.hedge_quantity,
        perp_start,
        perp_end,
        states_in_window_perp(perp_series, run.timestamp, window_end),
    );

    result
}

fn compute_market_diagnostics(
    run: &StrategyRunModel,
    target: &StrategyTargetModel,
    window_end: DateTime<Utc>,
    ctx: &MarketContext,
    start_market_state: Option<&MarketStateModel>,
    _end_market_state: Option<&MarketStateModel>,
    gm_qty: Decimal,
    gm_entry_price: Decimal,
    gm_exit_price: Decimal,
    gm_pnl: Decimal,
    hedge_result: &HedgeResult,
    token_price_series: &HashMap<i32, Vec<TokenPriceModel>>,
    notes: &mut Vec<String>,
    warnings: &mut Vec<String>,
) -> MarketDiagnostics {
    let mut diagnostics = MarketDiagnostics {
        gm_return_pct: calculate_return_pct(gm_entry_price, gm_exit_price),
        perp_return_pct: calculate_return_pct(hedge_result.perp_entry_price, hedge_result.perp_exit_price),
        ..Default::default()
    };

    let long_token_series = token_price_series.get(&ctx.long_token_id);
    let short_token_series = token_price_series.get(&ctx.short_token_id);

    let long_token_start = long_token_series
        .and_then(|rows| latest_at_or_before_token(rows, run.timestamp));
    let long_token_end = long_token_series
        .and_then(|rows| latest_at_or_before_token(rows, window_end));
    let short_token_start = short_token_series
        .and_then(|rows| latest_at_or_before_token(rows, run.timestamp));
    let short_token_end = short_token_series
        .and_then(|rows| latest_at_or_before_token(rows, window_end));

    if let (Some(start), Some(end)) = (long_token_start, long_token_end) {
        diagnostics.long_token_entry_price_usd = start.mid_price;
        diagnostics.long_token_exit_price_usd = end.mid_price;
        diagnostics.long_token_return_pct = calculate_return_pct(start.mid_price, end.mid_price);
    } else if !ctx.long_is_stable {
        notes.push("Missing long token spot diagnostics".to_string());
        warnings.push(format!(
            "run_id={} market_id={} missing long token spot diagnostics in window {} -> {}",
            run.id,
            target.market_id,
            run.timestamp.to_rfc3339(),
            window_end.to_rfc3339()
        ));
    }

    if let (Some(start), Some(end)) = (short_token_start, short_token_end) {
        diagnostics.short_token_entry_price_usd = start.mid_price;
        diagnostics.short_token_exit_price_usd = end.mid_price;
        diagnostics.short_token_return_pct = calculate_return_pct(start.mid_price, end.mid_price);
    } else if !ctx.short_is_stable {
        notes.push("Missing short token spot diagnostics".to_string());
        warnings.push(format!(
            "run_id={} market_id={} missing short token spot diagnostics in window {} -> {}",
            run.id,
            target.market_id,
            run.timestamp.to_rfc3339(),
            window_end.to_rfc3339()
        ));
    }

    let Some(start_state) = start_market_state else {
        return diagnostics;
    };

    if gm_qty <= Decimal::ZERO || gm_entry_price <= Decimal::ZERO {
        return diagnostics;
    }

    let entry_pool_value = start_state.pool_long_token_usd.unwrap_or(Decimal::ZERO)
        + start_state.pool_short_token_usd.unwrap_or(Decimal::ZERO)
        - start_state.pool_impact_token_usd.unwrap_or(Decimal::ZERO);
    if entry_pool_value <= Decimal::ZERO {
        notes.push("Entry pool value unavailable for token-move diagnostics".to_string());
        return diagnostics;
    }

    let entry_gm_value = gm_qty * gm_entry_price;
    if entry_gm_value <= Decimal::ZERO {
        return diagnostics;
    }

    diagnostics.entry_pool_value_usd = entry_pool_value;
    diagnostics.entry_pool_share = entry_gm_value / entry_pool_value;
    diagnostics.entry_long_token_amount_est =
        diagnostics.entry_pool_share * start_state.pool_long_amount.unwrap_or(Decimal::ZERO);
    diagnostics.entry_short_token_amount_est =
        diagnostics.entry_pool_share * start_state.pool_short_amount.unwrap_or(Decimal::ZERO);
    diagnostics.entry_long_token_exposure_usd_est =
        diagnostics.entry_pool_share * start_state.pool_long_token_usd.unwrap_or(Decimal::ZERO);
    diagnostics.entry_short_token_exposure_usd_est =
        diagnostics.entry_pool_share * start_state.pool_short_token_usd.unwrap_or(Decimal::ZERO);

    if diagnostics.long_token_entry_price_usd > Decimal::ZERO
        && diagnostics.long_token_exit_price_usd > Decimal::ZERO
    {
        diagnostics.est_long_token_price_move_pnl_usd = diagnostics.entry_long_token_amount_est
            * (diagnostics.long_token_exit_price_usd - diagnostics.long_token_entry_price_usd);
    }

    if diagnostics.short_token_entry_price_usd > Decimal::ZERO
        && diagnostics.short_token_exit_price_usd > Decimal::ZERO
    {
        diagnostics.est_short_token_price_move_pnl_usd = diagnostics.entry_short_token_amount_est
            * (diagnostics.short_token_exit_price_usd - diagnostics.short_token_entry_price_usd);
    }

    diagnostics.est_total_collateral_price_move_pnl_usd =
        diagnostics.est_long_token_price_move_pnl_usd + diagnostics.est_short_token_price_move_pnl_usd;
    diagnostics.long_token_price_move_plus_hedge_mtm_pnl_usd =
        diagnostics.est_long_token_price_move_pnl_usd + hedge_result.hedge_mtm_pnl_usd;
    diagnostics.gm_pnl_minus_est_collateral_price_move_pnl_usd =
        gm_pnl - diagnostics.est_total_collateral_price_move_pnl_usd;

    diagnostics
}

fn latest_at_or_before_market(
    rows: &[MarketStateModel],
    ts: DateTime<Utc>,
) -> Option<&MarketStateModel> {
    latest_at_or_before_by(rows, ts, |row| row.timestamp)
}

fn latest_at_or_before_perp(
    rows: &[DydxPerpStateModel],
    ts: DateTime<Utc>,
) -> Option<&DydxPerpStateModel> {
    latest_at_or_before_by(rows, ts, |row| row.timestamp)
}

fn latest_at_or_before_token(
    rows: &[TokenPriceModel],
    ts: DateTime<Utc>,
) -> Option<&TokenPriceModel> {
    latest_at_or_before_by(rows, ts, |row| row.timestamp)
}

fn latest_at_or_before_by<T, F>(rows: &[T], ts: DateTime<Utc>, get_ts: F) -> Option<&T>
where
    F: Fn(&T) -> DateTime<Utc>,
{
    if rows.is_empty() {
        return None;
    }

    let idx = rows.partition_point(|row| get_ts(row) <= ts);
    if idx == 0 {
        None
    } else {
        Some(&rows[idx - 1])
    }
}

fn states_in_window_market(
    rows: &[MarketStateModel],
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> &[MarketStateModel] {
    slice_in_window_by(rows, start, end, |row| row.timestamp)
}

fn states_in_window_perp(
    rows: &[DydxPerpStateModel],
    start: DateTime<Utc>,
    end: DateTime<Utc>,
) -> &[DydxPerpStateModel] {
    slice_in_window_by(rows, start, end, |row| row.timestamp)
}

fn slice_in_window_by<T, F>(rows: &[T], start: DateTime<Utc>, end: DateTime<Utc>, get_ts: F) -> &[T]
where
    F: Fn(&T) -> DateTime<Utc>,
{
    if rows.is_empty() {
        return &[];
    }

    let begin = rows.partition_point(|row| get_ts(row) <= start);
    let finish = rows.partition_point(|row| get_ts(row) <= end);

    if begin >= finish {
        &[]
    } else {
        &rows[begin..finish]
    }
}

fn derive_effective_leverage(perp_state: &DydxPerpStateModel) -> Decimal {
    let initial = perp_state.initial_margin_fraction.unwrap_or(Decimal::ZERO);
    let maintenance = perp_state.maintenance_margin_fraction.unwrap_or(Decimal::ZERO);
    let margin_frac = initial.max(maintenance);

    if margin_frac <= Decimal::ZERO {
        return Decimal::ZERO;
    }

    let denom = Decimal::from_i32(2).unwrap() * margin_frac;
    if denom <= Decimal::ZERO {
        Decimal::ZERO
    } else {
        Decimal::ONE / denom
    }
}

fn calculate_return_pct(entry_price: Decimal, exit_price: Decimal) -> Decimal {
    if entry_price <= Decimal::ZERO {
        Decimal::ZERO
    } else {
        (exit_price / entry_price - Decimal::ONE) * Decimal::from_i32(100).unwrap()
    }
}

fn calculate_funding_pnl(
    hedge_qty: Decimal,
    start_state: &DydxPerpStateModel,
    end_state: &DydxPerpStateModel,
    in_window_rows: &[DydxPerpStateModel],
) -> Decimal {
    if hedge_qty.is_zero() {
        return Decimal::ZERO;
    }

    let mut funding = Decimal::ZERO;
    let mut prev = start_state;

    for row in in_window_rows {
        let dt_hours = hours_between(prev.timestamp, row.timestamp);
        if dt_hours > Decimal::ZERO {
            let rate = prev.funding_rate.unwrap_or(Decimal::ZERO);
            let price = prev.oracle_price.unwrap_or(Decimal::ZERO);
            funding += -hedge_qty * price * rate * dt_hours;
        }
        prev = row;
    }

    let tail_hours = hours_between(prev.timestamp, end_state.timestamp);
    if tail_hours > Decimal::ZERO {
        let rate = prev.funding_rate.unwrap_or(Decimal::ZERO);
        let price = prev.oracle_price.unwrap_or(Decimal::ZERO);
        funding += -hedge_qty * price * rate * tail_hours;
    }

    funding
}

fn calculate_fee_attribution(
    gm_qty: Decimal,
    market_rows_in_window: &[MarketStateModel],
) -> Decimal {
    if gm_qty <= Decimal::ZERO {
        return Decimal::ZERO;
    }

    let mut fee_attr = Decimal::ZERO;

    for row in market_rows_in_window {
        let gm_price = row.gm_price_mid.unwrap_or(Decimal::ZERO);
        if gm_price <= Decimal::ZERO {
            continue;
        }

        let pool_value = row.pool_long_token_usd.unwrap_or(Decimal::ZERO)
            + row.pool_short_token_usd.unwrap_or(Decimal::ZERO)
            - row.pool_impact_token_usd.unwrap_or(Decimal::ZERO);

        if pool_value <= Decimal::ZERO {
            continue;
        }

        let our_gm_value = gm_qty * gm_price;
        if our_gm_value <= Decimal::ZERO {
            continue;
        }

        let share = our_gm_value / pool_value;
        let fees_total = row.fees_total.unwrap_or(Decimal::ZERO);
        fee_attr += share * fees_total;
    }

    fee_attr
}

fn hours_between(start: DateTime<Utc>, end: DateTime<Utc>) -> Decimal {
    if end <= start {
        return Decimal::ZERO;
    }

    let seconds = (end - start).num_seconds();
    Decimal::from_i64(seconds).unwrap_or(Decimal::ZERO)
        / Decimal::from_i64(3600).unwrap()
}

fn write_outputs(
    output_dir: &Path,
    window_results: &[WindowResult],
    market_results: &[MarketWindowResult],
    warnings: &[String],
    args: &CliArgs,
    runs: &[StrategyRunModel],
    nav_points: &[(DateTime<Utc>, Decimal)],
) -> Result<BacktestArtifacts> {
    let windows_csv_path = output_dir.join("windows.csv");
    let market_windows_csv_path = output_dir.join("market_windows.csv");
    let summary_json_path = output_dir.join("summary.json");

    write_windows_csv(&windows_csv_path, window_results)?;
    write_market_windows_csv(&market_windows_csv_path, market_results)?;

    let summary = build_summary(
        args,
        runs,
        window_results,
        warnings,
        nav_points,
        &summary_json_path,
        &windows_csv_path,
        &market_windows_csv_path,
    )?;

    let summary_json = serde_json::to_string_pretty(&summary)
        .context("Failed to serialize summary JSON")?;
    fs::write(&summary_json_path, summary_json)
        .with_context(|| format!("Failed to write {}", summary_json_path.display()))?;

    Ok(BacktestArtifacts {
        summary_json: summary_json_path,
        windows_csv: windows_csv_path,
        market_windows_csv: market_windows_csv_path,
    })
}

fn write_windows_csv(path: &Path, rows: &[WindowResult]) -> Result<()> {
    let mut out = String::new();
    out.push_str(
        "run_id,strategy_version,window_start_utc,window_end_utc,start_nav_usd,end_nav_usd,window_return_pct,gm_pnl_usd,hedge_mtm_pnl_usd,funding_pnl_usd,fee_attribution_usd,target_count,active_market_count\n",
    );

    for row in rows {
        let cols = vec![
            row.run_id.to_string(),
            row.strategy_version.clone(),
            row.window_start.to_rfc3339(),
            row.window_end.to_rfc3339(),
            row.start_nav_usd.to_string(),
            row.end_nav_usd.to_string(),
            row.window_return_pct.to_string(),
            row.gm_pnl_usd.to_string(),
            row.hedge_mtm_pnl_usd.to_string(),
            row.funding_pnl_usd.to_string(),
            row.fee_attribution_usd.to_string(),
            row.target_count.to_string(),
            row.active_market_count.to_string(),
        ];
        out.push_str(&csv_line(&cols));
        out.push('\n');
    }

    fs::write(path, out).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

fn write_market_windows_csv(path: &Path, rows: &[MarketWindowResult]) -> Result<()> {
    let mut out = String::new();
    out.push_str(
        "run_id,strategy_version,window_start_utc,window_end_utc,market_id,market_address,market_display_name,long_token_symbol,short_token_symbol,target_weight_raw,target_weight_normalized,package_capital_usd,hedge_exposure_fraction,desired_hedge_notional_usd,actual_hedge_notional_usd,required_hedge_margin_usd,gm_capital_usd,gm_entry_price,gm_exit_price,gm_token_quantity,gm_pnl_usd,perp_ticker,perp_entry_price,perp_exit_price,hedge_quantity,hedge_mtm_pnl_usd,funding_pnl_usd,fee_attribution_usd,long_token_entry_price_usd,long_token_exit_price_usd,long_token_return_pct,short_token_entry_price_usd,short_token_exit_price_usd,short_token_return_pct,gm_return_pct,perp_return_pct,entry_pool_value_usd,entry_pool_share,entry_long_token_amount_est,entry_short_token_amount_est,entry_long_token_exposure_usd_est,entry_short_token_exposure_usd_est,est_long_token_price_move_pnl_usd,est_short_token_price_move_pnl_usd,est_total_collateral_price_move_pnl_usd,long_token_price_move_plus_hedge_mtm_pnl_usd,gm_pnl_minus_est_collateral_price_move_pnl_usd,notes\n",
    );

    for row in rows {
        let cols = vec![
            row.run_id.to_string(),
            row.strategy_version.clone(),
            row.window_start.to_rfc3339(),
            row.window_end.to_rfc3339(),
            row.market_id.to_string(),
            row.market_address.clone(),
            row.market_display_name.clone(),
            row.long_token_symbol.clone(),
            row.short_token_symbol.clone(),
            row.target_weight_raw.to_string(),
            row.target_weight_normalized.to_string(),
            row.package_capital_usd.to_string(),
            row.hedge_exposure_fraction.to_string(),
            row.desired_hedge_notional_usd.to_string(),
            row.actual_hedge_notional_usd.to_string(),
            row.required_hedge_margin_usd.to_string(),
            row.gm_capital_usd.to_string(),
            row.gm_entry_price.to_string(),
            row.gm_exit_price.to_string(),
            row.gm_token_quantity.to_string(),
            row.gm_pnl_usd.to_string(),
            row.perp_ticker.clone().unwrap_or_default(),
            row.perp_entry_price.to_string(),
            row.perp_exit_price.to_string(),
            row.hedge_quantity.to_string(),
            row.hedge_mtm_pnl_usd.to_string(),
            row.funding_pnl_usd.to_string(),
            row.fee_attribution_usd.to_string(),
            row.long_token_entry_price_usd.to_string(),
            row.long_token_exit_price_usd.to_string(),
            row.long_token_return_pct.to_string(),
            row.short_token_entry_price_usd.to_string(),
            row.short_token_exit_price_usd.to_string(),
            row.short_token_return_pct.to_string(),
            row.gm_return_pct.to_string(),
            row.perp_return_pct.to_string(),
            row.entry_pool_value_usd.to_string(),
            row.entry_pool_share.to_string(),
            row.entry_long_token_amount_est.to_string(),
            row.entry_short_token_amount_est.to_string(),
            row.entry_long_token_exposure_usd_est.to_string(),
            row.entry_short_token_exposure_usd_est.to_string(),
            row.est_long_token_price_move_pnl_usd.to_string(),
            row.est_short_token_price_move_pnl_usd.to_string(),
            row.est_total_collateral_price_move_pnl_usd.to_string(),
            row.long_token_price_move_plus_hedge_mtm_pnl_usd.to_string(),
            row.gm_pnl_minus_est_collateral_price_move_pnl_usd.to_string(),
            row.notes.clone(),
        ];
        out.push_str(&csv_line(&cols));
        out.push('\n');
    }

    fs::write(path, out).with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

fn csv_line(cols: &[String]) -> String {
    cols.iter()
        .map(|v| csv_escape(v))
        .collect::<Vec<_>>()
        .join(",")
}

fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') {
        let escaped = value.replace('"', "\"\"");
        format!("\"{}\"", escaped)
    } else {
        value.to_string()
    }
}

fn build_summary(
    args: &CliArgs,
    runs: &[StrategyRunModel],
    windows: &[WindowResult],
    warnings: &[String],
    nav_points: &[(DateTime<Utc>, Decimal)],
    summary_json_path: &Path,
    windows_csv_path: &Path,
    market_windows_csv_path: &Path,
) -> Result<SummaryOutput> {
    let first_window_start = windows.first().map(|w| w.window_start);
    let last_window_end = windows.last().map(|w| w.window_end);

    let final_nav = windows
        .last()
        .map(|w| w.end_nav_usd)
        .unwrap_or(args.initial_capital);

    let total_pnl = final_nav - args.initial_capital;

    let total_return_pct = if args.initial_capital > Decimal::ZERO {
        (final_nav / args.initial_capital - Decimal::ONE) * Decimal::from_i32(100).unwrap()
    } else {
        Decimal::ZERO
    };

    let gm_total: Decimal = windows.iter().map(|w| w.gm_pnl_usd).sum();
    let hedge_mtm_total: Decimal = windows.iter().map(|w| w.hedge_mtm_pnl_usd).sum();
    let funding_total: Decimal = windows.iter().map(|w| w.funding_pnl_usd).sum();
    let fee_total: Decimal = windows.iter().map(|w| w.fee_attribution_usd).sum();

    let max_drawdown_pct = compute_max_drawdown_pct(nav_points);
    let (annualized_return_pct, annualized_volatility_pct, sharpe) =
        compute_annualized_metrics(args.initial_capital, windows);

    let config = SummaryConfig {
        strategy_version: args.strategy_version.clone(),
        start: args.start.map(|s| s.to_rfc3339()),
        end: args.end.map(|e| e.to_rfc3339()),
        initial_capital_usd: args.initial_capital.to_string(),
        fee_mode: "attribution_only".to_string(),
        hedge_exposure_rule: "0.5_if_short_collateral_is_stable_else_1.0".to_string(),
        hedge_leverage_rule: "max_leverage = 1 / (2 * max(initial_margin_fraction, maintenance_margin_fraction))".to_string(),
        funding_rate_unit: "hourly".to_string(),
        price_selection_rule: "latest_state_at_or_before_timestamp (UTC)".to_string(),
    };

    let metrics = SummaryMetrics {
        window_count: windows.len(),
        first_window_start: first_window_start.map(|v| v.to_rfc3339()),
        last_window_end: last_window_end.map(|v| v.to_rfc3339()),
        initial_nav_usd: args.initial_capital.to_string(),
        final_nav_usd: final_nav.to_string(),
        total_pnl_usd: total_pnl.to_string(),
        total_return_pct: total_return_pct.to_string(),
        annualized_return_pct: annualized_return_pct.to_string(),
        annualized_volatility_pct: annualized_volatility_pct.to_string(),
        sharpe: sharpe.to_string(),
        max_drawdown_pct: max_drawdown_pct.to_string(),
        gm_pnl_usd: gm_total.to_string(),
        hedge_mtm_pnl_usd: hedge_mtm_total.to_string(),
        funding_pnl_usd: funding_total.to_string(),
        fee_attribution_usd: fee_total.to_string(),
    };

    let mut output_files = HashMap::new();
    output_files.insert(
        "summary_json".to_string(),
        summary_json_path.to_string_lossy().to_string(),
    );
    output_files.insert(
        "windows_csv".to_string(),
        windows_csv_path.to_string_lossy().to_string(),
    );
    output_files.insert(
        "market_windows_csv".to_string(),
        market_windows_csv_path.to_string_lossy().to_string(),
    );

    let mut summary_warnings = warnings.to_vec();
    if windows.len() + 1 < runs.len() {
        summary_warnings.push(
            "Some strategy runs were not simulated because they did not form a valid window"
                .to_string(),
        );
    }

    Ok(SummaryOutput {
        generated_at_utc: Utc::now().to_rfc3339(),
        config,
        metrics,
        warning_count: summary_warnings.len(),
        warnings: summary_warnings,
        output_files,
    })
}

fn compute_max_drawdown_pct(nav_points: &[(DateTime<Utc>, Decimal)]) -> Decimal {
    if nav_points.is_empty() {
        return Decimal::ZERO;
    }

    let mut peak = nav_points[0].1;
    let mut max_dd = Decimal::ZERO;

    for (_, nav) in nav_points {
        if *nav > peak {
            peak = *nav;
        }
        if peak > Decimal::ZERO {
            let dd = (peak - *nav) / peak;
            if dd > max_dd {
                max_dd = dd;
            }
        }
    }

    max_dd * Decimal::from_i32(100).unwrap()
}

fn compute_annualized_metrics(
    initial_capital: Decimal,
    windows: &[WindowResult],
) -> (Decimal, Decimal, Decimal) {
    if windows.is_empty() || initial_capital <= Decimal::ZERO {
        return (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO);
    }

    let first = windows.first().unwrap();
    let last = windows.last().unwrap();

    let elapsed_hours = hours_between(first.window_start, last.window_end);
    if elapsed_hours <= Decimal::ZERO {
        return (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO);
    }

    let elapsed_years_f = elapsed_hours.to_f64().unwrap_or(0.0) / (24.0 * 365.0);
    if elapsed_years_f <= 0.0 {
        return (Decimal::ZERO, Decimal::ZERO, Decimal::ZERO);
    }

    let final_nav = last.end_nav_usd;
    let growth_f = if initial_capital > Decimal::ZERO {
        (final_nav / initial_capital).to_f64().unwrap_or(0.0)
    } else {
        0.0
    };

    let annualized_return_f = if growth_f > 0.0 {
        growth_f.powf(1.0 / elapsed_years_f) - 1.0
    } else {
        -1.0
    };

    let window_returns: Vec<f64> = windows
        .iter()
        .map(|w| (w.window_return_pct / Decimal::from_i32(100).unwrap()).to_f64().unwrap_or(0.0))
        .collect();

    let mean_window = if window_returns.is_empty() {
        0.0
    } else {
        window_returns.iter().sum::<f64>() / window_returns.len() as f64
    };

    let var_window = if window_returns.len() < 2 {
        0.0
    } else {
        let denom = (window_returns.len() - 1) as f64;
        window_returns
            .iter()
            .map(|r| (r - mean_window).powi(2))
            .sum::<f64>()
            / denom
    };

    let std_window = var_window.sqrt();

    let avg_window_hours = windows
        .iter()
        .map(|w| hours_between(w.window_start, w.window_end).to_f64().unwrap_or(0.0))
        .sum::<f64>()
        / windows.len() as f64;

    let periods_per_year = if avg_window_hours > 0.0 {
        (24.0 * 365.0) / avg_window_hours
    } else {
        0.0
    };

    let annualized_vol_f = std_window * periods_per_year.sqrt();
    let sharpe_f = if annualized_vol_f > 0.0 {
        annualized_return_f / annualized_vol_f
    } else {
        0.0
    };

    (
        Decimal::from_f64(annualized_return_f * 100.0).unwrap_or(Decimal::ZERO),
        Decimal::from_f64(annualized_vol_f * 100.0).unwrap_or(Decimal::ZERO),
        Decimal::from_f64(sharpe_f).unwrap_or(Decimal::ZERO),
    )
}

fn log_summary(windows: &[WindowResult], warnings: &[String], artifacts: &BacktestArtifacts, args: &CliArgs) {
    let first = windows.first().unwrap();
    let last = windows.last().unwrap();

    let total_pnl = last.end_nav_usd - args.initial_capital;
    let total_return_pct = if args.initial_capital > Decimal::ZERO {
        (last.end_nav_usd / args.initial_capital - Decimal::ONE) * Decimal::from_i32(100).unwrap()
    } else {
        Decimal::ZERO
    };

    let gm_total: Decimal = windows.iter().map(|w| w.gm_pnl_usd).sum();
    let hedge_mtm_total: Decimal = windows.iter().map(|w| w.hedge_mtm_pnl_usd).sum();
    let funding_total: Decimal = windows.iter().map(|w| w.funding_pnl_usd).sum();
    let fee_total: Decimal = windows.iter().map(|w| w.fee_attribution_usd).sum();

    info!("Backtest complete");
    info!(
        strategy_version = %args.strategy_version,
        windows_simulated = windows.len(),
        start_utc = %first.window_start.to_rfc3339(),
        end_utc = %last.window_end.to_rfc3339(),
        initial_nav_usd = %args.initial_capital,
        final_nav_usd = %last.end_nav_usd,
        total_pnl_usd = %total_pnl,
        total_return_pct = %total_return_pct,
        gm_pnl_usd = %gm_total,
        hedge_mtm_pnl_usd = %hedge_mtm_total,
        funding_pnl_usd = %funding_total,
        fee_attribution_usd = %fee_total,
        warning_count = warnings.len(),
        summary_json = %artifacts.summary_json.display(),
        windows_csv = %artifacts.windows_csv.display(),
        market_windows_csv = %artifacts.market_windows_csv.display(),
        "Backtest summary"
    );
    if !warnings.is_empty() {
        warn!(warning_count = warnings.len(), "Backtest completed with warnings");
    }
}
