# Execution Workflow

The execution layer takes persisted strategy targets, compares them to live wallet and dYdX state, and moves the live portfolio toward the desired allocation while:

- keeping capital available for hedge margin on dYdX
- keeping enough native ETH for Arbitrum gas
- minimizing unnecessary GM withdraw/deposit churn by using direct shifts when possible
- sizing dYdX hedges from quote-derived GM collateral exposure instead of static heuristics
- supporting a durable operator-controlled `stable_only` mode that unwinds risk and stays flat

The core code lives in:

- `crypto-yield-farming-bot/src/execution/engine.rs`
- `crypto-yield-farming-bot/src/execution/planner.rs`
- `crypto-yield-farming-bot/src/execution/types.rs`

The runtime entrypoints live in:

- `crypto-yield-farming-bot/src/bin/executor.rs`
- `crypto-yield-farming-bot/src/bin/executor_ctl.rs`

The durable execution control and transfer state live in:

- `crypto-yield-farming-bot/src/db/schema/execution_control_state.sql`
- `crypto-yield-farming-bot/src/db/schema/execution_control_events.sql`
- `crypto-yield-farming-bot/src/db/schema/execution_transfer_state.sql`

## Runtime Modes

The executor runs in one of two durable modes:

1. `normal`
   - the executor consumes fresh strategy runs and calls `run_once_with_existing_strategy_run(...)`
   - the system is allowed to enter, exit, and rebalance portfolio positions

2. `stable_only`
   - the executor ignores strategy deployment
   - the system repeatedly calls `run_unwind_to_stable(...)`
   - the goal is to converge to the target stablecoin plus a small native ETH gas buffer

The mode is stored in Postgres and changed by `executor_ctl`.

Redis pubsub is only a wake-up mechanism. Postgres is the source of truth.

## Control Plane

`executor_ctl` is the operator-facing binary for changing execution mode.

Supported commands:

- `status`
- `stay-stable`
- `resume-normal`

At a high level:

- `stay-stable` writes `stable_only` to Postgres, records an audit event, and publishes an `execution_control` Redis wake-up
- `resume-normal` writes `normal`, records an audit event, computes `min_strategy_run_id_to_execute`, and publishes the same wake-up
- `status` prints the current durable control state

### Resume Gate

When the system returns from `stable_only` to `normal`, it does not immediately consume the most recent already-produced strategy run.

Instead:

1. `executor_ctl resume-normal` reads the latest strategy-run id
2. it writes `min_strategy_run_id_to_execute = latest_run_id + 1`
3. the executor ignores older runs
4. the next fresh strategy run is the first one eligible for deployment

This prevents a manual unwind from being immediately reversed by a stale strategy output.

## Core State Types

The execution system operates on four layers of state:

1. Strategy targets
   - represented by `ExecutionTargets` in the long-running executor path
   - `run_once(...)` can still accept full `PortfolioData`, but that is only a convenience wrapper that persists the run and then converts to `ExecutionTargets`

2. Live portfolio state
   - represented by `PortfolioSnapshot`
   - built from Arbitrum wallet balances plus dYdX balances and perp positions

3. Concrete action plan
   - represented by `TradeAction`
   - includes:
     - `GmDeposit`
     - `GmWithdrawal`
     - `GmShift`
     - `SpotSwap`
     - `HedgeOrder`

4. Durable operator/runtime state
   - represented in Postgres by `execution_control_state`
   - plus singleton `execution_transfer_state` for cross-chain dYdX transfer tracking

## Snapshot Model

Every execution pass starts from a fresh `PortfolioSnapshot`.

The snapshot contains:

- GM balances by market
- GM USD values by market
- asset-token balances on Arbitrum
- asset-token USD values on Arbitrum
- native ETH balance and USD value
- dYdX perp positions
- dYdX main-account USDC
- dYdX subaccount equity
- dYdX free collateral
- aggregate USD totals

The aggregate totals are defined as:

- `native_value_usd = native_balance * ETH_price`
- `arbitrum_value_usd = market_value_usd + asset_value_usd + native_value_usd`
- `total_value_usd = arbitrum_value_usd + dydx_main_usdc + dydx_subaccount_equity`

## Durable dYdX Transfer Tracking

Cross-chain dYdX transfers are asynchronous because they use SkipGo.

The executor therefore persists any in-flight cross-chain transfer in `execution_transfer_state` with:

- direction
- tx hash
- chain id
- amount
- expected completion time
- initiation timestamp

On each normal execution wake-up, the engine checks the stored transfer status through `DydxClient::check_skipgo_transfer_status(...)`.

If a transfer exists and is:

- `completed`
  - the durable transfer state is cleared
- `failed`
  - the durable transfer state is cleared
- `pending`
  - the cycle continues, but the pending transfer is treated as unavailable capital
  - the engine will not start a second cross-chain dYdX capital transfer while one is already pending

`stable_only` unwind does not block on pending SkipGo transfers. A pending top-up or withdrawal may settle during stable-only mode, and that is acceptable because it only changes where idle stablecoin capital sits.

## High-Level Normal Execution Flow

When the executor is in `normal` mode and receives an eligible strategy run, the cycle is:

1. refresh pending dYdX transfer status
2. build a live snapshot
3. load dYdX hedge metadata
4. run dYdX capital sync preflight
5. ensure gas reserve
6. compute target market values and deltas
7. execute GM shifts
8. execute GM withdrawals
9. execute deposit stage
10. execute dYdX hedge adjustments
11. wait for dYdX hedge polling tasks to converge
12. record final portfolio and position snapshots

Each stage is described below.

## 1. Strategy Targets Enter Execution

The long-running executor binary loads the exact persisted `strategy_run_id` and the exact target rows for that run from Postgres. It does not rely on “latest run” behavior and does not need to reconstruct a full covariance matrix.

The planner normalizes raw target weights before execution:

- the target weights are divided by their sum
- if the sum is non-positive, the denominator is treated as `1` to avoid division by zero

Execution therefore always works from a well-defined normalized weight map.

## 2. Capital Base and Reserve Sizing

Before allocating capital across GM markets, the engine computes:

- a generic portfolio reserve
- required dYdX margin/equity/free collateral
- a native ETH gas reserve target

### 2.1 Capital Base

The engine does not size deployment from raw `total_value_usd`.

It first computes a deployment capital base that excludes:

- native ETH value
- an Arbitrum stable buffer

The Arbitrum stable buffer is:

- `max(arbitrum_stable_buffer_floor_usd, arbitrum_stable_buffer_pct * arbitrum_non_native_value)`

where `arbitrum_non_native_value = arbitrum_value_usd - native_value_usd`.

This buffer is intended to leave a small amount of stable liquidity on Arbitrum outside the deployable pool.

### 2.2 Generic Reserve

The planner config contains `reserve_pct`.

The generic reserve is:

- `base_reserve = capital_base * reserve_pct`

This is a broad capital haircut that keeps some capital undeployed even before dYdX-specific hedge requirements are considered.

### 2.3 Quote-Derived dYdX Margin Requirement

For each target market, the engine estimates hedge notional from actual GM quote behavior instead of from a fixed percentage rule.

For a target market:

1. choose a deposit token
2. simulate a GM deposit for the target USD size
3. simulate a full GM withdrawal of the resulting GM amount
4. read `long_amount_out` and `short_amount_out`
5. convert those outputs to USD
6. map them to hedge notional

If the short collateral is stable, hedge notional is the long-side USD value.

If the short collateral is non-stable, hedge notional is the sum of long-side and short-side USD values.

Required raw margin is then the sum of:

- `hedge_notional / usable_leverage`

across the hedged markets.

### 2.4 Low-Leverage Guard

The engine also tracks the smallest usable leverage across the hedged markets and applies a low-leverage guard so that reserve sizing does not become too optimistic when the target book may rotate into lower-leverage collateral types.

The final required margin is the larger of:

- the raw summed margin requirement
- the guard-adjusted requirement

### 2.5 Required Free Collateral and Equity

Once required margin is determined, the engine computes:

- `target_free_collateral_without_buffer = 50% of required_margin`
- `free_collateral_buffer = max(dydx_free_collateral_buffer_floor_usd, dydx_free_collateral_buffer_pct * target_free_collateral_without_buffer)`
- `required_free_collateral = target_free_collateral_without_buffer + free_collateral_buffer`
- `required_equity = required_margin + required_free_collateral`

The dYdX percentage buffer is intentionally based on target free collateral rather than on total subaccount equity.

### 2.6 Upper dYdX Capital Bound

The engine also computes an upper bound for dYdX capital so it can pull excess back to Arbitrum when dYdX is materially overfunded.

It does this by adding one more free-collateral-scaled buffer on top of the required free collateral:

- `upper_free_collateral = required_free_collateral + compute_dydx_free_collateral_buffer_usd(required_free_collateral)`
- `upper_equity = required_margin + upper_free_collateral`

### 2.7 Gas Reserve

Separately, the engine computes a native gas target:

- `gas_reserve_target_usd = arbitrum_value_usd * gas_reserve_pct`
- `gas_reserve_target_eth = gas_reserve_target_usd / ETH_price`

This is independent from the stable buffer and from the dYdX free-collateral buffer.

## 3. dYdX Capital Sync Preflight

Before the engine starts GM rebalancing, it runs a dYdX capital preflight:

1. compute the ideal reserve state from the current snapshot
2. if dYdX main-account USDC already exists and the subaccount is short on equity/free collateral, move main-account USDC into the subaccount first
3. evaluate whether a cross-chain dYdX transfer is needed
4. if dYdX is short:
   - current-cycle investable capital is scaled down to what the current dYdX equity/free collateral can support
   - an async Arbitrum -> dYdX top-up may still be initiated for a future cycle
5. if dYdX is materially overfunded:
   - an async dYdX -> Arbitrum withdrawal may be initiated
6. if a cross-chain transfer is already pending:
   - no second cross-chain dYdX transfer is started

### 3.1 Current-Run Investable Capital

For the current run, deployment is limited by:

- current Arbitrum deployable capital
- current dYdX equity ratio
- current dYdX free-collateral ratio

In other words, the engine does not assume a newly initiated SkipGo transfer will arrive in time for the current iteration.

If dYdX is underfunded, the engine scales the current run down and separately prepares the future run by sending the missing USDC.

### 3.2 Cross-Chain Top-Ups and Withdrawals

If dYdX is short and no transfer is already pending:

- the engine computes how much Arbitrum USDC is available after keeping the Arbitrum stable buffer
- it sends at most that amount through `dydx_deposit(...)`
- it persists the resulting SkipGo tracking information

If dYdX is overfunded and no transfer is already pending:

- the engine computes how much capital sits above `upper_equity`
- it first uses dYdX main-account USDC
- if needed, it also moves withdrawable excess from the dYdX subaccount into the main account
- it sends the resulting amount through `dydx_withdrawal(...)`
- it persists the resulting SkipGo tracking information

## 4. Gas Reserve Management

The engine ensures the wallet has enough native ETH to continue transacting.

If native ETH is below target:

1. unwrap WETH first
2. if still short, buy native ETH from stables
3. if even stable balances are insufficient, and dYdX has excess above the required reserve, the engine may initiate a dYdX withdrawal to replenish gas

If native ETH is above target by more than the minimum USD threshold:

- the excess is sold into the preferred stablecoin

Any dYdX withdrawal initiated for gas reserve purposes is also persisted in `execution_transfer_state`.

## 5. Target Values and Market Deltas

Once capital sync and gas reserve are handled, the engine computes:

- `target_market_value = normalized_weight * investable_capital`

for each target market.

It then compares current GM exposure to target GM exposure and converts the difference to GM token quantity deltas using current GM mid prices.

A delta is ignored only if both:

- the absolute USD delta is below `min_value_usd`
- the absolute weight delta is below `min_weight_delta`

This prevents noise-sized churn while still allowing meaningful rebalances.

## 6. GM Shift Stage

Before withdrawals and deposits, the engine tries to directly shift GM inventory between markets that share the same collateral pair.

Within each collateral pair:

- sellers are markets with negative delta
- buyers are markets with positive delta

The planner greedily matches seller surplus against buyer demand and emits `GmShift` actions.

This is cheaper and cleaner than doing a withdrawal plus a new deposit when the collateral pair is already the same.

## 7. GM Withdrawal Stage

After shifts, remaining oversized markets are reduced through direct GM withdrawals.

For each negative delta, the engine withdraws up to the smaller of:

- the requested reduction
- the current GM balance

Withdrawals happen before deposits so that capital is freed first and the deposit stage can work from a cleaner inventory.

## 8. Deposit Stage

The deposit stage has three responsibilities:

1. choose the best deposit token per market
2. clean up unused inventory
3. source funding and submit GM deposits

### 8.1 Preferred Stablecoin

The engine picks a preferred stable denomination with this priority:

1. `USDC`
2. `USDC.e`
3. otherwise the stablecoin with the highest live USD balance

### 8.2 Deposit-Token Selection

For each market, the engine estimates the all-in cost of depositing via the long token vs the short token.

The estimated cost is:

- GM deposit fee estimate
- plus swap fee estimate if the wallet does not already hold enough of that token

If one side is clearly cheaper, the engine chooses it.

If the difference is small, the engine falls back to the operationally cleaner stable-side preference when applicable.

### 8.3 Inventory Cleanup

Before deposits, the engine liquidates non-stable tokens that are not useful for pending deposits.

It keeps:

- the preferred stable token
- all stablecoins
- any token selected as a preferred deposit token for some pending target market

Other material non-stable balances are sold back into the preferred stable.

### 8.4 Funding and Depositing

For each positive market delta:

1. compute the USD amount needed
2. convert that to the selected deposit-token amount
3. inspect the live current balance of that deposit token
4. if short:
   - wrap native ETH into WETH if the funding token is WETH, but only from ETH above the gas reserve target
   - otherwise swap a stable source token into the needed deposit token
5. submit a GM deposit

The engine clips the final deposit amount to the actually available balance, so a cycle can still make partial progress even if the exact intended amount could not be sourced.

### 8.5 Final Cleanup

After the deposit loop, the engine performs one more cleanup pass and liquidates remaining non-stable idle inventory back into the preferred stable token, subject to the minimum-value threshold.

## 9. Hedge Adjustment Stage

The hedge stage runs after the GM-side allocation is in place.

For each deployed GM market that should be hedged:

1. simulate a full GM withdrawal of the current GM balance
2. estimate the hedge notional from the withdrawal outputs
3. convert that notional into the ideal dYdX perp size for the long collateral token

The important implementation detail is that the engine aggregates these ideal hedge sizes across all deployed GM markets that map to the same dYdX perp ticker.

So if multiple GM markets all contribute `ETH` long-side exposure, the engine:

- sums the ideal `ETH-USD` target position across all of them
- compares that single net target to the current `ETH-USD` position on dYdX
- emits one net `HedgeOrder` for `ETH`

This avoids submitting separate dYdX orders for each GM market when the hedge venue only cares about the final net perp position.

### Reduce-Only vs Normal Hedge Orders

Normal rebalance hedges are emitted with `reduce_only = false`.

Stable-only hedge closures are emitted with `reduce_only = true` and are executed through the dYdX reduction path.

After hedge orders are submitted, the engine waits for active dYdX perp polling tasks to converge before recording the final normal-mode snapshot.

## 10. Stable-Only Unwind Flow

When the executor is in `stable_only`, it runs `run_unwind_to_stable(...)` instead of the normal rebalance path.

The unwind pass is:

1. build a fresh live snapshot
2. resolve the target stable token, typically `USDC`
3. withdraw all GM positions above the unwind threshold
4. close all dYdX perp positions using reduce-only hedge orders
5. wait for dYdX hedge polling to converge
6. swap all material ERC-20 balances into the target stable token
7. sell excess native ETH into the target stable token while preserving the explicit stable-only native buffer
8. record a portfolio snapshot and position snapshots

The unwind ERC-20 threshold is separate from the normal trading threshold. This allows stable-only mode to clear smaller residual balances than the normal rebalance path would bother touching.

### Stable-Only Retry Loop

The executor runs a periodic unwind retry loop while in `stable_only`.

At startup:

- if the persisted mode is already `stable_only`, the executor runs an unwind pass immediately

During runtime:

- the executor reacts to `execution_control` wake-up signals
- the executor also runs a periodic stable-only tick, currently every `180` seconds

The periodic retry exists because a single unwind pass may not fully flatten the portfolio:

- a venue action may fail and succeed later
- balances may settle over multiple passes
- a residual token may only become swappable on a later pass

The unwind pass is awaited inline, so the executor does not run multiple unwind passes concurrently.

## Persistence and Logging

After each execution or unwind pass, the engine persists:

- a portfolio snapshot
- per-position snapshots
- trade/action logs

Portfolio snapshots include:

- `strategy_run_id` for normal execution cycles
- Arbitrum GM / asset / native values
- dYdX main-account USDC
- dYdX subaccount equity
- dYdX free collateral
- total portfolio value

Trade logs are action-oriented execution records. They include:

- action type
- status
- tx hash when available
- related market / token ids
- descriptive details

The engine also emits structured `INFO` logs for major workflow stages and `DEBUG` logs for intermediate calculations so a full execution pass can be traced from snapshot creation through final persistence.
