use eyre::Result;
use std::collections::HashMap;
use ethers::types::Address;
use rust_decimal::Decimal;
use rust_decimal::prelude::*;

use crate::hedging::hedge_utils;
use crate::strategy::types::PortfolioData;
use crate::wallet::WalletManager;

use super::types::{PlannerConfig, PortfolioSnapshot, RebalancePlan, TradeAction};

pub fn build_rebalance_plan(
    portfolio_data: &PortfolioData,
    snapshot: &PortfolioSnapshot,
    wallet_manager: &WalletManager,
    config: &PlannerConfig,
) -> Result<RebalancePlan> {
    let mut notes = Vec::new();
    let mut actions = Vec::new();
    let mut target_weights = HashMap::new();

    let total_value_usd = snapshot.total_value_usd;
    if total_value_usd <= Decimal::ZERO {
        return Ok(RebalancePlan {
            actions,
            target_weights,
            notes: vec!["Total portfolio value is zero; skipping rebalancing".to_string()],
        });
    }

    let weight_sum = portfolio_data.weights.sum();
    let weight_denominator = if weight_sum > Decimal::ZERO { weight_sum } else { Decimal::ONE };

    let mut market_deltas_tokens: HashMap<Address, Decimal> = HashMap::new();

    for (i, market) in portfolio_data.market_addresses.iter().enumerate() {
        let target_weight = portfolio_data.weights[i] / weight_denominator;
        target_weights.insert(*market, target_weight);

        let target_value = total_value_usd * target_weight;
        let current_value = snapshot
            .market_values_usd
            .get(market)
            .cloned()
            .unwrap_or(Decimal::ZERO);
        let current_weight = current_value / total_value_usd;
        let delta_weight = target_weight - current_weight;
        let delta_value = target_value - current_value;

        if delta_weight.abs() < config.min_weight_delta && delta_value.abs() < config.min_value_usd {
            continue;
        }

        let market_info = match wallet_manager.market_tokens.get(market) {
            Some(info) => info,
            None => {
                notes.push(format!("Market token info not found for {}", market));
                continue;
            }
        };
        if market_info.last_mid_price_usd <= Decimal::ZERO {
            notes.push(format!("Market price unavailable for {}", market));
            continue;
        }

        let delta_tokens = delta_value / market_info.last_mid_price_usd;
        if !delta_tokens.is_zero() {
            market_deltas_tokens.insert(*market, delta_tokens);
        }
    }

    if market_deltas_tokens.is_empty() {
        return Ok(RebalancePlan {
            actions,
            target_weights,
            notes,
        });
    }

    // Build collateral groups for shift optimization
    let mut market_collateral_map: HashMap<Address, (Address, Address)> = HashMap::new();
    for (market, info) in wallet_manager.market_tokens.iter() {
        market_collateral_map.insert(*market, (info.long_token_address, info.short_token_address));
    }

    let mut deltas = market_deltas_tokens.clone();

    let mut grouped: HashMap<(Address, Address), Vec<Address>> = HashMap::new();
    for market in deltas.keys() {
        if let Some(collateral) = market_collateral_map.get(market) {
            grouped.entry(*collateral).or_default().push(*market);
        }
    }

    for (collateral, markets) in grouped.iter() {
        let mut sellers: Vec<Address> = markets
            .iter()
            .filter(|m| deltas.get(m).cloned().unwrap_or(Decimal::ZERO) < Decimal::ZERO)
            .cloned()
            .collect();
        let mut buyers: Vec<Address> = markets
            .iter()
            .filter(|m| deltas.get(m).cloned().unwrap_or(Decimal::ZERO) > Decimal::ZERO)
            .cloned()
            .collect();

        if sellers.is_empty() || buyers.is_empty() {
            continue;
        }

        let _ = collateral; // keep for future logging

        let mut i = 0usize;
        let mut j = 0usize;
        while i < sellers.len() && j < buyers.len() {
            let seller = sellers[i];
            let buyer = buyers[j];
            let seller_delta = deltas.get(&seller).cloned().unwrap_or(Decimal::ZERO);
            let buyer_delta = deltas.get(&buyer).cloned().unwrap_or(Decimal::ZERO);

            let available = (-seller_delta).max(Decimal::ZERO);
            let needed = buyer_delta.max(Decimal::ZERO);
            let shift_amount = available.min(needed);

            if shift_amount > Decimal::ZERO {
                actions.push(TradeAction::GmShift {
                    from_market: seller,
                    to_market: buyer,
                    amount: shift_amount,
                });
                deltas.insert(seller, seller_delta + shift_amount);
                deltas.insert(buyer, buyer_delta - shift_amount);
            }

            if deltas.get(&seller).cloned().unwrap_or(Decimal::ZERO) >= Decimal::ZERO {
                i += 1;
            }
            if deltas.get(&buyer).cloned().unwrap_or(Decimal::ZERO) <= Decimal::ZERO {
                j += 1;
            }
        }
    }

    // Available asset balances for deposits/swaps
    let mut available_asset_balances = snapshot.asset_balances.clone();

    // Remaining deltas -> withdrawals and deposits
    for (market, delta) in deltas.iter() {
        if *delta <= Decimal::ZERO {
            actions.push(TradeAction::GmWithdrawal {
                market: *market,
                amount: delta.abs(),
            });
            continue;
        }

        let market_info = match wallet_manager.market_tokens.get(market) {
            Some(info) => info,
            None => {
                notes.push(format!("Market token info not found for {}", market));
                continue;
            }
        };

        let gm_price = market_info.last_mid_price_usd;
        if gm_price <= Decimal::ZERO {
            notes.push(format!("Market price unavailable for {}", market));
            continue;
        }

        let usd_needed = *delta * gm_price;
        let long_token = wallet_manager.asset_tokens.get(&market_info.long_token_address);
        let short_token = wallet_manager.asset_tokens.get(&market_info.short_token_address);

        let (use_short, funding_token) = match (long_token, short_token) {
            (_, Some(short)) if hedge_utils::STABLE_COINS.contains(&short.symbol.as_str()) => (true, short),
            (Some(long), _) if hedge_utils::STABLE_COINS.contains(&long.symbol.as_str()) => (false, long),
            (Some(long), Some(short)) => {
                // Default to the more liquid stable-like choice; otherwise split later
                let use_short = short.last_mid_price_usd >= long.last_mid_price_usd;
                if use_short { (true, short) } else { (false, long) }
            }
            (Some(long), None) => (false, long),
            (None, Some(short)) => (true, short),
            (None, None) => {
                notes.push(format!("No collateral tokens available for market {}", market));
                continue;
            }
        };

        if funding_token.last_mid_price_usd <= Decimal::ZERO {
            notes.push(format!("Price unavailable for funding token {}", funding_token.symbol));
            continue;
        }

        let mut funding_needed = usd_needed / funding_token.last_mid_price_usd;
        let available = available_asset_balances
            .get(&funding_token.address)
            .cloned()
            .unwrap_or(Decimal::ZERO);

        if available < funding_needed {
            let deficit = funding_needed - available;
            if let Some((stable_addr, stable_balance, stable_price)) = find_stable_funding_token(wallet_manager, &available_asset_balances, funding_token.address) {
                let stable_usd = stable_balance * stable_price;
                let max_buy = if funding_token.last_mid_price_usd > Decimal::ZERO {
                    stable_usd / funding_token.last_mid_price_usd
                } else {
                    Decimal::ZERO
                };
                let buy_amount = deficit.min(max_buy);
                if buy_amount > Decimal::ZERO {
                    actions.push(TradeAction::SpotSwap {
                        from_token: stable_addr,
                        to_token: funding_token.address,
                        amount: buy_amount,
                        side: "BUY".to_string(),
                    });
                    // Update balances optimistically
                    let stable_spend_usd = buy_amount * funding_token.last_mid_price_usd;
                    let stable_spend = if stable_price > Decimal::ZERO {
                        stable_spend_usd / stable_price
                    } else {
                        Decimal::ZERO
                    };
                    let new_stable_balance = stable_balance - stable_spend;
                    available_asset_balances.insert(stable_addr, new_stable_balance.max(Decimal::ZERO));
                    let new_funding_balance = available + buy_amount;
                    available_asset_balances.insert(funding_token.address, new_funding_balance);
                }
            }
        }

        let available_post = available_asset_balances
            .get(&funding_token.address)
            .cloned()
            .unwrap_or(Decimal::ZERO);
        funding_needed = funding_needed.min(available_post);
        if funding_needed <= Decimal::ZERO {
            notes.push(format!("Insufficient funding token {} for market {}", funding_token.symbol, market));
            continue;
        }

        available_asset_balances.insert(funding_token.address, available_post - funding_needed);

        let (long_amount, short_amount) = if use_short {
            (Decimal::ZERO, funding_needed)
        } else {
            (funding_needed, Decimal::ZERO)
        };

        actions.push(TradeAction::GmDeposit {
            market: *market,
            long_amount,
            short_amount,
        });
    }

    Ok(RebalancePlan {
        actions,
        target_weights,
        notes,
    })
}

fn find_stable_funding_token(
    wallet_manager: &WalletManager,
    balances: &HashMap<Address, Decimal>,
    exclude_token: Address,
) -> Option<(Address, Decimal, Decimal)> {
    let mut best: Option<(Address, Decimal, Decimal)> = None;
    for token in wallet_manager.asset_tokens.values() {
        if token.address == exclude_token {
            continue;
        }
        if !hedge_utils::STABLE_COINS.contains(&token.symbol.as_str()) {
            continue;
        }
        let balance = balances.get(&token.address).cloned().unwrap_or(Decimal::ZERO);
        if balance <= Decimal::ZERO {
            continue;
        }
        let usd = balance * token.last_mid_price_usd;
        if usd <= Decimal::ZERO {
            continue;
        }
        let is_better = best
            .as_ref()
            .map(|(_, best_balance, best_price)| usd > *best_balance * *best_price)
            .unwrap_or(true);
        if is_better {
            best = Some((token.address, balance, token.last_mid_price_usd));
        }
    }
    best
}
