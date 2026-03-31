use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::Utc;
use ethers::types::Address;
use eyre::Result;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use tracing::{error, info, warn};

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
use super::types::{PlannerConfig, PortfolioSnapshot, ReserveState, TradeAction, TradeStatus};

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
        let mut snapshot = self.build_snapshot().await?;
        let reserve_state = self
            .compute_reserve_state(portfolio_data, &snapshot)
            .await?;

        if reserve_state.investable_capital <= Decimal::ZERO {
            warn!(
                total_value_usd = %snapshot.total_value_usd,
                reserve_total = %reserve_state.reserve_total,
                "No investable capital after reserves; skipping"
            );
            return Ok(());
        }

        let strategy_run_id = self.record_strategy_run(portfolio_data).await?;

        self.maybe_withdraw_dydx_excess(&snapshot, &reserve_state)
            .await?;
        snapshot = self.build_snapshot().await?;

        self.ensure_gas_reserve(&snapshot, &reserve_state, strategy_run_id)
            .await?;
        snapshot = self.build_snapshot().await?;

        let target_weights = compute_target_weights(portfolio_data);
        let target_values =
            compute_target_values(&target_weights, reserve_state.investable_capital);

        let mut deltas = compute_market_deltas(
            &snapshot,
            &target_values,
            &self.wallet_manager,
            &self.planner_config,
        );

        // Stage 1: Shifts
        let (shift_actions, _) = plan_shift_actions(&deltas, &self.wallet_manager);
        self.execute_actions(&shift_actions, strategy_run_id).await;

        snapshot = self.build_snapshot().await?;
        deltas = compute_market_deltas(
            &snapshot,
            &target_values,
            &self.wallet_manager,
            &self.planner_config,
        );

        // Stage 2: Withdrawals
        let withdraw_actions = self.build_withdraw_actions(&snapshot, &deltas);
        self.execute_actions(&withdraw_actions, strategy_run_id)
            .await;

        snapshot = self.build_snapshot().await?;
        self.ensure_gas_reserve(&snapshot, &reserve_state, strategy_run_id)
            .await?;
        snapshot = self.build_snapshot().await?;
        deltas = compute_market_deltas(
            &snapshot,
            &target_values,
            &self.wallet_manager,
            &self.planner_config,
        );

        // Stage 3: Deposits + cleanup swaps
        self.execute_deposit_stage(&snapshot, &deltas, strategy_run_id)
            .await?;

        snapshot = self.build_snapshot().await?;

        // Stage 4: dYdX reserve management
        self.manage_dydx_reserves(&snapshot, &reserve_state).await?;

        // Stage 5: Hedge adjustments
        let hedge_actions = self
            .build_hedge_actions(portfolio_data, &snapshot, &target_weights)
            .await?;
        self.execute_actions(&hedge_actions, strategy_run_id).await;

        let post_snapshot = self.build_snapshot().await?;
        self.record_snapshot(&post_snapshot).await?;

        Ok(())
    }

    async fn maybe_withdraw_dydx_excess(
        &self,
        snapshot: &PortfolioSnapshot,
        reserve_state: &ReserveState,
    ) -> Result<()> {
        let excess_equity = snapshot.dydx_subaccount_equity - reserve_state.required_equity;
        let excess_free = snapshot.dydx_free_collateral - reserve_state.required_free_collateral;
        let excess = excess_equity.min(excess_free);
        let shortfall = reserve_state.investable_capital - snapshot.arbitrum_value_usd;
        if excess <= Decimal::ZERO || shortfall <= self.planner_config.min_value_usd {
            return Ok(());
        }

        let amount = excess.min(shortfall);
        if amount <= self.planner_config.min_value_usd {
            return Ok(());
        }

        let mut client = self.dydx_client.lock().await;
        if let Err(e) = client.withdraw_from_subaccount(amount).await {
            warn!(error = ?e, "Failed to move funds from subaccount for deployment");
            return Ok(());
        }
        if let Err(e) = client.dydx_withdrawal(Some(amount), None, true, None).await {
            warn!(error = ?e, "Failed to withdraw dYdX excess for deployment");
        } else {
            info!(amount = %amount, "Withdrawing dYdX excess for deployment");
        }

        Ok(())
    }

    async fn build_snapshot(&self) -> Result<PortfolioSnapshot> {
        let market_balances = self.wallet_manager.get_market_token_balances().await?;
        let asset_balances = self.wallet_manager.get_asset_token_balances().await?;

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

        let hedge_positions = {
            let client = self.dydx_client.lock().await;
            client
                .get_dydx_subaccount_perp_positions()
                .await
                .unwrap_or_default()
        };

        let (dydx_main_usdc, dydx_subaccount_equity, dydx_free_collateral) = {
            let mut client = self.dydx_client.lock().await;
            let main = client
                .get_dydx_usdc_balance()
                .await
                .unwrap_or(Decimal::ZERO);
            let summary = client.get_subaccount_summary().await;
            let (equity, free_collateral) = match summary {
                Ok(summary) => (summary.equity, summary.free_collateral),
                Err(_) => (Decimal::ZERO, Decimal::ZERO),
            };
            (main, equity, free_collateral)
        };

        let arbitrum_value_usd = market_value_usd + asset_value_usd;
        let total_value_usd = arbitrum_value_usd + dydx_main_usdc + dydx_subaccount_equity;

        Ok(PortfolioSnapshot {
            timestamp: Utc::now(),
            market_balances,
            market_values_usd,
            asset_balances,
            asset_values_usd,
            hedge_positions,
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
        portfolio_data: &PortfolioData,
        snapshot: &PortfolioSnapshot,
    ) -> Result<ReserveState> {
        let total_value = snapshot.total_value_usd;
        let mut investable = Decimal::ZERO;
        let mut required_margin = Decimal::ZERO;
        let mut required_equity = Decimal::ZERO;
        let mut required_free_collateral = Decimal::ZERO;
        let mut reserve_total = Decimal::ZERO;

        let iterations = 2;
        for _ in 0..iterations {
            let base_reserve = total_value * self.planner_config.reserve_pct;
            investable = (total_value - base_reserve).max(Decimal::ZERO);
            let reserve_req = self
                .compute_required_dydx_reserve(portfolio_data, snapshot, investable)
                .await?;
            required_margin = reserve_req.0;
            required_equity = reserve_req.1;
            required_free_collateral = reserve_req.2;
            reserve_total = base_reserve.max(required_equity);
            investable = (total_value - reserve_total).max(Decimal::ZERO);
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
        portfolio_data: &PortfolioData,
        snapshot: &PortfolioSnapshot,
        investable_capital: Decimal,
    ) -> Result<(Decimal, Decimal, Decimal)> {
        if investable_capital <= Decimal::ZERO {
            return Ok((Decimal::ZERO, Decimal::ZERO, Decimal::ZERO));
        }

        let token_hedgeinfo_map = {
            let client = self.dydx_client.lock().await;
            client.get_token_hedgeinfo_map().await.unwrap_or_default()
        };

        let mut required_margin = Decimal::ZERO;
        let mut min_leverage = None::<Decimal>;
        let target_weights = compute_target_weights(portfolio_data);
        let base_stable = self.get_preferred_stable_token(&snapshot.asset_balances);

        for market in portfolio_data.market_addresses.iter() {
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

            let hedge_notional = match self
                .estimate_target_hedge_notional_usd(
                    *market,
                    target_value,
                    base_stable.as_ref(),
                    &snapshot.asset_balances,
                )
                .await?
            {
                Some(notional) => notional,
                None => continue,
            };

            if leverage > Decimal::ZERO {
                required_margin += hedge_notional / leverage;
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
        base_stable: Option<&TokenInfo>,
        balances: &HashMap<Address, Decimal>,
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

        let deposit_token = match base_stable {
            Some(base_stable) => self
                .select_deposit_token(market, base_stable, balances)
                .await?
                .unwrap_or_else(|| {
                    if hedge_utils::STABLE_COINS.contains(&short_token.symbol.as_str()) {
                        short_token.clone()
                    } else {
                        long_token.clone()
                    }
                }),
            None => {
                if hedge_utils::STABLE_COINS.contains(&short_token.symbol.as_str()) {
                    short_token.clone()
                } else {
                    long_token.clone()
                }
            }
        };

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

    async fn ensure_gas_reserve(
        &self,
        snapshot: &PortfolioSnapshot,
        reserve_state: &ReserveState,
        strategy_run_id: i32,
    ) -> Result<()> {
        let target_eth = reserve_state.gas_reserve_target_eth;
        if target_eth <= Decimal::ZERO {
            return Ok(());
        }

        let current_eth = self.wallet_manager.get_native_balance().await?;
        let native_price = self.wallet_manager.native_token.last_mid_price_usd;
        let threshold_usd = self.planner_config.min_value_usd;

        if current_eth < target_eth {
            let mut needed = target_eth - current_eth;
            let weth_address = WNT_ADDRESS.parse().unwrap();
            let weth_balance = self.wallet_manager.get_token_balance(weth_address).await?;
            if weth_balance > Decimal::ZERO {
                let unwrap_amount = weth_balance.min(needed);
                let action = TradeAction::SpotSwap {
                    from_token: weth_address,
                    to_token: NATIVE_ADDRESS.parse().unwrap(),
                    amount: unwrap_amount,
                    side: "SELL".to_string(),
                };
                let status = self.execute_action(&action).await;
                self.log_trade(strategy_run_id, &action, status).await.ok();
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
                                    warn!(
                                        "Triggered dYdX withdrawal for gas reserve; skipping ETH top-up this cycle"
                                    );
                                    return Ok(());
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
                    let status = self.execute_action(&action).await;
                    self.log_trade(strategy_run_id, &action, status).await.ok();
                }
            }
        } else {
            let excess = current_eth - target_eth;
            let excess_usd = excess * native_price;
            if excess_usd > threshold_usd {
                if let Some(base_stable) = self.get_preferred_stable_token(&snapshot.asset_balances)
                {
                    let action = TradeAction::SpotSwap {
                        from_token: NATIVE_ADDRESS.parse().unwrap(),
                        to_token: base_stable.address,
                        amount: excess,
                        side: "SELL".to_string(),
                    };
                    let status = self.execute_action(&action).await;
                    self.log_trade(strategy_run_id, &action, status).await.ok();
                }
            }
        }

        Ok(())
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
        actions
    }

    async fn execute_deposit_stage(
        &self,
        snapshot: &PortfolioSnapshot,
        deltas: &HashMap<Address, Decimal>,
        strategy_run_id: i32,
    ) -> Result<()> {
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
            return Ok(());
        }

        let base_stable = match self.get_preferred_stable_token(&snapshot.asset_balances) {
            Some(token) => token,
            None => {
                warn!("No stable token available for deposits");
                return Ok(());
            }
        };

        let mut preferred_tokens = HashSet::new();
        for (market, _) in deposit_targets.iter() {
            if let Some(token) = self
                .select_deposit_token(*market, &base_stable, &snapshot.asset_balances)
                .await?
            {
                preferred_tokens.insert(token.address);
            }
        }

        self.cleanup_idle_tokens(
            &snapshot.asset_balances,
            &preferred_tokens,
            &base_stable,
            strategy_run_id,
        )
        .await;

        let mut refreshed = self.build_snapshot().await?;
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

            let deposit_token = match self
                .select_deposit_token(market, &base_stable, &refreshed.asset_balances)
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

            let mut funding_balance = self
                .wallet_manager
                .get_token_balance(deposit_token.address)
                .await?;
            if funding_balance < amount_needed {
                let shortfall = amount_needed - funding_balance;
                if deposit_token.address == WNT_ADDRESS.parse().unwrap() {
                    let native_balance = self.wallet_manager.get_native_balance().await?;
                    if native_balance > Decimal::ZERO {
                        let wrap_amount = native_balance.min(shortfall);
                        let action = TradeAction::SpotSwap {
                            from_token: NATIVE_ADDRESS.parse().unwrap(),
                            to_token: deposit_token.address,
                            amount: wrap_amount,
                            side: "BUY".to_string(),
                        };
                        let status = self.execute_action(&action).await;
                        self.log_trade(strategy_run_id, &action, status).await.ok();
                    }
                }

                funding_balance = self
                    .wallet_manager
                    .get_token_balance(deposit_token.address)
                    .await?;
                if funding_balance < amount_needed {
                    let remaining = amount_needed - funding_balance;
                    if let Some(source_stable) =
                        self.get_stable_source_token(&refreshed.asset_balances, base_stable.address)
                    {
                        let action = TradeAction::SpotSwap {
                            from_token: source_stable.address,
                            to_token: deposit_token.address,
                            amount: remaining,
                            side: "BUY".to_string(),
                        };
                        let status = self.execute_action(&action).await;
                        self.log_trade(strategy_run_id, &action, status).await.ok();
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
            let status = self.execute_action(&action).await;
            self.log_trade(strategy_run_id, &action, status).await.ok();

            refreshed = self.build_snapshot().await?;
        }

        self.cleanup_idle_tokens(
            &refreshed.asset_balances,
            &HashSet::new(),
            &base_stable,
            strategy_run_id,
        )
        .await;

        Ok(())
    }

    async fn cleanup_idle_tokens(
        &self,
        balances: &HashMap<Address, Decimal>,
        keep_tokens: &HashSet<Address>,
        base_stable: &TokenInfo,
        strategy_run_id: i32,
    ) {
        for (token_addr, balance) in balances.iter() {
            if *balance <= Decimal::ZERO {
                continue;
            }
            if *token_addr == base_stable.address {
                continue;
            }
            if hedge_utils::STABLE_COINS.contains(
                &self
                    .wallet_manager
                    .asset_tokens
                    .get(token_addr)
                    .map(|t| t.symbol.as_str())
                    .unwrap_or(""),
            ) {
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
            if (*balance * token_price) < self.planner_config.min_value_usd {
                continue;
            }

            let action = TradeAction::SpotSwap {
                from_token: *token_addr,
                to_token: base_stable.address,
                amount: *balance,
                side: "SELL".to_string(),
            };
            let status = self.execute_action(&action).await;
            self.log_trade(strategy_run_id, &action, status).await.ok();
        }
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

        let prefer_short = if hedge_utils::STABLE_COINS.contains(&short_token.symbol.as_str()) {
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

    async fn build_hedge_actions(
        &self,
        _portfolio_data: &PortfolioData,
        snapshot: &PortfolioSnapshot,
        target_weights: &HashMap<Address, Decimal>,
    ) -> Result<Vec<TradeAction>> {
        let mut actions = Vec::new();
        let current_positions = {
            let client = self.dydx_client.lock().await;
            client
                .get_dydx_subaccount_perp_positions()
                .await
                .unwrap_or_default()
        };

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
            let current_size = current_positions
                .get(&ticker)
                .cloned()
                .unwrap_or(Decimal::ZERO);
            let delta = target_size - current_size;

            if delta.abs() * long_token.last_mid_price_usd < self.planner_config.min_value_usd {
                continue;
            }

            actions.push(TradeAction::HedgeOrder {
                token_symbol: long_token.symbol.clone(),
                size: delta.abs(),
                side_is_buy: delta > Decimal::ZERO,
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
    ) -> Result<()> {
        let mut client = self.dydx_client.lock().await;
        let sub_equity = snapshot.dydx_subaccount_equity;
        let sub_free = snapshot.dydx_free_collateral;
        let required_equity = reserve_state.required_equity;
        let required_free = reserve_state.required_free_collateral;

        let equity_shortfall = (required_equity - sub_equity).max(Decimal::ZERO);
        let free_shortfall = (required_free - sub_free).max(Decimal::ZERO);
        let shortfall = equity_shortfall.max(free_shortfall);

        if shortfall <= self.planner_config.min_value_usd {
            return Ok(());
        }

        let main_usdc = snapshot.dydx_main_usdc;
        if main_usdc >= shortfall {
            if let Err(e) = client.deposit_to_subaccount(shortfall).await {
                warn!(error = ?e, "Failed to move USDC to subaccount");
            } else {
                info!(amount = %shortfall, "Moved USDC to dYdX subaccount");
            }
            return Ok(());
        }

        if main_usdc > Decimal::ZERO {
            if let Err(e) = client.deposit_to_subaccount(main_usdc).await {
                warn!(error = ?e, "Failed to move USDC to subaccount");
                return Ok(());
            }
        }

        let remaining = shortfall - main_usdc;
        if remaining > self.planner_config.min_value_usd {
            info!(amount = %remaining, "Funding dYdX reserve via SkipGo");
            if let Err(e) = client.dydx_deposit(Some(remaining), None, true, None).await {
                warn!(error = ?e, "Failed to deposit to dYdX");
            }
        }

        Ok(())
    }

    async fn execute_actions(&self, actions: &[TradeAction], strategy_run_id: i32) {
        if actions.is_empty() {
            return;
        }
        info!(action_count = actions.len(), "Executing actions");
        for action in actions.iter() {
            let status = self.execute_action(action).await;
            if let Err(e) = self.log_trade(strategy_run_id, action, status).await {
                warn!(error = ?e, action = ?action, "Failed to log trade");
            }
        }
    }

    async fn execute_action(&self, action: &TradeAction) -> TradeStatus {
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
            } => {
                let mut client = self.dydx_client.lock().await;
                client
                    .submit_perp_order(token_symbol, *size, *side_is_buy)
                    .await
            }
        };

        match result {
            Ok(_) => TradeStatus::Executed,
            Err(e) => {
                error!(error = ?e, action = ?action, "Execution failed");
                TradeStatus::Failed
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

    async fn record_snapshot(&self, snapshot: &PortfolioSnapshot) -> Result<()> {
        let prev_snapshot = self.db_manager.get_latest_portfolio_snapshot().await?;
        let pnl = match prev_snapshot {
            Some(prev) => snapshot.total_value_usd - prev.total_value_usd.unwrap_or(Decimal::ZERO),
            None => Decimal::ZERO,
        };

        let new_snapshot = NewPortfolioSnapshotModel {
            timestamp: snapshot.timestamp,
            total_value_usd: snapshot.total_value_usd,
            market_value_usd: snapshot.market_value_usd,
            asset_value_usd: snapshot.asset_value_usd,
            hedge_value_usd: Decimal::ZERO,
            pnl_usd: pnl,
        };
        let snapshot_id = self
            .db_manager
            .insert_portfolio_snapshot(&new_snapshot)
            .await?;

        let positions = self.build_position_snapshots(snapshot, snapshot_id).await?;
        if !positions.is_empty() {
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
            let usd_value = snapshot.market_values_usd.get(market).cloned();
            positions.push(NewPositionSnapshotModel {
                portfolio_snapshot_id,
                position_type: "gm_market".to_string(),
                market_id,
                token_id: None,
                symbol: None,
                size: Some(*balance),
                usd_value,
            });
        }

        for (token, balance) in snapshot.asset_balances.iter() {
            if *balance <= Decimal::ZERO {
                continue;
            }
            let token_id = self.db_manager.token_id_map.get(token).cloned();
            let usd_value = snapshot.asset_values_usd.get(token).cloned();
            positions.push(NewPositionSnapshotModel {
                portfolio_snapshot_id,
                position_type: "token_balance".to_string(),
                market_id: None,
                token_id,
                symbol: None,
                size: Some(*balance),
                usd_value,
            });
        }

        let native_balance = self
            .wallet_manager
            .get_native_balance()
            .await
            .unwrap_or(Decimal::ZERO);
        if native_balance > Decimal::ZERO {
            let native_price = self.wallet_manager.native_token.last_mid_price_usd;
            let usd_value = if native_price > Decimal::ZERO {
                Some(native_balance * native_price)
            } else {
                None
            };
            positions.push(NewPositionSnapshotModel {
                portfolio_snapshot_id,
                position_type: "native_balance".to_string(),
                market_id: None,
                token_id: None,
                symbol: Some("ETH".to_string()),
                size: Some(native_balance),
                usd_value,
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
        strategy_run_id: i32,
        action: &TradeAction,
        status: TradeStatus,
    ) -> Result<()> {
        let (market_id, from_token_id, to_token_id, amount_in, amount_out, usd_value, details) =
            self.build_trade_details(action);

        let trade = NewTradeModel {
            timestamp: Utc::now(),
            action_type: action.action_type().to_string(),
            strategy_run_id: Some(strategy_run_id),
            market_id,
            from_token_id,
            to_token_id,
            amount_in,
            amount_out,
            usd_value,
            fee_usd: None,
            tx_hash: None,
            status: status.as_str().to_string(),
            details,
        };
        self.db_manager.insert_trade(&trade).await?;
        Ok(())
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
            } => {
                let details = serde_json::json!({
                    "token_symbol": token_symbol,
                    "side_is_buy": side_is_buy,
                })
                .to_string();
                (None, None, None, Some(*size), None, None, Some(details))
            }
        }
    }
}
