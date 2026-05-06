use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::Utc;
use ethers::types::Address;
use eyre::Result;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::constants::{NATIVE_ADDRESS, WNT_ADDRESS};
use crate::db::db_manager::DbManager;
use crate::db::models::portfolio_snapshots::NewPortfolioSnapshotModel;
use crate::db::models::position_snapshots::NewPositionSnapshotModel;
use crate::db::models::strategy_runs::NewStrategyRunModel;
use crate::db::models::strategy_targets::NewStrategyTargetModel;
use crate::db::models::trades::NewTradeModel;
use crate::gm_token_txs::gm_tx_manager::GmTxManager;
use crate::gm_token_txs::types::{
    GmAmountOutResponse, GmDepositRequest, GmShiftRequest, GmTxRequest, GmWithdrawalRequest,
};
use crate::hedging::dydx_client::DydxClient;
use crate::hedging::hedge_utils;
use crate::spot_swap::paraswap_api_client::ParaSwapClient;
use crate::spot_swap::swap_manager::SwapManager;
use crate::spot_swap::types::{QuoteRequest, SwapRequest};
use crate::strategy::types::PortfolioData;
use crate::wallet::{TokenInfo, WalletManager};

use super::planner::{
    compute_market_deltas, compute_target_values, compute_target_weights, plan_shift_actions,
};
use super::types::{
    ExecutionTargets, PlannerConfig, PortfolioSnapshot, ReserveState, TradeAction, TradeStatus,
};

pub struct ExecutionEngine {
    config: Arc<Config>,
    db_manager: Arc<DbManager>,
    wallet_manager: Arc<WalletManager>,
    gm_tx_manager: GmTxManager,
    swap_manager: SwapManager,
    paraswap_client: ParaSwapClient,
    dydx_client: Arc<tokio::sync::Mutex<DydxClient>>,
    planner_config: PlannerConfig,
}

struct ActionExecutionResult {
    status: TradeStatus,
    tx_hash: Option<String>,
}

impl ExecutionEngine {
    pub fn new(
        config: Arc<Config>,
        db_manager: Arc<DbManager>,
        wallet_manager: Arc<WalletManager>,
        dydx_client: Arc<tokio::sync::Mutex<DydxClient>>,
    ) -> Self {
        let gm_tx_manager =
            GmTxManager::new(config.clone(), wallet_manager.clone(), db_manager.clone());
        let swap_manager = SwapManager::new(&config, wallet_manager.clone());
        let paraswap_client = ParaSwapClient::new(wallet_manager.address, &config);
        Self {
            config,
            db_manager,
            wallet_manager,
            gm_tx_manager,
            swap_manager,
            paraswap_client,
            dydx_client,
            planner_config: PlannerConfig::default(),
        }
    }

    pub async fn run_once(&self, portfolio_data: &PortfolioData) -> Result<()> {
        let strategy_run_id = self.record_strategy_run(portfolio_data).await?;
        let execution_targets = Self::execution_targets_from_portfolio_data(portfolio_data);
        self.run_once_with_existing_strategy_run(&execution_targets, strategy_run_id)
            .await
    }

    pub async fn run_once_with_existing_strategy_run(
        &self,
        execution_targets: &ExecutionTargets,
        strategy_run_id: i32,
    ) -> Result<()> {
        let target_weights = compute_target_weights(execution_targets);
        info!(
            strategy_run_id,
            target_market_count = target_weights.len(),
            target_weight_sum = %target_weights.values().cloned().sum::<Decimal>(),
            target_weights = ?self.summarize_target_weights(&target_weights),
            "Starting execution cycle"
        );
        let mut snapshot = self.build_snapshot().await?;
        info!(
            strategy_run_id,
            snapshot = %self.snapshot_summary(&snapshot),
            "Loaded live portfolio snapshot"
        );
        let token_hedgeinfo_map = self.fetch_token_hedgeinfo_map().await;
        debug!(
            strategy_run_id,
            hedgeable_token_count = token_hedgeinfo_map.len(),
            "Fetched dYdX hedge metadata"
        );
        let reserve_state = self
            .compute_reserve_state(&target_weights, &snapshot, &token_hedgeinfo_map)
            .await?;
        info!(
            strategy_run_id,
            investable_capital = %reserve_state.investable_capital,
            reserve_total = %reserve_state.reserve_total,
            required_margin = %reserve_state.required_margin,
            required_equity = %reserve_state.required_equity,
            required_free_collateral = %reserve_state.required_free_collateral,
            gas_reserve_target_usd = %reserve_state.gas_reserve_target_usd,
            gas_reserve_target_eth = %reserve_state.gas_reserve_target_eth,
            "Computed reserve state"
        );

        if reserve_state.investable_capital <= Decimal::ZERO {
            warn!(
                total_value_usd = %snapshot.total_value_usd,
                reserve_total = %reserve_state.reserve_total,
                strategy_run_id,
                "No investable capital after reserves; skipping"
            );
            self.record_snapshot(Some(strategy_run_id), &snapshot).await?;
            return Ok(());
        }

        if self
            .maybe_withdraw_dydx_excess(&snapshot, &reserve_state)
            .await?
        {
            snapshot = self.build_snapshot().await?;
            info!(
                strategy_run_id,
                snapshot = %self.snapshot_summary(&snapshot),
                "Refreshed snapshot after dYdX excess withdrawal"
            );
        }

        if self
            .ensure_gas_reserve(&snapshot, &reserve_state, Some(strategy_run_id))
            .await?
        {
            snapshot = self.build_snapshot().await?;
            info!(
                strategy_run_id,
                snapshot = %self.snapshot_summary(&snapshot),
                "Refreshed snapshot after gas reserve adjustment"
            );
        }

        let target_values =
            compute_target_values(&target_weights, reserve_state.investable_capital);
        debug!(
            strategy_run_id,
            target_values = ?self.summarize_market_values(&target_values),
            "Computed target market values"
        );

        let mut deltas = compute_market_deltas(
            &snapshot,
            &target_values,
            &self.wallet_manager,
            &self.planner_config,
        );
        info!(
            strategy_run_id,
            deltas = ?self.summarize_market_deltas(&deltas),
            "Computed initial market deltas"
        );

        // Stage 1: Shifts
        let (shift_actions, _) = plan_shift_actions(&deltas, &self.wallet_manager);
        info!(
            strategy_run_id,
            action_count = shift_actions.len(),
            actions = ?self.describe_actions(&shift_actions),
            "Stage 1: planned GM shifts"
        );
        if self.execute_actions(&shift_actions, Some(strategy_run_id)).await {
            snapshot = self.build_snapshot().await?;
            deltas = compute_market_deltas(
                &snapshot,
                &target_values,
                &self.wallet_manager,
                &self.planner_config,
            );
            info!(
                strategy_run_id,
                snapshot = %self.snapshot_summary(&snapshot),
                deltas = ?self.summarize_market_deltas(&deltas),
                "Completed shift stage and recomputed deltas"
            );
        }

        // Stage 2: Withdrawals
        let withdraw_actions = self.build_withdraw_actions(&snapshot, &deltas);
        info!(
            strategy_run_id,
            action_count = withdraw_actions.len(),
            actions = ?self.describe_actions(&withdraw_actions),
            "Stage 2: planned GM withdrawals"
        );
        let withdrew = self
            .execute_actions(&withdraw_actions, Some(strategy_run_id))
            .await;
        let gas_rebalanced = if withdrew {
            snapshot = self.build_snapshot().await?;
            self.ensure_gas_reserve(&snapshot, &reserve_state, Some(strategy_run_id))
                .await?
        } else {
            self.ensure_gas_reserve(&snapshot, &reserve_state, Some(strategy_run_id))
                .await?
        };
        if withdrew || gas_rebalanced {
            snapshot = self.build_snapshot().await?;
            deltas = compute_market_deltas(
                &snapshot,
                &target_values,
                &self.wallet_manager,
                &self.planner_config,
            );
            info!(
                strategy_run_id,
                withdrew,
                gas_rebalanced,
                snapshot = %self.snapshot_summary(&snapshot),
                deltas = ?self.summarize_market_deltas(&deltas),
                "Completed withdrawal stage and recomputed deltas"
            );
        }

        // Stage 3: Deposits + cleanup swaps
        info!(strategy_run_id, "Stage 3: executing deposit and asset cleanup stage");
        if self
            .execute_deposit_stage(&snapshot, &deltas, &reserve_state, Some(strategy_run_id))
            .await?
        {
            snapshot = self.build_snapshot().await?;
            info!(
                strategy_run_id,
                snapshot = %self.snapshot_summary(&snapshot),
                "Completed deposit stage and refreshed snapshot"
            );
        }

        // Stage 4: dYdX reserve management
        info!(strategy_run_id, "Stage 4: managing dYdX reserves");
        let reserves_changed = self.manage_dydx_reserves(&snapshot, &reserve_state).await?;

        // Stage 5: Hedge adjustments
        let hedge_actions = self.build_hedge_actions(&snapshot, &target_weights).await?;
        info!(
            strategy_run_id,
            action_count = hedge_actions.len(),
            actions = ?self.describe_actions(&hedge_actions),
            "Stage 5: planned hedge adjustments"
        );
        let hedges_changed = self.execute_actions(&hedge_actions, Some(strategy_run_id)).await;
        if !hedge_actions.is_empty() {
            info!(strategy_run_id, "Waiting for dYdX hedge polling tasks to converge");
            let mut client = self.dydx_client.lock().await;
            client.wait_for_active_perp_tasks().await;
        }

        let post_snapshot = if reserves_changed || hedges_changed {
            self.build_snapshot().await?
        } else {
            snapshot
        };
        info!(
            strategy_run_id,
            reserves_changed,
            hedges_changed,
            snapshot = %self.snapshot_summary(&post_snapshot),
            "Execution cycle completed; recording final snapshot"
        );
        self.record_snapshot(Some(strategy_run_id), &post_snapshot)
            .await?;

        Ok(())
    }

    pub async fn run_unwind_to_stable(&self, target_stable_symbol: &str) -> Result<()> {
        let mut snapshot = self.build_snapshot().await?;
        let target_stable = self
            .resolve_target_stable_token(target_stable_symbol, &snapshot.asset_balances)
            .ok_or_else(|| {
                eyre::eyre!(
                    "No target stable token available for {}",
                    target_stable_symbol
                )
            })?;

        info!(
            target_stable_symbol = %target_stable.symbol,
            snapshot = %self.snapshot_summary(&snapshot),
            "Starting unwind to stable"
        );

        let mut changed = false;

        let withdraw_actions = self.build_full_unwind_withdraw_actions(&snapshot);
        info!(
            target_stable_symbol = %target_stable.symbol,
            action_count = withdraw_actions.len(),
            actions = ?self.describe_actions(&withdraw_actions),
            "Unwind stage 1: planned GM withdrawals"
        );
        if self.execute_actions(&withdraw_actions, None).await {
            changed = true;
            snapshot = self.build_snapshot().await?;
            info!(
                target_stable_symbol = %target_stable.symbol,
                snapshot = %self.snapshot_summary(&snapshot),
                "Completed unwind withdrawal stage"
            );
        }

        let hedge_actions = self.build_full_unwind_hedge_actions(&snapshot);
        info!(
            target_stable_symbol = %target_stable.symbol,
            action_count = hedge_actions.len(),
            actions = ?self.describe_actions(&hedge_actions),
            "Unwind stage 2: planned hedge closures"
        );
        if self.execute_actions(&hedge_actions, None).await {
            changed = true;
        }
        if !hedge_actions.is_empty() {
            info!(
                target_stable_symbol = %target_stable.symbol,
                "Waiting for dYdX hedge polling tasks to converge"
            );
            let mut client = self.dydx_client.lock().await;
            client.wait_for_active_perp_tasks().await;
        }
        if !hedge_actions.is_empty() {
            snapshot = self.build_snapshot().await?;
            info!(
                target_stable_symbol = %target_stable.symbol,
                snapshot = %self.snapshot_summary(&snapshot),
                "Completed unwind hedge stage"
            );
        }

        info!(
            target_stable_symbol = %target_stable.symbol,
            min_value_usd = %self.planner_config.unwind_min_value_usd,
            "Unwind stage 3: cleaning ERC-20 balances into target stable"
        );
        if self
            .cleanup_asset_tokens_to_stable(
                &snapshot.asset_balances,
                &HashSet::new(),
                &target_stable,
                None,
                true,
                self.planner_config.unwind_min_value_usd,
            )
            .await
        {
            changed = true;
            snapshot = self.build_snapshot().await?;
            info!(
                target_stable_symbol = %target_stable.symbol,
                snapshot = %self.snapshot_summary(&snapshot),
                "Completed unwind asset cleanup stage"
            );
        }

        info!(
            target_stable_symbol = %target_stable.symbol,
            native_buffer_usd = %self.planner_config.stable_only_native_buffer_usd,
            "Unwind stage 4: selling excess native ETH into target stable"
        );
        if self
            .sell_excess_native_to_stable(&snapshot, &target_stable, None)
            .await
        {
            changed = true;
            snapshot = self.build_snapshot().await?;
            info!(
                target_stable_symbol = %target_stable.symbol,
                snapshot = %self.snapshot_summary(&snapshot),
                "Completed unwind native cleanup stage"
            );
        }

        self.record_snapshot(None, &snapshot).await?;

        info!(
            target_stable_symbol = %target_stable.symbol,
            changed,
            "Completed unwind to stable pass"
        );

        Ok(())
    }

    async fn sell_excess_native_to_stable(
        &self,
        snapshot: &PortfolioSnapshot,
        target_stable: &TokenInfo,
        strategy_run_id: Option<i32>,
    ) -> bool {
        let native_price = self.wallet_manager.native_token.last_mid_price_usd;
        if native_price <= Decimal::ZERO {
            debug!("Skipping native unwind because native price is unavailable");
            return false;
        }

        // Keep a small explicit ETH buffer for future transactions during a risk-off unwind.
        let native_buffer = self.planner_config.stable_only_native_buffer_usd / native_price;
        let sell_amount = (snapshot.native_balance - native_buffer).max(Decimal::ZERO);
        debug!(
            current_native_balance = %snapshot.native_balance,
            native_price_usd = %native_price,
            native_buffer_eth = %native_buffer,
            stable_only_native_buffer_usd = %self.planner_config.stable_only_native_buffer_usd,
            sell_amount = %sell_amount,
            "Evaluated native unwind buffer"
        );
        if sell_amount <= Decimal::ZERO {
            return false;
        }

        let action = TradeAction::SpotSwap {
            from_token: NATIVE_ADDRESS.parse().unwrap(),
            to_token: target_stable.address,
            amount: sell_amount,
            side: "SELL".to_string(),
        };
        info!(
            action = %self.describe_action(&action),
            "Selling excess native ETH into target stable"
        );
        self.execute_and_log(&action, strategy_run_id).await
    }

    fn execution_targets_from_portfolio_data(portfolio_data: &PortfolioData) -> ExecutionTargets {
        ExecutionTargets::new(
            portfolio_data.market_addresses.clone(),
            portfolio_data.weights.iter().copied().collect(),
        )
    }

    async fn maybe_withdraw_dydx_excess(
        &self,
        snapshot: &PortfolioSnapshot,
        reserve_state: &ReserveState,
    ) -> Result<bool> {
        let excess_equity = snapshot.dydx_subaccount_equity - reserve_state.required_equity;
        let excess_free = snapshot.dydx_free_collateral - reserve_state.required_free_collateral;
        let excess = excess_equity.min(excess_free);
        let shortfall = reserve_state.investable_capital - snapshot.arbitrum_value_usd;
        debug!(
            dydx_subaccount_equity = %snapshot.dydx_subaccount_equity,
            dydx_free_collateral = %snapshot.dydx_free_collateral,
            required_equity = %reserve_state.required_equity,
            required_free_collateral = %reserve_state.required_free_collateral,
            excess_equity = %excess_equity,
            excess_free_collateral = %excess_free,
            arbitrum_value_usd = %snapshot.arbitrum_value_usd,
            investable_capital = %reserve_state.investable_capital,
            arbitrum_shortfall_usd = %shortfall,
            "Evaluated dYdX excess withdrawal opportunity"
        );
        if excess <= Decimal::ZERO || shortfall <= self.planner_config.min_value_usd {
            return Ok(false);
        }

        let amount = excess.min(shortfall);
        if amount <= self.planner_config.min_value_usd {
            return Ok(false);
        }

        let mut client = self.dydx_client.lock().await;
        if let Err(e) = client.withdraw_from_subaccount(amount).await {
            warn!(error = ?e, "Failed to move funds from subaccount for deployment");
            return Ok(false);
        }
        if let Err(e) = client.dydx_withdrawal(Some(amount), None, true, None).await {
            warn!(error = ?e, "Failed to withdraw dYdX excess for deployment");
            Ok(false)
        } else {
            info!(amount = %amount, "Withdrawing dYdX excess for deployment");
            Ok(true)
        }
    }

    async fn build_snapshot(&self) -> Result<PortfolioSnapshot> {
        let all_balances = self.wallet_manager.get_all_token_balances().await?;
        let native_balance = self.wallet_manager.get_native_balance().await?;
        let mut market_balances = HashMap::new();
        let mut asset_balances = HashMap::new();

        for (address, balance) in all_balances.into_iter() {
            if balance > Decimal::ZERO && self.wallet_manager.market_tokens.contains_key(&address) {
                market_balances.insert(address, balance);
            }
            if balance > Decimal::ZERO && self.wallet_manager.asset_tokens.contains_key(&address) {
                asset_balances.insert(address, balance);
            }
        }

        let mut market_values_usd = HashMap::new();
        let mut asset_values_usd = HashMap::new();
        let mut market_value_usd = Decimal::ZERO;
        let mut asset_value_usd = Decimal::ZERO;

        for (market, balance) in market_balances.iter() {
            let price = self
                .wallet_manager
                .market_tokens
                .get(market)
                .map(|token| token.last_mid_price_usd)
                .unwrap_or(Decimal::ZERO);
            let value = balance * price;
            if value > Decimal::ZERO {
                market_values_usd.insert(*market, value);
            }
            market_value_usd += value;
        }

        for (token, balance) in asset_balances.iter() {
            let price = self
                .wallet_manager
                .asset_tokens
                .get(token)
                .map(|token| token.last_mid_price_usd)
                .unwrap_or(Decimal::ZERO);
            let value = balance * price;
            if value > Decimal::ZERO {
                asset_values_usd.insert(*token, value);
            }
            asset_value_usd += value;
        }

        let (hedge_positions, dydx_main_usdc, dydx_subaccount_equity, dydx_free_collateral) =
            self.fetch_dydx_snapshot_state().await;

        let native_value_usd = native_balance * self.wallet_manager.native_token.last_mid_price_usd;
        let arbitrum_value_usd = market_value_usd + asset_value_usd + native_value_usd;
        let total_value_usd = arbitrum_value_usd + dydx_main_usdc + dydx_subaccount_equity;
        debug!(
            market_value_usd = %market_value_usd,
            asset_value_usd = %asset_value_usd,
            native_value_usd = %native_value_usd,
            arbitrum_value_usd = %arbitrum_value_usd,
            dydx_main_usdc = %dydx_main_usdc,
            dydx_subaccount_equity = %dydx_subaccount_equity,
            dydx_free_collateral = %dydx_free_collateral,
            native_balance = %native_balance,
            gm_position_count = market_balances.len(),
            asset_balance_count = asset_balances.len(),
            hedge_position_count = hedge_positions.len(),
            total_value_usd = %total_value_usd,
            "Built portfolio snapshot"
        );

        Ok(PortfolioSnapshot {
            timestamp: Utc::now(),
            market_balances,
            market_values_usd,
            asset_balances,
            asset_values_usd,
            hedge_positions,
            native_balance,
            native_value_usd,
            dydx_main_usdc,
            dydx_subaccount_equity,
            dydx_free_collateral,
            total_value_usd,
            market_value_usd,
            asset_value_usd,
            arbitrum_value_usd,
        })
    }

    async fn compute_reserve_state(
        &self,
        target_weights: &HashMap<Address, Decimal>,
        snapshot: &PortfolioSnapshot,
        token_hedgeinfo_map: &HashMap<String, Option<(Decimal, Decimal)>>,
    ) -> Result<ReserveState> {
        let total_value = snapshot.total_value_usd;
        let mut investable = Decimal::ZERO;
        let mut required_margin = Decimal::ZERO;
        let mut required_equity = Decimal::ZERO;
        let mut required_free_collateral = Decimal::ZERO;
        let mut reserve_total = Decimal::ZERO;
        let mut deposit_token_cache = HashMap::new();

        let iterations = 2;
        for iteration in 0..iterations {
            let base_reserve = total_value * self.planner_config.reserve_pct;
            investable = (total_value - base_reserve).max(Decimal::ZERO);
            let reserve_req = self
                .compute_required_dydx_reserve(
                    target_weights,
                    snapshot,
                    investable,
                    token_hedgeinfo_map,
                    &mut deposit_token_cache,
                )
                .await?;
            required_margin = reserve_req.0;
            required_equity = reserve_req.1;
            required_free_collateral = reserve_req.2;
            reserve_total = base_reserve.max(required_equity);
            investable = (total_value - reserve_total).max(Decimal::ZERO);
            debug!(
                iteration = iteration + 1,
                base_reserve = %base_reserve,
                required_margin = %required_margin,
                required_equity = %required_equity,
                required_free_collateral = %required_free_collateral,
                reserve_total = %reserve_total,
                investable_capital = %investable,
                "Reserve iteration completed"
            );
        }

        let gas_reserve_target_usd =
            snapshot.arbitrum_value_usd * self.planner_config.gas_reserve_pct;
        let native_price = self.wallet_manager.native_token.last_mid_price_usd;
        let gas_reserve_target_eth = if native_price > Decimal::ZERO {
            gas_reserve_target_usd / native_price
        } else {
            Decimal::ZERO
        };

        Ok(ReserveState {
            reserve_total,
            investable_capital: investable,
            required_margin,
            required_equity,
            required_free_collateral,
            upper_equity: required_equity * Decimal::from_f64(2.0).unwrap(),
            gas_reserve_target_usd,
            gas_reserve_target_eth,
        })
    }

    async fn compute_required_dydx_reserve(
        &self,
        target_weights: &HashMap<Address, Decimal>,
        snapshot: &PortfolioSnapshot,
        investable_capital: Decimal,
        token_hedgeinfo_map: &HashMap<String, Option<(Decimal, Decimal)>>,
        deposit_token_cache: &mut HashMap<Address, TokenInfo>,
    ) -> Result<(Decimal, Decimal, Decimal)> {
        if investable_capital <= Decimal::ZERO {
            return Ok((Decimal::ZERO, Decimal::ZERO, Decimal::ZERO));
        }

        let mut required_margin = Decimal::ZERO;
        let mut min_leverage = None::<Decimal>;
        let base_stable = self.get_preferred_stable_token(&snapshot.asset_balances);
        debug!(
            investable_capital = %investable_capital,
            base_stable = ?base_stable.as_ref().map(|token| token.symbol.clone()),
            target_market_count = target_weights.len(),
            "Computing required dYdX reserve"
        );

        for market in target_weights.keys() {
            let market_info = match self.wallet_manager.market_tokens.get(market) {
                Some(info) => info,
                None => continue,
            };
            let long_token = match self
                .wallet_manager
                .asset_tokens
                .get(&market_info.long_token_address)
            {
                Some(token) => token,
                None => continue,
            };

            if hedge_utils::STABLE_COINS.contains(&long_token.symbol.as_str()) {
                continue;
            }

            let leverage = match token_hedgeinfo_map.get(&long_token.symbol) {
                Some(Some((_, leverage))) => *leverage,
                _ => continue,
            };

            if leverage > Decimal::ZERO {
                min_leverage = Some(min_leverage.map_or(leverage, |min| min.min(leverage)));
            }

            let target_weight = target_weights.get(market).cloned().unwrap_or(Decimal::ZERO);
            let target_value = investable_capital * target_weight;
            if target_value <= Decimal::ZERO {
                continue;
            }

            let deposit_token = match self
                .select_deposit_token_with_fallback_cached(
                    *market,
                    base_stable.as_ref(),
                    &snapshot.asset_balances,
                    deposit_token_cache,
                )
                .await?
            {
                Some(token) => token,
                None => continue,
            };

            let hedge_notional = match self
                .estimate_target_hedge_notional_usd(*market, target_value, &deposit_token)
                .await?
            {
                Some(notional) => notional,
                None => continue,
            };

            if leverage > Decimal::ZERO {
                required_margin += hedge_notional / leverage;
                debug!(
                    market = %market_info.symbol,
                    target_weight = %target_weight,
                    target_value_usd = %target_value,
                    deposit_token = %deposit_token.symbol,
                    leverage = %leverage,
                    hedge_notional_usd = %hedge_notional,
                    incremental_required_margin = %(hedge_notional / leverage),
                    "Included market in dYdX reserve calculation"
                );
            }
        }

        let low_leverage_guard = if let Some(min_lev) = min_leverage {
            if min_lev > Decimal::ZERO {
                investable_capital * Decimal::from_f64(0.25).unwrap() / min_lev
            } else {
                Decimal::ZERO
            }
        } else {
            Decimal::ZERO
        };

        let guard_adjusted =
            required_margin * Decimal::from_f64(0.75).unwrap() + low_leverage_guard;
        let required_margin = required_margin.max(guard_adjusted);
        let required_equity = required_margin * Decimal::from_f64(1.5).unwrap();
        let required_free_collateral = (required_equity - required_margin).max(Decimal::ZERO);
        Ok((required_margin, required_equity, required_free_collateral))
    }

    async fn estimate_target_hedge_notional_usd(
        &self,
        market: Address,
        target_value_usd: Decimal,
        deposit_token: &TokenInfo,
    ) -> Result<Option<Decimal>> {
        if target_value_usd <= Decimal::ZERO {
            return Ok(None);
        }

        let market_info = match self.wallet_manager.market_tokens.get(&market) {
            Some(info) => info,
            None => return Ok(None),
        };
        let long_token = match self
            .wallet_manager
            .asset_tokens
            .get(&market_info.long_token_address)
        {
            Some(token) => token,
            None => return Ok(None),
        };
        let short_token = match self
            .wallet_manager
            .asset_tokens
            .get(&market_info.short_token_address)
        {
            Some(token) => token,
            None => return Ok(None),
        };

        if hedge_utils::STABLE_COINS.contains(&long_token.symbol.as_str()) {
            return Ok(None);
        }

        if deposit_token.last_mid_price_usd <= Decimal::ZERO {
            return Ok(None);
        }

        let deposit_amount = target_value_usd / deposit_token.last_mid_price_usd;
        if deposit_amount <= Decimal::ZERO {
            return Ok(None);
        }

        let deposit_request = if deposit_token.address == market_info.long_token_address {
            GmDepositRequest {
                market,
                long_amount: deposit_amount,
                short_amount: Decimal::ZERO,
            }
        } else {
            GmDepositRequest {
                market,
                long_amount: Decimal::ZERO,
                short_amount: deposit_amount,
            }
        };

        let deposit_amount_out = match self
            .gm_tx_manager
            .get_transaction_amount_out(&GmTxRequest::Deposit(deposit_request))
            .await?
        {
            GmAmountOutResponse::Deposit { amount_out } => amount_out,
            _ => Decimal::ZERO,
        };
        if deposit_amount_out <= Decimal::ZERO {
            return Ok(None);
        }

        let (long_amount_out, short_amount_out) = match self
            .gm_tx_manager
            .get_transaction_amount_out(&GmTxRequest::Withdrawal(GmWithdrawalRequest {
                market,
                amount: deposit_amount_out,
            }))
            .await?
        {
            GmAmountOutResponse::Withdrawal {
                long_amount_out,
                short_amount_out,
            } => (long_amount_out, short_amount_out),
            _ => (Decimal::ZERO, Decimal::ZERO),
        };

        let long_value = long_amount_out * long_token.last_mid_price_usd;
        let short_value = short_amount_out * short_token.last_mid_price_usd;
        let hedge_notional = Self::hedge_notional_from_withdrawal_values(
            long_value,
            short_value,
            hedge_utils::STABLE_COINS.contains(&short_token.symbol.as_str()),
        );

        if hedge_notional > Decimal::ZERO {
            Ok(Some(hedge_notional))
        } else {
            Ok(None)
        }
    }

    async fn fetch_token_hedgeinfo_map(&self) -> HashMap<String, Option<(Decimal, Decimal)>> {
        let client = self.dydx_client.lock().await;
        client.get_token_hedgeinfo_map().await.unwrap_or_default()
    }

    async fn fetch_dydx_snapshot_state(
        &self,
    ) -> (HashMap<String, Decimal>, Decimal, Decimal, Decimal) {
        let mut client = self.dydx_client.lock().await;
        let hedge_positions = client
            .get_dydx_subaccount_perp_positions()
            .await
            .unwrap_or_default();
        let dydx_main_usdc = client
            .get_dydx_usdc_balance()
            .await
            .unwrap_or(Decimal::ZERO);
        let summary = client.get_subaccount_summary().await;
        let (dydx_subaccount_equity, dydx_free_collateral) = match summary {
            Ok(summary) => (summary.equity, summary.free_collateral),
            Err(_) => (Decimal::ZERO, Decimal::ZERO),
        };
        (
            hedge_positions,
            dydx_main_usdc,
            dydx_subaccount_equity,
            dydx_free_collateral,
        )
    }

    async fn ensure_gas_reserve(
        &self,
        snapshot: &PortfolioSnapshot,
        reserve_state: &ReserveState,
        strategy_run_id: Option<i32>,
    ) -> Result<bool> {
        let target_eth = reserve_state.gas_reserve_target_eth;
        if target_eth <= Decimal::ZERO {
            return Ok(false);
        }

        let current_eth = snapshot.native_balance;
        let native_price = self.wallet_manager.native_token.last_mid_price_usd;
        let threshold_usd = self.planner_config.min_value_usd;
        let mut changed = false;
        debug!(
            current_eth = %current_eth,
            target_eth = %target_eth,
            native_price_usd = %native_price,
            threshold_usd = %threshold_usd,
            "Evaluating gas reserve state"
        );

        if current_eth < target_eth {
            let mut needed = target_eth - current_eth;
            let weth_address = WNT_ADDRESS.parse().unwrap();
            let weth_balance = self.wallet_manager.get_token_balance(weth_address).await?;
            info!(
                current_eth = %current_eth,
                target_eth = %target_eth,
                needed_eth = %needed,
                available_weth = %weth_balance,
                "Gas reserve below target; attempting top-up"
            );
            if weth_balance > Decimal::ZERO {
                let unwrap_amount = weth_balance.min(needed);
                let action = TradeAction::SpotSwap {
                    from_token: weth_address,
                    to_token: NATIVE_ADDRESS.parse().unwrap(),
                    amount: unwrap_amount,
                    side: "SELL".to_string(),
                };
                info!(action = %self.describe_action(&action), "Using WETH to replenish gas reserve");
                changed = self.execute_and_log(&action, strategy_run_id).await || changed;
                needed = (needed - unwrap_amount).max(Decimal::ZERO);
            }

            if needed > Decimal::ZERO {
                if let Some(base_stable) = self.get_preferred_stable_token(&snapshot.asset_balances)
                {
                    let source_stable = self
                        .get_stable_source_token(&snapshot.asset_balances, base_stable.address)
                        .unwrap_or(base_stable.clone());
                    let source_balance = self
                        .wallet_manager
                        .get_token_balance(source_stable.address)
                        .await?;
                    let usd_available = source_balance * source_stable.last_mid_price_usd;
                    let usd_needed = needed * native_price;
                    debug!(
                        source_stable = %source_stable.symbol,
                        source_balance = %source_balance,
                        usd_available = %usd_available,
                        usd_needed = %usd_needed,
                        "Evaluated stable funding for gas reserve"
                    );
                    if usd_available < usd_needed {
                        let excess_equity =
                            snapshot.dydx_subaccount_equity - reserve_state.required_equity;
                        let excess_free =
                            snapshot.dydx_free_collateral - reserve_state.required_free_collateral;
                        let excess = excess_equity.min(excess_free);
                        if excess > Decimal::ZERO {
                            let withdraw_amount =
                                (usd_needed - usd_available) / source_stable.last_mid_price_usd;
                            let withdraw_amount = withdraw_amount.min(excess);
                            if withdraw_amount > Decimal::ZERO {
                                let mut client = self.dydx_client.lock().await;
                                if let Err(e) =
                                    client.withdraw_from_subaccount(withdraw_amount).await
                                {
                                    warn!(error = ?e, "Failed to move funds from subaccount for gas reserve");
                                } else if let Err(e) = client
                                    .dydx_withdrawal(Some(withdraw_amount), None, true, None)
                                    .await
                                {
                                    warn!(error = ?e, "Failed to withdraw from dYdX for gas reserve");
                                } else {
                                    info!(
                                        amount = %withdraw_amount,
                                        "Triggered dYdX withdrawal for gas reserve; skipping ETH top-up this cycle"
                                    );
                                    return Ok(true);
                                }
                            }
                        }
                    }

                    let action = TradeAction::SpotSwap {
                        from_token: source_stable.address,
                        to_token: NATIVE_ADDRESS.parse().unwrap(),
                        amount: needed,
                        side: "BUY".to_string(),
                    };
                    info!(action = %self.describe_action(&action), "Buying native ETH to replenish gas reserve");
                    changed = self.execute_and_log(&action, strategy_run_id).await || changed;
                }
            }
        } else {
            let excess = current_eth - target_eth;
            let excess_usd = excess * native_price;
            debug!(
                excess_eth = %excess,
                excess_usd = %excess_usd,
                "Gas reserve above target"
            );
            if excess_usd > threshold_usd {
                if let Some(base_stable) = self.get_preferred_stable_token(&snapshot.asset_balances)
                {
                    let action = TradeAction::SpotSwap {
                        from_token: NATIVE_ADDRESS.parse().unwrap(),
                        to_token: base_stable.address,
                        amount: excess,
                        side: "SELL".to_string(),
                    };
                    info!(action = %self.describe_action(&action), "Selling excess native ETH above gas reserve target");
                    changed = self.execute_and_log(&action, strategy_run_id).await || changed;
                }
            }
        }

        Ok(changed)
    }

    fn build_withdraw_actions(
        &self,
        snapshot: &PortfolioSnapshot,
        deltas: &HashMap<Address, Decimal>,
    ) -> Vec<TradeAction> {
        let mut actions = Vec::new();
        for (market, delta) in deltas.iter() {
            if *delta >= Decimal::ZERO {
                continue;
            }
            let current_balance = snapshot
                .market_balances
                .get(market)
                .cloned()
                .unwrap_or(Decimal::ZERO);
            let withdraw_amount = (-*delta).min(current_balance);
            if withdraw_amount > Decimal::ZERO {
                actions.push(TradeAction::GmWithdrawal {
                    market: *market,
                    amount: withdraw_amount,
                });
            }
        }
        debug!(
            actions = ?self.describe_actions(&actions),
            "Built GM withdrawal actions"
        );
        actions
    }

    fn build_full_unwind_withdraw_actions(&self, snapshot: &PortfolioSnapshot) -> Vec<TradeAction> {
        let actions: Vec<TradeAction> = snapshot
            .market_balances
            .iter()
            .filter_map(|(market, balance)| {
                if *balance <= Decimal::ZERO {
                    return None;
                }
                let usd_value = snapshot
                    .market_values_usd
                    .get(market)
                    .cloned()
                    .unwrap_or(Decimal::ZERO);
                if usd_value < self.planner_config.unwind_min_value_usd {
                    return None;
                }
                Some(TradeAction::GmWithdrawal {
                    market: *market,
                    amount: *balance,
                })
            })
            .collect();
        debug!(
            actions = ?self.describe_actions(&actions),
            min_value_usd = %self.planner_config.unwind_min_value_usd,
            "Built full unwind GM withdrawal actions"
        );
        actions
    }

    fn build_full_unwind_hedge_actions(&self, snapshot: &PortfolioSnapshot) -> Vec<TradeAction> {
        let mut actions = Vec::new();

        for (ticker, size) in snapshot.hedge_positions.iter() {
            if size.abs() <= Decimal::ZERO {
                continue;
            }

            let token_symbol = match self.find_token_symbol_for_perp_ticker(ticker) {
                Some(symbol) => symbol,
                None => {
                    warn!(ticker = %ticker, "Unable to map perp ticker to token symbol for unwind");
                    continue;
                }
            };

            actions.push(TradeAction::HedgeOrder {
                token_symbol,
                size: size.abs(),
                side_is_buy: !size.is_sign_positive(),
                reduce_only: true,
            });
        }

        debug!(
            actions = ?self.describe_actions(&actions),
            "Built full unwind hedge actions"
        );
        actions
    }

    async fn execute_deposit_stage(
        &self,
        snapshot: &PortfolioSnapshot,
        deltas: &HashMap<Address, Decimal>,
        reserve_state: &ReserveState,
        strategy_run_id: Option<i32>,
    ) -> Result<bool> {
        let mut deposit_targets: Vec<(Address, Decimal)> = deltas
            .iter()
            .filter_map(|(market, delta)| {
                if *delta > Decimal::ZERO {
                    Some((*market, *delta))
                } else {
                    None
                }
            })
            .collect();

        if deposit_targets.is_empty() {
            debug!("Skipping deposit stage because there are no positive market deltas");
            return Ok(false);
        }

        let base_stable = match self.get_preferred_stable_token(&snapshot.asset_balances) {
            Some(token) => token,
            None => {
                warn!("No stable token available for deposits");
                return Ok(false);
            }
        };
        info!(
            strategy_run_id = ?strategy_run_id,
            base_stable = %base_stable.symbol,
            deposit_targets = ?self.summarize_market_deltas_map(&deposit_targets),
            "Planning deposit stage"
        );

        let mut preferred_tokens = HashSet::new();
        let mut deposit_token_cache = HashMap::new();
        let mut invalidated_markets = HashSet::new();
        for (market, _) in deposit_targets.iter() {
            if let Some(token) = self
                .select_deposit_token_with_fallback_cached(
                    *market,
                    Some(&base_stable),
                    &snapshot.asset_balances,
                    &mut deposit_token_cache,
                )
                .await?
            {
                preferred_tokens.insert(token.address);
            }
        }

        let cleaned = self
            .cleanup_asset_tokens_to_stable(
                &snapshot.asset_balances,
                &preferred_tokens,
                &base_stable,
                strategy_run_id,
                false,
                self.planner_config.min_value_usd,
            )
            .await;
        debug!(
            cleaned_preferred_asset_tokens = cleaned,
            preferred_tokens = ?preferred_tokens,
            "Completed pre-deposit asset cleanup"
        );

        let mut refreshed = if cleaned {
            self.build_snapshot().await?
        } else {
            snapshot.clone()
        };
        let mut changed = cleaned;
        for (market, delta_tokens) in deposit_targets.drain(..) {
            let market_info = match self.wallet_manager.market_tokens.get(&market) {
                Some(info) => info,
                None => continue,
            };
            let gm_price = market_info.last_mid_price_usd;
            if gm_price <= Decimal::ZERO {
                continue;
            }
            let usd_needed = delta_tokens * gm_price;
            if usd_needed <= Decimal::ZERO || usd_needed < self.planner_config.min_value_usd {
                continue;
            }
            debug!(
                market = %market_info.symbol,
                delta_tokens = %delta_tokens,
                usd_needed = %usd_needed,
                "Evaluating deposit target"
            );

            let deposit_token = match self
                .get_cached_or_recomputed_deposit_token(
                    market,
                    &base_stable,
                    &mut deposit_token_cache,
                    &mut invalidated_markets,
                )
                .await?
            {
                Some(token) => token,
                None => continue,
            };
            let funding_price = deposit_token.last_mid_price_usd;
            if funding_price <= Decimal::ZERO {
                continue;
            }
            let amount_needed = usd_needed / funding_price;
            debug!(
                market = %market_info.symbol,
                deposit_token = %deposit_token.symbol,
                funding_price_usd = %funding_price,
                amount_needed = %amount_needed,
                "Selected deposit token"
            );

            let mut funding_balance = self
                .wallet_manager
                .get_token_balance(deposit_token.address)
                .await?;
            if funding_balance < amount_needed {
                let shortfall = amount_needed - funding_balance;
                info!(
                    market = %market_info.symbol,
                    deposit_token = %deposit_token.symbol,
                    funding_balance = %funding_balance,
                    amount_needed = %amount_needed,
                    shortfall = %shortfall,
                    "Deposit stage needs additional funding"
                );
                if deposit_token.address == WNT_ADDRESS.parse().unwrap() {
                    let native_balance = self.wallet_manager.get_native_balance().await?;
                    let wrappable_native =
                        (native_balance - reserve_state.gas_reserve_target_eth).max(Decimal::ZERO);
                    if wrappable_native > Decimal::ZERO {
                        let wrap_amount = wrappable_native.min(shortfall);
                        let action = TradeAction::SpotSwap {
                            from_token: NATIVE_ADDRESS.parse().unwrap(),
                            to_token: deposit_token.address,
                            amount: wrap_amount,
                            side: "BUY".to_string(),
                        };
                        info!(action = %self.describe_action(&action), "Wrapping native ETH to fund GM deposit");
                        if self.execute_and_log(&action, strategy_run_id).await {
                            changed = true;
                            self.invalidate_markets_for_token(
                                &mut invalidated_markets,
                                deposit_token.address,
                            );
                        }
                    }
                }

                funding_balance = self
                    .wallet_manager
                    .get_token_balance(deposit_token.address)
                    .await?;
                if funding_balance < amount_needed {
                    let remaining = amount_needed - funding_balance;
                    let live_stable_balances = self.get_live_stable_balances().await?;
                    if let Some(source_stable) =
                        self.get_stable_source_token(&live_stable_balances, base_stable.address)
                    {
                        let action = TradeAction::SpotSwap {
                            from_token: source_stable.address,
                            to_token: deposit_token.address,
                            amount: remaining,
                            side: "BUY".to_string(),
                        };
                        info!(action = %self.describe_action(&action), "Swapping stable balance to fund GM deposit");
                        if self.execute_and_log(&action, strategy_run_id).await {
                            changed = true;
                            self.invalidate_markets_for_token(
                                &mut invalidated_markets,
                                deposit_token.address,
                            );
                        }
                    }
                }
            }

            let updated_balance = self
                .wallet_manager
                .get_token_balance(deposit_token.address)
                .await?;
            let deposit_amount = updated_balance.min(amount_needed);
            if deposit_amount <= Decimal::ZERO {
                warn!(market = ?market, "No funding available for deposit");
                continue;
            }

            let (long_amount, short_amount) =
                if deposit_token.address == market_info.long_token_address {
                    (deposit_amount, Decimal::ZERO)
                } else {
                    (Decimal::ZERO, deposit_amount)
                };

            let action = TradeAction::GmDeposit {
                market,
                long_amount,
                short_amount,
            };
            info!(action = %self.describe_action(&action), "Submitting GM deposit");
            if self.execute_and_log(&action, strategy_run_id).await {
                changed = true;
                if deposit_token.address != base_stable.address {
                    self.invalidate_markets_for_token(
                        &mut invalidated_markets,
                        deposit_token.address,
                    );
                }
                refreshed = self.build_snapshot().await?;
            }
        }

        if changed {
            refreshed = self.build_snapshot().await?;
        }

        changed = self
            .cleanup_asset_tokens_to_stable(
                &refreshed.asset_balances,
                &HashSet::new(),
                &base_stable,
                strategy_run_id,
                false,
                self.planner_config.min_value_usd,
            )
            .await
            || changed;

        Ok(changed)
    }

    async fn cleanup_asset_tokens_to_stable(
        &self,
        balances: &HashMap<Address, Decimal>,
        keep_tokens: &HashSet<Address>,
        base_stable: &TokenInfo,
        strategy_run_id: Option<i32>,
        include_other_stables: bool,
        min_value_usd: Decimal,
    ) -> bool {
        let mut changed = false;
        for (token_addr, balance) in balances.iter() {
            if *balance <= Decimal::ZERO {
                continue;
            }
            if *token_addr == base_stable.address {
                continue;
            }
            let is_stable = hedge_utils::STABLE_COINS.contains(
                &self
                    .wallet_manager
                    .asset_tokens
                    .get(token_addr)
                    .map(|t| t.symbol.as_str())
                    .unwrap_or(""),
            );
            if is_stable && !include_other_stables {
                continue;
            }
            if keep_tokens.contains(token_addr) {
                continue;
            }

            let token_price = self
                .wallet_manager
                .asset_tokens
                .get(token_addr)
                .map(|t| t.last_mid_price_usd)
                .unwrap_or(Decimal::ZERO);
            let token_value_usd = *balance * token_price;
            if token_value_usd < min_value_usd {
                debug!(
                    token = %self.token_symbol(*token_addr),
                    balance = %balance,
                    usd_value = %token_value_usd,
                    min_value_usd = %min_value_usd,
                    "Skipping asset cleanup because balance is below threshold"
                );
                continue;
            }

            let action = TradeAction::SpotSwap {
                from_token: *token_addr,
                to_token: base_stable.address,
                amount: *balance,
                side: "SELL".to_string(),
            };
            info!(
                action = %self.describe_action(&action),
                usd_value = %token_value_usd,
                include_other_stables,
                "Cleaning asset balance into target stable"
            );
            changed = self.execute_and_log(&action, strategy_run_id).await || changed;
        }
        changed
    }

    async fn select_deposit_token(
        &self,
        market: Address,
        base_stable: &TokenInfo,
        balances: &HashMap<Address, Decimal>,
    ) -> Result<Option<TokenInfo>> {
        let market_info = match self.wallet_manager.market_tokens.get(&market) {
            Some(info) => info,
            None => return Ok(None),
        };
        let long_token = match self
            .wallet_manager
            .asset_tokens
            .get(&market_info.long_token_address)
        {
            Some(token) => token,
            None => return Ok(None),
        };
        let short_token = match self
            .wallet_manager
            .asset_tokens
            .get(&market_info.short_token_address)
        {
            Some(token) => token,
            None => return Ok(None),
        };

        let long_fee = self.estimate_gm_fee_pct(market, long_token, true).await?;
        let short_fee = self.estimate_gm_fee_pct(market, short_token, false).await?;

        let long_balance_usd = balances
            .get(&long_token.address)
            .cloned()
            .unwrap_or(Decimal::ZERO)
            * long_token.last_mid_price_usd;
        let short_balance_usd = balances
            .get(&short_token.address)
            .cloned()
            .unwrap_or(Decimal::ZERO)
            * short_token.last_mid_price_usd;

        let long_swap_fee = if long_balance_usd > self.planner_config.min_value_usd {
            Decimal::ZERO
        } else {
            self.estimate_swap_fee_pct(base_stable, long_token)
                .await?
                .unwrap_or(Decimal::ZERO)
        };
        let short_swap_fee = if short_balance_usd > self.planner_config.min_value_usd {
            Decimal::ZERO
        } else {
            self.estimate_swap_fee_pct(base_stable, short_token)
                .await?
                .unwrap_or(Decimal::ZERO)
        };

        let long_total = long_fee.unwrap_or(Decimal::ZERO) + long_swap_fee;
        let short_total = short_fee.unwrap_or(Decimal::ZERO) + short_swap_fee;
        let cost_tie_break_threshold = Decimal::from_f64(0.001).unwrap();
        let cost_diff = (long_total - short_total).abs();

        let prefer_short = if cost_diff > cost_tie_break_threshold {
            short_total <= long_total
        } else if hedge_utils::STABLE_COINS.contains(&short_token.symbol.as_str()) {
            true
        } else if hedge_utils::STABLE_COINS.contains(&long_token.symbol.as_str()) {
            false
        } else {
            short_total <= long_total
        };

        Ok(Some(if prefer_short {
            short_token.clone()
        } else {
            long_token.clone()
        }))
    }

    async fn select_deposit_token_with_fallback_cached(
        &self,
        market: Address,
        base_stable: Option<&TokenInfo>,
        balances: &HashMap<Address, Decimal>,
        cache: &mut HashMap<Address, TokenInfo>,
    ) -> Result<Option<TokenInfo>> {
        if let Some(token) = cache.get(&market) {
            return Ok(Some(token.clone()));
        }

        let selected = match base_stable {
            Some(base_stable) => {
                self.select_deposit_token(market, base_stable, balances)
                    .await?
            }
            None => None,
        }
        .or_else(|| self.default_deposit_token_for_market(market));

        if let Some(token) = selected.clone() {
            cache.insert(market, token);
        }

        Ok(selected)
    }

    async fn get_cached_or_recomputed_deposit_token(
        &self,
        market: Address,
        base_stable: &TokenInfo,
        deposit_token_cache: &mut HashMap<Address, TokenInfo>,
        invalidated_markets: &mut HashSet<Address>,
    ) -> Result<Option<TokenInfo>> {
        if invalidated_markets.remove(&market) {
            let live_balances = self.get_live_market_pair_balances(market).await?;
            let selected = self
                .select_deposit_token(market, base_stable, &live_balances)
                .await?
                .or_else(|| self.default_deposit_token_for_market(market));
            if let Some(token) = selected.clone() {
                deposit_token_cache.insert(market, token);
            } else {
                deposit_token_cache.remove(&market);
            }
            return Ok(selected);
        }

        Ok(deposit_token_cache.get(&market).cloned())
    }

    async fn get_live_market_pair_balances(
        &self,
        market: Address,
    ) -> Result<HashMap<Address, Decimal>> {
        let market_info = match self.wallet_manager.market_tokens.get(&market) {
            Some(info) => info,
            None => return Ok(HashMap::new()),
        };
        let token_addresses = [
            market_info.long_token_address,
            market_info.short_token_address,
        ];
        self.wallet_manager
            .get_token_balances(&token_addresses)
            .await
    }

    async fn get_live_stable_balances(&self) -> Result<HashMap<Address, Decimal>> {
        let stable_addresses: Vec<Address> = self
            .wallet_manager
            .asset_tokens
            .values()
            .filter(|token| hedge_utils::STABLE_COINS.contains(&token.symbol.as_str()))
            .map(|token| token.address)
            .collect();
        self.wallet_manager
            .get_token_balances(&stable_addresses)
            .await
    }

    fn invalidate_markets_for_token(
        &self,
        invalidated_markets: &mut HashSet<Address>,
        token_address: Address,
    ) {
        for (market, market_info) in self.wallet_manager.market_tokens.iter() {
            if market_info.long_token_address == token_address
                || market_info.short_token_address == token_address
            {
                invalidated_markets.insert(*market);
            }
        }
    }

    fn default_deposit_token_for_market(&self, market: Address) -> Option<TokenInfo> {
        let market_info = self.wallet_manager.market_tokens.get(&market)?;
        let long_token = self
            .wallet_manager
            .asset_tokens
            .get(&market_info.long_token_address)?;
        let short_token = self
            .wallet_manager
            .asset_tokens
            .get(&market_info.short_token_address)?;

        if hedge_utils::STABLE_COINS.contains(&short_token.symbol.as_str()) {
            Some(short_token.clone())
        } else {
            Some(long_token.clone())
        }
    }

    async fn estimate_gm_fee_pct(
        &self,
        market: Address,
        token: &TokenInfo,
        is_long: bool,
    ) -> Result<Option<Decimal>> {
        if token.last_mid_price_usd <= Decimal::ZERO {
            return Ok(None);
        }
        let amount = Decimal::ONE / token.last_mid_price_usd;
        let request = if is_long {
            GmDepositRequest {
                market,
                long_amount: amount,
                short_amount: Decimal::ZERO,
            }
        } else {
            GmDepositRequest {
                market,
                long_amount: Decimal::ZERO,
                short_amount: amount,
            }
        };

        let value_in_usd = amount * token.last_mid_price_usd;
        if value_in_usd <= Decimal::ZERO {
            return Ok(None);
        }

        let response = self
            .gm_tx_manager
            .get_transaction_amount_out(&GmTxRequest::Deposit(request))
            .await?;
        let amount_out = match response {
            GmAmountOutResponse::Deposit { amount_out } => amount_out,
            _ => Decimal::ZERO,
        };

        let gm_price = self
            .wallet_manager
            .market_tokens
            .get(&market)
            .map(|token| token.last_mid_price_usd)
            .unwrap_or(Decimal::ZERO);
        if gm_price <= Decimal::ZERO {
            return Ok(None);
        }
        let value_out_usd = amount_out * gm_price;

        let fee_pct = ((value_in_usd - value_out_usd) / value_in_usd).max(Decimal::ZERO);
        Ok(Some(fee_pct))
    }

    async fn estimate_swap_fee_pct(
        &self,
        from_token: &TokenInfo,
        to_token: &TokenInfo,
    ) -> Result<Option<Decimal>> {
        if from_token.address == to_token.address {
            return Ok(Some(Decimal::ZERO));
        }
        if from_token.last_mid_price_usd <= Decimal::ZERO {
            return Ok(None);
        }

        let amount = Decimal::ONE / from_token.last_mid_price_usd;
        let request = QuoteRequest {
            from_token: from_token.address,
            from_token_decimals: from_token.decimals,
            to_token: to_token.address,
            to_token_decimals: to_token.decimals,
            amount,
            side: "SELL".to_string(),
            slippage_tolerance: Decimal::from_f64(0.5).unwrap(),
        };

        let quote = match self.paraswap_client.get_quote(&request).await {
            Ok(q) => q,
            Err(_) => return Ok(None),
        };
        if quote.from_amount_usd <= Decimal::ZERO {
            return Ok(None);
        }
        let fee_pct = ((quote.from_amount_usd - quote.to_amount_usd) / quote.from_amount_usd)
            .max(Decimal::ZERO);
        Ok(Some(fee_pct))
    }

    fn get_preferred_stable_token(
        &self,
        balances: &HashMap<Address, Decimal>,
    ) -> Option<TokenInfo> {
        let mut usdc = None::<TokenInfo>;
        let mut usdce = None::<TokenInfo>;
        for token in self.wallet_manager.asset_tokens.values() {
            if !hedge_utils::STABLE_COINS.contains(&token.symbol.as_str()) {
                continue;
            }
            if token.symbol == "USDC" {
                usdc = Some(token.clone());
                continue;
            }
            if token.symbol == "USDC.e" {
                usdce = Some(token.clone());
            }
        }

        if let Some(token) = usdc.clone() {
            return Some(token);
        }
        if let Some(token) = usdce.clone() {
            return Some(token);
        }

        let mut best: Option<(TokenInfo, Decimal)> = None;
        for token in self.wallet_manager.asset_tokens.values() {
            if !hedge_utils::STABLE_COINS.contains(&token.symbol.as_str()) {
                continue;
            }
            let balance = balances
                .get(&token.address)
                .cloned()
                .unwrap_or(Decimal::ZERO);
            let usd = balance * token.last_mid_price_usd;
            if usd <= Decimal::ZERO {
                continue;
            }
            let is_better = best
                .as_ref()
                .map(|(_, best_usd)| usd > *best_usd)
                .unwrap_or(true);
            if is_better {
                best = Some((token.clone(), usd));
            }
        }
        best.map(|(token, _)| token)
    }

    fn resolve_target_stable_token(
        &self,
        target_symbol: &str,
        balances: &HashMap<Address, Decimal>,
    ) -> Option<TokenInfo> {
        self.wallet_manager
            .asset_tokens
            .values()
            .find(|token| token.symbol == target_symbol)
            .cloned()
            .or_else(|| self.get_preferred_stable_token(balances))
    }

    fn get_stable_source_token(
        &self,
        balances: &HashMap<Address, Decimal>,
        prefer_addr: Address,
    ) -> Option<TokenInfo> {
        if let Some(token) = self.wallet_manager.asset_tokens.get(&prefer_addr) {
            let balance = balances.get(&prefer_addr).cloned().unwrap_or(Decimal::ZERO);
            if balance > Decimal::ZERO {
                return Some(token.clone());
            }
        }

        let mut best: Option<(TokenInfo, Decimal)> = None;
        for token in self.wallet_manager.asset_tokens.values() {
            if !hedge_utils::STABLE_COINS.contains(&token.symbol.as_str()) {
                continue;
            }
            let balance = balances
                .get(&token.address)
                .cloned()
                .unwrap_or(Decimal::ZERO);
            let usd = balance * token.last_mid_price_usd;
            if usd <= Decimal::ZERO {
                continue;
            }
            let is_better = best
                .as_ref()
                .map(|(_, best_usd)| usd > *best_usd)
                .unwrap_or(true);
            if is_better {
                best = Some((token.clone(), usd));
            }
        }
        best.map(|(token, _)| token)
    }

    fn find_token_symbol_for_perp_ticker(&self, ticker: &str) -> Option<String> {
        hedge_utils::get_token_symbol_for_dydx_perp_ticker(ticker)
    }

    async fn build_hedge_actions(
        &self,
        snapshot: &PortfolioSnapshot,
        target_weights: &HashMap<Address, Decimal>,
    ) -> Result<Vec<TradeAction>> {
        let mut actions = Vec::new();

        for (market, gm_balance) in snapshot.market_balances.iter() {
            if *gm_balance <= Decimal::ZERO {
                continue;
            }
            if target_weights.get(market).cloned().unwrap_or(Decimal::ZERO) <= Decimal::ZERO {
                continue;
            }

            let market_info = match self.wallet_manager.market_tokens.get(market) {
                Some(info) => info,
                None => continue,
            };
            let long_token = match self
                .wallet_manager
                .asset_tokens
                .get(&market_info.long_token_address)
            {
                Some(token) => token,
                None => continue,
            };
            let short_token = match self
                .wallet_manager
                .asset_tokens
                .get(&market_info.short_token_address)
            {
                Some(token) => token,
                None => continue,
            };
            if hedge_utils::STABLE_COINS.contains(&long_token.symbol.as_str()) {
                continue;
            }

            let response = self
                .gm_tx_manager
                .get_transaction_amount_out(&GmTxRequest::Withdrawal(GmWithdrawalRequest {
                    market: *market,
                    amount: *gm_balance,
                }))
                .await?;

            let (long_out, short_out) = match response {
                GmAmountOutResponse::Withdrawal {
                    long_amount_out,
                    short_amount_out,
                } => (long_amount_out, short_amount_out),
                _ => (Decimal::ZERO, Decimal::ZERO),
            };

            let long_value = long_out * long_token.last_mid_price_usd;
            let short_value = short_out * short_token.last_mid_price_usd;

            let hedge_notional = Self::hedge_notional_from_withdrawal_values(
                long_value,
                short_value,
                hedge_utils::STABLE_COINS.contains(&short_token.symbol.as_str()),
            );
            if hedge_notional <= Decimal::ZERO {
                continue;
            }

            let target_size = -(hedge_notional / long_token.last_mid_price_usd);
            let ticker = hedge_utils::get_dydx_perp_ticker(&long_token.symbol);
            let current_size = snapshot
                .hedge_positions
                .get(&ticker)
                .cloned()
                .unwrap_or(Decimal::ZERO);
            let delta = target_size - current_size;

            if delta.abs() * long_token.last_mid_price_usd < self.planner_config.min_value_usd {
                continue;
            }

            actions.push(TradeAction::HedgeOrder {
                token_symbol: hedge_utils::get_dydx_perp_base_symbol(&long_token.symbol),
                size: delta.abs(),
                side_is_buy: delta > Decimal::ZERO,
                reduce_only: false,
            });
        }

        Ok(actions)
    }

    fn hedge_notional_from_withdrawal_values(
        long_value_usd: Decimal,
        short_value_usd: Decimal,
        short_is_stable: bool,
    ) -> Decimal {
        if short_is_stable {
            long_value_usd.max(Decimal::ZERO)
        } else {
            (long_value_usd + short_value_usd).max(Decimal::ZERO)
        }
    }

    async fn manage_dydx_reserves(
        &self,
        snapshot: &PortfolioSnapshot,
        reserve_state: &ReserveState,
    ) -> Result<bool> {
        let mut client = self.dydx_client.lock().await;
        let sub_equity = snapshot.dydx_subaccount_equity;
        let sub_free = snapshot.dydx_free_collateral;
        let required_equity = reserve_state.required_equity;
        let required_free = reserve_state.required_free_collateral;

        let equity_shortfall = (required_equity - sub_equity).max(Decimal::ZERO);
        let free_shortfall = (required_free - sub_free).max(Decimal::ZERO);
        let shortfall = equity_shortfall.max(free_shortfall);
        debug!(
            subaccount_equity = %sub_equity,
            free_collateral = %sub_free,
            required_equity = %required_equity,
            required_free_collateral = %required_free,
            shortfall = %shortfall,
            "Evaluated dYdX reserve state"
        );

        if shortfall <= self.planner_config.min_value_usd {
            return Ok(false);
        }

        let main_usdc = snapshot.dydx_main_usdc;
        if main_usdc >= shortfall {
            if let Err(e) = client.deposit_to_subaccount(shortfall).await {
                warn!(error = ?e, "Failed to move USDC to subaccount");
                return Ok(false);
            } else {
                info!(amount = %shortfall, "Moved USDC to dYdX subaccount");
            }
            return Ok(true);
        }

        let mut changed = false;
        if main_usdc > Decimal::ZERO {
            if let Err(e) = client.deposit_to_subaccount(main_usdc).await {
                warn!(error = ?e, "Failed to move USDC to subaccount");
                return Ok(false);
            }
            info!(amount = %main_usdc, "Moved available main-account USDC to dYdX subaccount");
            changed = true;
        }

        let remaining = shortfall - main_usdc;
        if remaining > self.planner_config.min_value_usd {
            info!(amount = %remaining, "Funding dYdX reserve via SkipGo");
            if let Err(e) = client.dydx_deposit(Some(remaining), None, true, None).await {
                warn!(error = ?e, "Failed to deposit to dYdX");
            } else {
                changed = true;
            }
        }

        Ok(changed)
    }

    async fn execute_actions(&self, actions: &[TradeAction], strategy_run_id: Option<i32>) -> bool {
        if actions.is_empty() {
            debug!(strategy_run_id = ?strategy_run_id, "No actions to execute in this stage");
            return false;
        }
        info!(
            strategy_run_id = ?strategy_run_id,
            action_count = actions.len(),
            actions = ?self.describe_actions(actions),
            "Executing action batch"
        );
        let mut changed = false;
        for action in actions.iter() {
            changed = self.execute_and_log(action, strategy_run_id).await || changed;
        }
        changed
    }

    async fn execute_and_log(&self, action: &TradeAction, strategy_run_id: Option<i32>) -> bool {
        info!(
            strategy_run_id = ?strategy_run_id,
            action = %self.describe_action(action),
            "Executing action"
        );
        let execution = self.execute_action(action).await;
        info!(
            strategy_run_id = ?strategy_run_id,
            action = %self.describe_action(action),
            status = execution.status.as_str(),
            tx_hash = ?execution.tx_hash,
            "Action execution finished"
        );
        if let Err(e) = self
            .log_trade(strategy_run_id, action, execution.status, execution.tx_hash)
            .await
        {
            warn!(error = ?e, action = ?action, "Failed to log trade");
        }
        execution.status == TradeStatus::Executed
    }

    async fn execute_action(&self, action: &TradeAction) -> ActionExecutionResult {
        let result = match action {
            TradeAction::GmDeposit {
                market,
                long_amount,
                short_amount,
            } => {
                let request = GmDepositRequest {
                    market: *market,
                    long_amount: *long_amount,
                    short_amount: *short_amount,
                };
                self.gm_tx_manager
                    .execute_transaction(&GmTxRequest::Deposit(request))
                    .await
            }
            TradeAction::GmWithdrawal { market, amount } => {
                let request = GmWithdrawalRequest {
                    market: *market,
                    amount: *amount,
                };
                self.gm_tx_manager
                    .execute_transaction(&GmTxRequest::Withdrawal(request))
                    .await
            }
            TradeAction::GmShift {
                from_market,
                to_market,
                amount,
            } => {
                let request = GmShiftRequest {
                    from_market: *from_market,
                    to_market: *to_market,
                    amount: *amount,
                };
                self.gm_tx_manager
                    .execute_transaction(&GmTxRequest::Shift(request))
                    .await
            }
            TradeAction::SpotSwap {
                from_token,
                to_token,
                amount,
                side,
            } => {
                let request = SwapRequest {
                    from_token_address: *from_token,
                    to_token_address: *to_token,
                    amount: *amount,
                    side: side.clone(),
                };
                self.swap_manager.execute_swap(&request).await
            }
            TradeAction::HedgeOrder {
                token_symbol,
                size,
                side_is_buy,
                reduce_only,
            } => {
                let mut client = self.dydx_client.lock().await;
                if *reduce_only {
                    client.reduce_perp_position(token_symbol, Some(*size)).await
                } else {
                    client
                        .submit_perp_order(token_symbol, *size, *side_is_buy)
                        .await
                }
            }
        };

        match result {
            Ok(tx_hash) => ActionExecutionResult {
                status: TradeStatus::Executed,
                tx_hash: Some(tx_hash),
            },
            Err(e) => {
                error!(error = ?e, action = ?action, "Execution failed");
                ActionExecutionResult {
                    status: TradeStatus::Failed,
                    tx_hash: None,
                }
            }
        }
    }

    async fn record_strategy_run(&self, portfolio_data: &PortfolioData) -> Result<i32> {
        let total_weight = portfolio_data.weights.sum();
        let portfolio_return = portfolio_data.weights.dot(&portfolio_data.expected_returns);
        let portfolio_variance = portfolio_data.weights.dot(
            &portfolio_data
                .covariance_matrix
                .dot(&portfolio_data.weights),
        );
        let portfolio_volatility = portfolio_variance.sqrt().unwrap_or(Decimal::ZERO);
        let sharpe = if portfolio_volatility > Decimal::ZERO {
            portfolio_return / portfolio_volatility
        } else {
            Decimal::ZERO
        };

        let run = NewStrategyRunModel {
            timestamp: Utc::now(),
            strategy_version: self.config.strategy_version.clone(),
            total_weight,
            expected_return_bps: portfolio_return * Decimal::from_f64(10000.0).unwrap(),
            volatility_bps: portfolio_volatility * Decimal::from_f64(10000.0).unwrap(),
            sharpe,
        };
        let run_id = self.db_manager.insert_strategy_run(&run).await?;

        for (i, market) in portfolio_data.market_addresses.iter().enumerate() {
            if let Some(market_id) = self.db_manager.market_id_map.get(market).cloned() {
                let variance = portfolio_data.covariance_matrix[[i, i]];
                let target = NewStrategyTargetModel {
                    strategy_run_id: run_id,
                    market_id,
                    target_weight: portfolio_data.weights[i],
                    expected_return_bps: portfolio_data.expected_returns[i]
                        * Decimal::from_f64(10000.0).unwrap(),
                    variance_bps: variance * Decimal::from_f64(10000.0).unwrap(),
                };
                self.db_manager.insert_strategy_target(&target).await?;
            }
        }

        Ok(run_id)
    }

    async fn record_snapshot(
        &self,
        strategy_run_id: Option<i32>,
        snapshot: &PortfolioSnapshot,
    ) -> Result<()> {
        let prev_snapshot = self.db_manager.get_latest_portfolio_snapshot().await?;
        let pnl = match prev_snapshot {
            Some(prev) => snapshot.total_value_usd - prev.total_value_usd.unwrap_or(Decimal::ZERO),
            None => Decimal::ZERO,
        };

        let new_snapshot = NewPortfolioSnapshotModel {
            strategy_run_id,
            timestamp: snapshot.timestamp,
            total_value_usd: snapshot.total_value_usd,
            arbitrum_value_usd: snapshot.arbitrum_value_usd,
            market_value_usd: snapshot.market_value_usd,
            asset_value_usd: snapshot.asset_value_usd,
            native_balance: snapshot.native_balance,
            native_value_usd: snapshot.native_value_usd,
            dydx_main_usdc: snapshot.dydx_main_usdc,
            dydx_subaccount_equity: snapshot.dydx_subaccount_equity,
            dydx_free_collateral: snapshot.dydx_free_collateral,
            pnl_usd: pnl,
        };
        let snapshot_id = self
            .db_manager
            .insert_portfolio_snapshot(&new_snapshot)
            .await?;
        info!(
            snapshot_id,
            strategy_run_id = ?strategy_run_id,
            total_value_usd = %snapshot.total_value_usd,
            pnl_usd = %pnl,
            "Recorded portfolio snapshot"
        );

        let positions = self.build_position_snapshots(snapshot, snapshot_id).await?;
        if !positions.is_empty() {
            info!(
                snapshot_id,
                position_count = positions.len(),
                "Recording position snapshots"
            );
            self.db_manager.insert_position_snapshots(positions).await?;
        }

        Ok(())
    }

    async fn build_position_snapshots(
        &self,
        snapshot: &PortfolioSnapshot,
        portfolio_snapshot_id: i32,
    ) -> Result<Vec<NewPositionSnapshotModel>> {
        let mut positions = Vec::new();

        for (market, balance) in snapshot.market_balances.iter() {
            if *balance <= Decimal::ZERO {
                continue;
            }
            let market_id = self.db_manager.market_id_map.get(market).cloned();
            let symbol = self
                .wallet_manager
                .market_tokens
                .get(market)
                .map(|token| token.symbol.clone());
            let usd_value = snapshot.market_values_usd.get(market).cloned();
            positions.push(NewPositionSnapshotModel {
                portfolio_snapshot_id,
                position_type: "gm_market".to_string(),
                market_id,
                token_id: None,
                symbol,
                size: Some(*balance),
                usd_value,
            });
        }

        for (token, balance) in snapshot.asset_balances.iter() {
            if *balance <= Decimal::ZERO {
                continue;
            }
            let token_id = self.db_manager.token_id_map.get(token).cloned();
            let symbol = self
                .wallet_manager
                .asset_tokens
                .get(token)
                .map(|token| token.symbol.clone())
                .or_else(|| {
                    if *token == self.wallet_manager.native_token.address {
                        Some(self.wallet_manager.native_token.symbol.clone())
                    } else {
                        None
                    }
                });
            let usd_value = snapshot.asset_values_usd.get(token).cloned();
            positions.push(NewPositionSnapshotModel {
                portfolio_snapshot_id,
                position_type: "token_balance".to_string(),
                market_id: None,
                token_id,
                symbol,
                size: Some(*balance),
                usd_value,
            });
        }

        let native_balance = snapshot.native_balance;
        if native_balance > Decimal::ZERO {
            positions.push(NewPositionSnapshotModel {
                portfolio_snapshot_id,
                position_type: "native_balance".to_string(),
                market_id: None,
                token_id: None,
                symbol: Some("ETH".to_string()),
                size: Some(native_balance),
                usd_value: Some(snapshot.native_value_usd),
            });
        }

        for (ticker, size) in snapshot.hedge_positions.iter() {
            if size.abs() <= Decimal::ZERO {
                continue;
            }
            positions.push(NewPositionSnapshotModel {
                portfolio_snapshot_id,
                position_type: "hedge_perp".to_string(),
                market_id: None,
                token_id: None,
                symbol: Some(ticker.clone()),
                size: Some(*size),
                usd_value: None,
            });
        }

        Ok(positions)
    }

    async fn log_trade(
        &self,
        strategy_run_id: Option<i32>,
        action: &TradeAction,
        status: TradeStatus,
        tx_hash: Option<String>,
    ) -> Result<()> {
        let (market_id, from_token_id, to_token_id, amount_in, amount_out, usd_value, details) =
            self.build_trade_details(action);

        let trade = NewTradeModel {
            timestamp: Utc::now(),
            action_type: action.action_type().to_string(),
            strategy_run_id,
            market_id,
            from_token_id,
            to_token_id,
            amount_in,
            amount_out,
            usd_value,
            fee_usd: None,
            tx_hash,
            status: status.as_str().to_string(),
            details,
        };
        self.db_manager.insert_trade(&trade).await?;
        info!(
            strategy_run_id = ?strategy_run_id,
            action_type = %action.action_type(),
            status = %status.as_str(),
            tx_hash = ?trade.tx_hash,
            "Recorded trade log"
        );
        Ok(())
    }

    fn snapshot_summary(&self, snapshot: &PortfolioSnapshot) -> String {
        format!(
            "total={} arbitrum={} gm={} assets={} native={} native_usd={} dydx_main_usdc={} dydx_equity={} dydx_free_collateral={} gm_positions={} asset_positions={} hedge_positions={}",
            snapshot.total_value_usd,
            snapshot.arbitrum_value_usd,
            snapshot.market_value_usd,
            snapshot.asset_value_usd,
            snapshot.native_balance,
            snapshot.native_value_usd,
            snapshot.dydx_main_usdc,
            snapshot.dydx_subaccount_equity,
            snapshot.dydx_free_collateral,
            snapshot.market_balances.len(),
            snapshot.asset_balances.len(),
            snapshot.hedge_positions.len(),
        )
    }

    fn summarize_target_weights(&self, target_weights: &HashMap<Address, Decimal>) -> Vec<String> {
        let mut entries: Vec<(String, Decimal)> = target_weights
            .iter()
            .filter_map(|(market, weight)| {
                if *weight <= Decimal::ZERO {
                    return None;
                }
                Some((self.market_symbol(*market), *weight))
            })
            .collect();
        entries.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        entries
            .into_iter()
            .take(10)
            .map(|(symbol, weight)| format!("{}={}", symbol, weight))
            .collect()
    }

    fn summarize_market_values(&self, values: &HashMap<Address, Decimal>) -> Vec<String> {
        let mut entries: Vec<(String, Decimal)> = values
            .iter()
            .map(|(market, value)| (self.market_symbol(*market), *value))
            .collect();
        entries.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        entries
            .into_iter()
            .take(10)
            .map(|(symbol, value)| format!("{}={}", symbol, value))
            .collect()
    }

    fn summarize_market_deltas(&self, deltas: &HashMap<Address, Decimal>) -> Vec<String> {
        let mut entries: Vec<(String, Decimal)> = deltas
            .iter()
            .filter_map(|(market, delta)| {
                if delta.abs() <= Decimal::ZERO {
                    return None;
                }
                let usd_delta = self
                    .wallet_manager
                    .market_tokens
                    .get(market)
                    .map(|token| *delta * token.last_mid_price_usd)
                    .unwrap_or(Decimal::ZERO);
                Some((self.market_symbol(*market), usd_delta))
            })
            .collect();
        entries.sort_by(|a, b| {
            b.1.abs()
                .partial_cmp(&a.1.abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        entries
            .into_iter()
            .take(10)
            .map(|(symbol, value)| format!("{}={}", symbol, value))
            .collect()
    }

    fn summarize_market_deltas_map(&self, deltas: &[(Address, Decimal)]) -> Vec<String> {
        let mut entries: Vec<(String, Decimal)> = deltas
            .iter()
            .map(|(market, delta)| (self.market_symbol(*market), *delta))
            .collect();
        entries.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        entries
            .into_iter()
            .take(10)
            .map(|(symbol, delta)| format!("{}={}", symbol, delta))
            .collect()
    }

    fn describe_actions(&self, actions: &[TradeAction]) -> Vec<String> {
        actions
            .iter()
            .take(12)
            .map(|action| self.describe_action(action))
            .collect()
    }

    fn describe_action(&self, action: &TradeAction) -> String {
        match action {
            TradeAction::GmDeposit {
                market,
                long_amount,
                short_amount,
            } => format!(
                "gm_deposit market={} long_amount={} short_amount={}",
                self.market_symbol(*market),
                long_amount,
                short_amount
            ),
            TradeAction::GmWithdrawal { market, amount } => format!(
                "gm_withdrawal market={} amount={}",
                self.market_symbol(*market),
                amount
            ),
            TradeAction::GmShift {
                from_market,
                to_market,
                amount,
            } => format!(
                "gm_shift from={} to={} amount={}",
                self.market_symbol(*from_market),
                self.market_symbol(*to_market),
                amount
            ),
            TradeAction::SpotSwap {
                from_token,
                to_token,
                amount,
                side,
            } => format!(
                "spot_swap side={} from={} to={} amount={}",
                side,
                self.token_symbol(*from_token),
                self.token_symbol(*to_token),
                amount
            ),
            TradeAction::HedgeOrder {
                token_symbol,
                size,
                side_is_buy,
                reduce_only,
            } => format!(
                "hedge_order token={} side={} size={} reduce_only={}",
                token_symbol,
                if *side_is_buy { "buy" } else { "sell" },
                size,
                reduce_only
            ),
        }
    }

    fn market_symbol(&self, market: Address) -> String {
        self.wallet_manager
            .market_tokens
            .get(&market)
            .map(|token| token.symbol.clone())
            .unwrap_or_else(|| format!("{:?}", market))
    }

    fn token_symbol(&self, token: Address) -> String {
        if token == NATIVE_ADDRESS.parse().unwrap() {
            return self.wallet_manager.native_token.symbol.clone();
        }
        self.wallet_manager
            .asset_tokens
            .get(&token)
            .map(|token| token.symbol.clone())
            .or_else(|| {
                self.wallet_manager
                    .market_tokens
                    .get(&token)
                    .map(|token| token.symbol.clone())
            })
            .unwrap_or_else(|| format!("{:?}", token))
    }

    fn build_trade_details(
        &self,
        action: &TradeAction,
    ) -> (
        Option<i32>,
        Option<i32>,
        Option<i32>,
        Option<Decimal>,
        Option<Decimal>,
        Option<Decimal>,
        Option<String>,
    ) {
        match action {
            TradeAction::GmDeposit {
                market,
                long_amount,
                short_amount,
            } => {
                let market_id = self.db_manager.market_id_map.get(market).cloned();
                let mut usd_value = Decimal::ZERO;
                if let Some(info) = self.wallet_manager.market_tokens.get(market) {
                    let long_price = self
                        .wallet_manager
                        .asset_tokens
                        .get(&info.long_token_address)
                        .map(|t| t.last_mid_price_usd)
                        .unwrap_or(Decimal::ZERO);
                    let short_price = self
                        .wallet_manager
                        .asset_tokens
                        .get(&info.short_token_address)
                        .map(|t| t.last_mid_price_usd)
                        .unwrap_or(Decimal::ZERO);
                    usd_value = *long_amount * long_price + *short_amount * short_price;
                }
                let details = serde_json::json!({
                    "long_amount": long_amount,
                    "short_amount": short_amount,
                })
                .to_string();
                (
                    market_id,
                    None,
                    None,
                    Some(*long_amount + *short_amount),
                    None,
                    Some(usd_value),
                    Some(details),
                )
            }
            TradeAction::GmWithdrawal { market, amount } => {
                let market_id = self.db_manager.market_id_map.get(market).cloned();
                let usd_value = self
                    .wallet_manager
                    .market_tokens
                    .get(market)
                    .map(|t| t.last_mid_price_usd * *amount)
                    .unwrap_or(Decimal::ZERO);
                let details = serde_json::json!({
                    "amount": amount,
                })
                .to_string();
                (
                    market_id,
                    None,
                    None,
                    Some(*amount),
                    None,
                    Some(usd_value),
                    Some(details),
                )
            }
            TradeAction::GmShift {
                from_market,
                to_market,
                amount,
            } => {
                let market_id = self.db_manager.market_id_map.get(from_market).cloned();
                let usd_value = self
                    .wallet_manager
                    .market_tokens
                    .get(from_market)
                    .map(|t| t.last_mid_price_usd * *amount)
                    .unwrap_or(Decimal::ZERO);
                let details = serde_json::json!({
                    "to_market": format!("{:?}", to_market),
                })
                .to_string();
                (
                    market_id,
                    None,
                    None,
                    Some(*amount),
                    None,
                    Some(usd_value),
                    Some(details),
                )
            }
            TradeAction::SpotSwap {
                from_token,
                to_token,
                amount,
                side,
            } => {
                let from_token_id = self.db_manager.token_id_map.get(from_token).cloned();
                let to_token_id = self.db_manager.token_id_map.get(to_token).cloned();
                let price_for = |addr: &Address| {
                    if *addr == NATIVE_ADDRESS.parse().unwrap() {
                        self.wallet_manager.native_token.last_mid_price_usd
                    } else {
                        self.wallet_manager
                            .asset_tokens
                            .get(addr)
                            .map(|t| t.last_mid_price_usd)
                            .unwrap_or(Decimal::ZERO)
                    }
                };
                let usd_value = if side == "SELL" {
                    price_for(from_token) * *amount
                } else {
                    price_for(to_token) * *amount
                };
                (
                    None,
                    from_token_id,
                    to_token_id,
                    Some(*amount),
                    None,
                    Some(usd_value),
                    Some(
                        serde_json::json!({
                            "side": side,
                        })
                        .to_string(),
                    ),
                )
            }
            TradeAction::HedgeOrder {
                token_symbol,
                size,
                side_is_buy,
                reduce_only,
            } => {
                let details = serde_json::json!({
                    "token_symbol": token_symbol,
                    "side_is_buy": side_is_buy,
                    "reduce_only": reduce_only,
                })
                .to_string();
                (None, None, None, Some(*size), None, None, Some(details))
            }
        }
    }
}
