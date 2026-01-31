use std::collections::HashMap;
use std::sync::Arc;
use chrono::Utc;
use eyre::Result;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;
use tracing::{debug, error, info, warn};

use crate::config::Config;
use crate::db::db_manager::DbManager;
use crate::db::models::portfolio_snapshots::NewPortfolioSnapshotModel;
use crate::db::models::strategy_runs::NewStrategyRunModel;
use crate::db::models::strategy_targets::NewStrategyTargetModel;
use crate::db::models::trades::NewTradeModel;
use crate::gm_token_txs::gm_tx_manager::GmTxManager;
use crate::gm_token_txs::types::{GmDepositRequest, GmShiftRequest, GmTxRequest, GmWithdrawalRequest};
use crate::hedging::dydx_client::DydxClient;
use crate::hedging::hedge_utils;
use crate::spot_swap::swap_manager::SwapManager;
use crate::spot_swap::types::SwapRequest;
use crate::strategy::types::PortfolioData;
use crate::wallet::WalletManager;

use super::planner::build_rebalance_plan;
use super::types::{ExecutionMode, PlannerConfig, PortfolioSnapshot, TradeAction, TradeStatus};

pub struct ExecutionEngine {
    config: Arc<Config>,
    db_manager: Arc<DbManager>,
    wallet_manager: Arc<WalletManager>,
    gm_tx_manager: GmTxManager,
    swap_manager: SwapManager,
    dydx_client: Arc<tokio::sync::Mutex<DydxClient>>,
    mode: ExecutionMode,
    planner_config: PlannerConfig,
}

impl ExecutionEngine {
    pub fn new(
        config: Arc<Config>,
        db_manager: Arc<DbManager>,
        wallet_manager: Arc<WalletManager>,
        dydx_client: Arc<tokio::sync::Mutex<DydxClient>>,
        mode: ExecutionMode,
    ) -> Self {
        let gm_tx_manager = GmTxManager::new(config.clone(), wallet_manager.clone(), db_manager.clone());
        let swap_manager = SwapManager::new(&config, wallet_manager.clone());
        Self {
            config,
            db_manager,
            wallet_manager,
            gm_tx_manager,
            swap_manager,
            dydx_client,
            mode,
            planner_config: PlannerConfig::default(),
        }
    }

    pub async fn run_once(&self, portfolio_data: &PortfolioData) -> Result<()> {
        let snapshot = self.build_snapshot().await?;
        let strategy_run_id = self.record_strategy_run(portfolio_data).await?;

        let plan = build_rebalance_plan(
            portfolio_data,
            &snapshot,
            &self.wallet_manager,
            &self.planner_config,
        )?;

        if !plan.notes.is_empty() {
            for note in plan.notes.iter() {
                warn!(note = %note, "Rebalance planner note");
            }
        }

        if plan.actions.is_empty() {
            info!("No rebalance actions required");
        } else {
            info!(action_count = plan.actions.len(), "Executing rebalance actions");
        }

        for action in plan.actions.iter() {
            let status = self.execute_action(action).await;
            if let Err(e) = self.log_trade(strategy_run_id, action, status).await {
                warn!(error = ?e, action = ?action, "Failed to log trade");
            }
        }

        // Hedge adjustments
        let hedge_actions = self.build_hedge_actions(portfolio_data, &snapshot).await?;
        for action in hedge_actions.iter() {
            let status = self.execute_action(action).await;
            if let Err(e) = self.log_trade(strategy_run_id, action, status).await {
                warn!(error = ?e, action = ?action, "Failed to log hedge trade");
            }
        }

        let post_snapshot = self.build_snapshot().await?;
        self.record_snapshot(&post_snapshot).await?;

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

        let hedge_positions = match self.mode {
            ExecutionMode::Live => {
                let client = self.dydx_client.lock().await;
                client.get_open_perp_positions().await.unwrap_or_default()
            }
            ExecutionMode::Paper => HashMap::new(),
        };

        let total_value_usd = market_value_usd + asset_value_usd;

        Ok(PortfolioSnapshot {
            timestamp: Utc::now(),
            market_balances,
            market_values_usd,
            asset_balances,
            asset_values_usd,
            hedge_positions,
            total_value_usd,
            market_value_usd,
            asset_value_usd,
        })
    }

    async fn execute_action(&self, action: &TradeAction) -> TradeStatus {
        match self.mode {
            ExecutionMode::Paper => {
                debug!(action = ?action, "Paper mode: simulated action");
                TradeStatus::Simulated
            }
            ExecutionMode::Live => {
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
        }
    }

    async fn record_strategy_run(&self, portfolio_data: &PortfolioData) -> Result<i32> {
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
            timestamp: Utc::now(),
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
        let prev_snapshot = self.db_manager.get_latest_portfolio_snapshot(self.mode.as_str()).await?;
        let pnl = match prev_snapshot {
            Some(prev) => snapshot.total_value_usd - prev.total_value_usd.unwrap_or(Decimal::ZERO),
            None => Decimal::ZERO,
        };

        let new_snapshot = NewPortfolioSnapshotModel {
            timestamp: snapshot.timestamp,
            mode: self.mode.as_str().to_string(),
            total_value_usd: snapshot.total_value_usd,
            market_value_usd: snapshot.market_value_usd,
            asset_value_usd: snapshot.asset_value_usd,
            hedge_value_usd: Decimal::ZERO,
            pnl_usd: pnl,
        };
        self.db_manager.insert_portfolio_snapshot(&new_snapshot).await?;
        Ok(())
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
            mode: self.mode.as_str().to_string(),
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
    ) -> (Option<i32>, Option<i32>, Option<i32>, Option<Decimal>, Option<Decimal>, Option<Decimal>, Option<String>) {
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
                (market_id, None, None, Some(*amount), None, Some(usd_value), Some(details))
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
                (market_id, None, None, Some(*amount), None, Some(usd_value), Some(details))
            }
            TradeAction::SpotSwap {
                from_token,
                to_token,
                amount,
                side,
            } => {
                let from_token_id = self.db_manager.token_id_map.get(from_token).cloned();
                let to_token_id = self.db_manager.token_id_map.get(to_token).cloned();
                let usd_value = if side == "SELL" {
                    self.wallet_manager
                        .asset_tokens
                        .get(from_token)
                        .map(|t| t.last_mid_price_usd * *amount)
                        .unwrap_or(Decimal::ZERO)
                } else {
                    self.wallet_manager
                        .asset_tokens
                        .get(to_token)
                        .map(|t| t.last_mid_price_usd * *amount)
                        .unwrap_or(Decimal::ZERO)
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

    async fn build_hedge_actions(
        &self,
        portfolio_data: &PortfolioData,
        snapshot: &PortfolioSnapshot,
    ) -> Result<Vec<TradeAction>> {
        if self.mode == ExecutionMode::Paper {
            return Ok(Vec::new());
        }

        let mut actions = Vec::new();
        let total_value_usd = snapshot.total_value_usd;
        if total_value_usd <= Decimal::ZERO {
            return Ok(actions);
        }

        let weight_sum = portfolio_data.weights.sum();
        let weight_denominator = if weight_sum > Decimal::ZERO { weight_sum } else { Decimal::ONE };

        let current_positions = {
            let client = self.dydx_client.lock().await;
            client.get_open_perp_positions().await.unwrap_or_default()
        };

        for (i, market) in portfolio_data.market_addresses.iter().enumerate() {
            let market_info = match self.wallet_manager.market_tokens.get(market) {
                Some(info) => info,
                None => continue,
            };

            let long_token = match self.wallet_manager.asset_tokens.get(&market_info.long_token_address) {
                Some(token) => token,
                None => continue,
            };
            let short_token = match self.wallet_manager.asset_tokens.get(&market_info.short_token_address) {
                Some(token) => token,
                None => continue,
            };

            let exposed_capital_frac = if hedge_utils::STABLE_COINS.contains(&short_token.symbol.as_str()) {
                Decimal::ONE
            } else {
                Decimal::from_f64(0.5).unwrap()
            };

            if hedge_utils::STABLE_COINS.contains(&long_token.symbol.as_str()) {
                continue;
            }

            let target_weight = portfolio_data.weights[i] / weight_denominator;
            let target_value_usd = total_value_usd * target_weight;
            let hedge_notional_usd = target_value_usd * exposed_capital_frac;

            if long_token.last_mid_price_usd <= Decimal::ZERO {
                continue;
            }

            let target_size = -(hedge_notional_usd / long_token.last_mid_price_usd); // short
            let ticker = hedge_utils::get_dydx_perp_ticker(&long_token.symbol);
            let current_size = current_positions.get(&ticker).cloned().unwrap_or(Decimal::ZERO);
            let delta = target_size - current_size;

            if delta.abs() < Decimal::from_f64(0.01).unwrap() {
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
}
