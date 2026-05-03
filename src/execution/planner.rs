use std::collections::HashMap;

use ethers::types::Address;
use rust_decimal::Decimal;

use crate::wallet::WalletManager;

use super::types::{ExecutionTargets, PlannerConfig, PortfolioSnapshot, TradeAction};

pub fn compute_target_weights(targets: &ExecutionTargets) -> HashMap<Address, Decimal> {
    let mut target_weights = HashMap::new();
    let weight_sum = targets.weights.iter().copied().sum::<Decimal>();
    let weight_denominator = if weight_sum > Decimal::ZERO {
        weight_sum
    } else {
        Decimal::ONE
    };

    for (market, weight) in targets.market_addresses.iter().zip(targets.weights.iter()) {
        let target_weight = *weight / weight_denominator;
        target_weights.insert(*market, target_weight);
    }

    target_weights
}

pub fn compute_target_values(
    target_weights: &HashMap<Address, Decimal>,
    investable_capital_usd: Decimal,
) -> HashMap<Address, Decimal> {
    target_weights
        .iter()
        .map(|(market, weight)| (*market, investable_capital_usd * *weight))
        .collect()
}

pub fn compute_market_deltas(
    snapshot: &PortfolioSnapshot,
    target_values: &HashMap<Address, Decimal>,
    wallet_manager: &WalletManager,
    config: &PlannerConfig,
) -> HashMap<Address, Decimal> {
    let mut deltas = HashMap::new();
    let total_value_usd = snapshot.arbitrum_value_usd;

    for (market, target_value) in target_values.iter() {
        let current_value = snapshot
            .market_values_usd
            .get(market)
            .cloned()
            .unwrap_or(Decimal::ZERO);
        let current_weight = if total_value_usd > Decimal::ZERO {
            current_value / total_value_usd
        } else {
            Decimal::ZERO
        };
        let target_weight = if total_value_usd > Decimal::ZERO {
            target_value / total_value_usd
        } else {
            Decimal::ZERO
        };
        let delta_weight = target_weight - current_weight;
        let delta_value = target_value - current_value;

        if delta_value.abs() < config.min_value_usd && delta_weight.abs() < config.min_weight_delta
        {
            continue;
        }

        let market_info = match wallet_manager.market_tokens.get(market) {
            Some(info) => info,
            None => continue,
        };
        if market_info.last_mid_price_usd <= Decimal::ZERO {
            continue;
        }

        let delta_tokens = delta_value / market_info.last_mid_price_usd;
        if !delta_tokens.is_zero() {
            deltas.insert(*market, delta_tokens);
        }
    }

    deltas
}

pub fn plan_shift_actions(
    deltas: &HashMap<Address, Decimal>,
    wallet_manager: &WalletManager,
) -> (Vec<TradeAction>, HashMap<Address, Decimal>) {
    let mut actions = Vec::new();
    let mut updated = deltas.clone();

    let mut market_collateral_map: HashMap<Address, (Address, Address)> = HashMap::new();
    for (market, info) in wallet_manager.market_tokens.iter() {
        market_collateral_map.insert(*market, (info.long_token_address, info.short_token_address));
    }

    let mut grouped: HashMap<(Address, Address), Vec<Address>> = HashMap::new();
    for market in updated.keys() {
        if let Some(collateral) = market_collateral_map.get(market) {
            grouped.entry(*collateral).or_default().push(*market);
        }
    }

    for markets in grouped.values() {
        let sellers: Vec<Address> = markets
            .iter()
            .filter(|m| updated.get(m).cloned().unwrap_or(Decimal::ZERO) < Decimal::ZERO)
            .cloned()
            .collect();
        let buyers: Vec<Address> = markets
            .iter()
            .filter(|m| updated.get(m).cloned().unwrap_or(Decimal::ZERO) > Decimal::ZERO)
            .cloned()
            .collect();

        if sellers.is_empty() || buyers.is_empty() {
            continue;
        }

        let mut i = 0usize;
        let mut j = 0usize;
        while i < sellers.len() && j < buyers.len() {
            let seller = sellers[i];
            let buyer = buyers[j];
            let seller_delta = updated.get(&seller).cloned().unwrap_or(Decimal::ZERO);
            let buyer_delta = updated.get(&buyer).cloned().unwrap_or(Decimal::ZERO);

            let available = (-seller_delta).max(Decimal::ZERO);
            let needed = buyer_delta.max(Decimal::ZERO);
            let shift_amount = available.min(needed);

            if shift_amount > Decimal::ZERO {
                actions.push(TradeAction::GmShift {
                    from_market: seller,
                    to_market: buyer,
                    amount: shift_amount,
                });
                updated.insert(seller, seller_delta + shift_amount);
                updated.insert(buyer, buyer_delta - shift_amount);
            }

            if updated.get(&seller).cloned().unwrap_or(Decimal::ZERO) >= Decimal::ZERO {
                i += 1;
            }
            if updated.get(&buyer).cloned().unwrap_or(Decimal::ZERO) <= Decimal::ZERO {
                j += 1;
            }
        }
    }

    (actions, updated)
}
