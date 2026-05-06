# Execution Workflow

This document describes the execution module end-to-end: how a strategy output is transformed into concrete GMX, spot-swap, and dYdX hedge actions.

The goal of the execution layer is not to recompute the strategy. Its job is to take an already-produced target portfolio, compare it to the live portfolio state, and move the live system toward the target while:

- preserving a reserve for dYdX hedge margin
- preserving a reserve for native gas
- minimizing unnecessary swaps when GM positions can be shifted directly
- sizing hedges from actual GMX quote-derived collateral exposure

The core code lives in:

- `crypto-yield-farming-bot/src/execution/engine.rs`
- `crypto-yield-farming-bot/src/execution/planner.rs`
- `crypto-yield-farming-bot/src/execution/types.rs`

The execution runtime and control-plane entrypoints live in:

- `crypto-yield-farming-bot/src/bin/executor.rs`
- `crypto-yield-farming-bot/src/bin/executor_ctl.rs`

## Execution Control Plane

The execution system has a durable control plane. The executor checks a database-backed execution mode before deciding whether to deploy capital or remain flat.

There are two execution modes:

1. `normal`
   - The executor consumes fresh strategy runs and calls `run_once_with_existing_strategy_run(...)`.
   - This is the standard live-trading mode.

2. `stable_only`
   - The executor refuses to enter or rebalance into strategy positions.
   - Instead it repeatedly calls `run_unwind_to_stable(...)` until the live portfolio is reduced to the target stablecoin plus a small ETH gas buffer.

The durable state is stored in:

- `crypto-yield-farming-bot/src/db/schema/execution_control_state.sql`
- `crypto-yield-farming-bot/src/db/schema/execution_control_events.sql`

The singleton control-state row stores:

- `mode`
- `target_stable_symbol`
- `min_strategy_run_id_to_execute`
- `updated_at`
- `updated_by`
- `reason`

The append-only audit table stores every operator-issued mode change.

### Why the control state is database-backed

Redis pubsub serves as a wake-up mechanism. Postgres is the source of truth.

That means:

- the desired execution mode survives executor restarts
- the desired execution mode survives missed pubsub messages
- `executor_ctl` can safely change state even if the executor is temporarily offline

### `executor_ctl`

`executor_ctl` is the operator-facing binary for changing execution mode.

Supported commands:

- `status`
- `stay-stable`
- `resume-normal`

At a high level:

- `stay-stable` writes `stable_only` state to Postgres, records an audit event, and publishes an `execution_control` wake-up signal to Redis
- `resume-normal` writes `normal` state to Postgres, records an audit event, computes a resume gate via `min_strategy_run_id_to_execute`, and publishes the same wake-up signal
- `status` prints the current durable control-state values

### Resume gate semantics

When the system returns from `stable_only` to `normal`, it does **not** immediately reuse the most recent already-produced strategy run.

Instead:

1. `executor_ctl resume-normal` reads the latest strategy-run id in the database
2. it writes:

$$
\text{min\\_strategy\\_run\\_id\\_to\\_execute} = \text{latest\\_run\\_id} + 1
$$

3. the executor ignores all earlier strategy runs
4. the executor waits for the next fresh strategy run before deploying capital again

This prevents an intentional unwind from being immediately undone by a stale strategy output that was produced while the system was supposed to remain flat.

## High-Level Flow

Below is the execution flow in the exact order it runs once the executor has determined that it should process a normal strategy cycle.

Before any normal execution cycle begins, the executor:

1. loads the durable execution control state from Postgres
2. decides whether the current mode is `normal` or `stable_only`
3. either:
   - proceeds with strategy execution, or
   - runs an unwind pass instead

If the current mode is `stable_only`, the normal strategy-execution flow below is skipped entirely.

1. Receive strategy output
   - `ExecutionEngine::run_once(...)` receives a `PortfolioData` object from the strategy layer.
   - If execution is being driven from an already-persisted strategy run, `run_once_with_existing_strategy_run(...)` is used instead.

2. Persist the strategy run
   - If `run_once(...)` is called, the strategy run and targets are first written to the database.
   - This gives all subsequent trade records a `strategy_run_id`.

3. Normalize target weights
   - Strategy weights are normalized to sum to $1$ before execution decisions are made.

4. Build a live portfolio snapshot
   - The engine reads current GM balances, asset balances, native ETH, dYdX perp positions, dYdX main-account USDC, dYdX subaccount equity, and dYdX free collateral.
   - This snapshot is the single source of truth for the current portfolio state at that step.

5. Compute reserve requirements
   - The engine estimates how much capital must be held back rather than invested:
     - base reserve
     - dYdX hedge reserve
     - gas reserve target

6. Pull excess capital back from dYdX if deployment needs it
   - If the subaccount has more equity/free collateral than required, and Arbitrum is short on deployable capital, the engine withdraws excess from dYdX.

7. Rebalance native gas reserve
   - The engine ensures the wallet holds enough native ETH for future transactions.
   - It may unwrap WETH, buy ETH with stables, or sell excess ETH into stables.

8. Convert target weights into target market values
   - The investable capital is allocated across GM markets according to normalized weights.

9. Compute market deltas
   - The engine compares current GM market positions to target GM market values and converts those value differences into GM token quantity deltas.

10. Execute same-collateral GM shifts first
   - If one GM market should be reduced and another with the same collateral pair should be increased, the engine prefers a GM-to-GM shift instead of a withdrawal plus deposit (to minimize fees).

11. Execute remaining GM withdrawals
   - Markets that remain oversized after shifts are reduced by direct GM withdrawals.

12. Recheck gas and recompute deltas
   - Withdrawals may change token balances and gas state, so the engine refreshes state and recomputes market deltas.

13. Execute deposit stage
   - The engine chooses the cheapest funding token for each target market.
   - It cleans up idle non-stable tokens, sources missing deposit tokens via swaps if needed, and submits GM deposits.

14. Ensure dYdX reserves are actually funded
   - After the GM side is placed, the engine makes sure enough USDC sits inside the dYdX subaccount.

15. Adjust hedge positions
   - For each GM position that should be hedged, the engine estimates the actual collateral exposure via a GMX withdrawal quote and sizes the dYdX hedge from that quote-derived exposure.

16. Persist end-of-cycle snapshot
   - The final portfolio state is written to the database along with per-position snapshots.

17. Persist trade records
   - Every executed action is logged with action type, sizes, approximate USD value, and metadata.

## Stable-Only Unwind Flow

When the executor is in `stable_only` mode, it does **not** run the normal rebalance path above.

Instead it runs `ExecutionEngine::run_unwind_to_stable(...)`.

The unwind flow is a one-directional risk-reduction flow:

1. build a fresh live portfolio snapshot
2. resolve the target stablecoin, typically `USDC`
3. withdraw all material GM positions
4. close all dYdX hedge perp positions
5. swap all Arbitrum ERC-20 inventory, including non-preferred stables, into the target stablecoin
6. sell excess native ETH into the target stablecoin while preserving a small gas buffer
7. persist a portfolio snapshot and trade records

This flow is not intended to optimize portfolio allocation. It is intended to reduce risk and converge the live system toward a flat stablecoin posture.

### Stable-only retry loop

The executor binary runs a periodic retry loop while in `stable_only` mode.

At startup:

- if the persisted control-state mode is already `stable_only`, the executor runs an unwind pass immediately

During runtime:

- the executor listens for `execution_control` wake-up signals
- the executor also runs a periodic unwind tick, every `180` seconds by default

The periodic tick exists because a single unwind pass may not fully flatten the portfolio:

- transactions may fail
- balances may settle across multiple passes
- some venues may require retry behavior

Additionally, the periodic tick ensures that even if the `execution_control` Redis wake-up signal is somehow missed, any change in the durable Postgres execution state is still adhered to. 

The retry loop is awaited inline, so the executor does not run multiple unwind passes concurrently.

## Core Data Model

The execution engine and runtime work with four layers of state:

1. Strategy target state
   - This is the desired portfolio produced by the strategy engine.
   - Represented by `PortfolioData`.

2. Live portfolio state
   - This is the wallet + dYdX state at execution time.
   - Represented by `PortfolioSnapshot`.

3. Trade action plan
   - This is the set of concrete actions to perform:
     - GM deposit
     - GM withdrawal
     - GM shift
     - spot swap
     - dYdX hedge order
   - Represented by `TradeAction`.

4. Execution control state
   - This is the operator-controlled runtime mode that decides whether execution is allowed to enter positions or should remain in stablecoins.
   - Represented durably by `execution_control_state`.

## Detailed Step-by-Step Workflow

### 0. Executor Mode Resolution

Before a strategy run is executed, the executor binary resolves its current control mode from Postgres.

This logic lives in:

- `crypto-yield-farming-bot/src/bin/executor.rs`

The executor subscribes to two Redis pubsub channels:

- `strategy_run_completed`
- `execution_control`

But pubsub does not itself decide behavior. Each time a relevant signal arrives, the executor reloads `execution_control_state` and then makes a decision.

#### If mode is `normal`

The executor evaluates the incoming strategy run normally, subject to:

- matching `strategy_version`
- duplicate/staleness checks
- resume-gate check via `min_strategy_run_id_to_execute`

#### If mode is `stable_only`

The executor ignores strategy-run execution entirely. A strategy-run signal may still act as a wake-up point for the process, but it will not cause the executor to deploy capital.

This separation matters because it guarantees:

- operator intent is durable
- strategy output cannot override a manual unwind command
- the bot can be held flat across multiple strategy iterations

### 0.1 `min_strategy_run_id_to_execute`

The field `min_strategy_run_id_to_execute` is only meaningful in `normal` mode after resuming from a stable-only period.

If it is set, the executor requires:

$$
\text{strategy\\_run\\_id} \ge \text{min\\_strategy\\_run\\_id\\_to\\_execute}
$$

before `run_once_with_existing_strategy_run(...)` is allowed to run.

This is how the executor prevents stale strategy outputs from immediately re-entering positions after a deliberate unwind.

### 1. Strategy Targets Enter Execution

Normal execution starts with strategy targets.

There are two entrypoints:

- `run_once(...)`, which accepts full `PortfolioData`
- `run_once_with_existing_strategy_run(...)`, which accepts the smaller execution-oriented target composition that has already been persisted in the database

The long-running executor binary uses the second path. It loads:

- the exact `strategy_run_id` received from Redis
- the exact persisted targets for that run

and then executes that run specifically.

This means the executor path does **not** need to reconstruct or rely on the full strategy-layer covariance structure.

Execution begins from strategy targets, and the persisted live executor path uses the reduced execution target state.

The planner first normalizes raw strategy weights:

$$
w_i^{\text{target}} = \frac{w_i}{\sum_j w_j}
$$

If the raw sum is non-positive, the code uses $1$ as the denominator to avoid division by zero. This means execution always works with a well-defined weight map, even if the strategy produced an invalid or degenerate result.

This happens in:

- `crypto-yield-farming-bot/src/execution/planner.rs`

The normalized map is then used throughout execution.

### 2. Live Snapshot Construction

The engine constructs a `PortfolioSnapshot` before any rebalancing decisions are made.

The snapshot contains:

- GM token balances by market
- GM token USD values by market
- asset token balances on Arbitrum
- asset token USD values on Arbitrum
- native ETH balance
- current dYdX hedge perp sizes
- dYdX main-account USDC
- dYdX subaccount equity
- dYdX free collateral
- aggregate GM / asset / Arbitrum / total USD values

#### Important implementation detail

The snapshot defines:

$$
\text{native\\_value\\_usd} = \text{native\\_balance} \cdot P_{\text{ETH}}
$$

$$
\text{arbitrum\\_value\\_usd} = \text{market\\_value\\_usd} + \text{asset\\_value\\_usd} + \text{native\\_value\\_usd}
$$

and:

$$
\text{total\\_value\\_usd} = \text{arbitrum\\_value\\_usd} + \text{dydx\\_main\\_usdc} + \text{dydx\\_subaccount\\_equity}
$$

This means native ETH is tracked separately as its own balance field for operational purposes, but its USD value is included in both `arbitrum_value_usd` and `total_value_usd`.

#### Why snapshot-first matters

Execution is stateful and multi-stage. Every later decision depends on current balances and current dYdX state. The snapshot step centralizes that state and prevents each downstream stage from independently reconstructing it.

### 3. Reserve Sizing

Before the engine allocates capital to GM markets, it computes how much capital must be reserved.

There are three reserve concepts:

1. Base reserve
2. dYdX hedge reserve
3. Gas reserve

#### 3.1 Base reserve

The planner config contains `reserve_pct`. The base reserve is:

$$
R_{\text{base}} = V_{\text{total}} \cdot r
$$

where:

- $V_{\text{total}}$ is `snapshot.total_value_usd`
- $r$ is `planner_config.reserve_pct`

This is a generic capital holdback even before dYdX-specific requirements are considered.

#### 3.2 dYdX hedge reserve
##### 3.2.1 Investible capital

GM investable capital and hedge reserve requirements depend on each other.

If more capital is deployed:

- GM target sizes become larger
- hedge notionals become larger
- required dYdX margin becomes larger
- reserve must therefore be larger

But if reserve becomes larger, deployable capital becomes smaller again.

So the engine approximates the fixed point:

$$
V_{\text{investable}} = V_{\text{total}} - \max(R_{\text{base}}, E_{\text{required}}(V_{\text{investable}}))
$$

using a 2-pass iteration, where $E_{\text{required}}(V_{\text{investable}})$ is the required dYdX equity based on the investible capital.

That logic lives in `compute_reserve_state(...)`.

##### 3.2.2 dYdX required margin

For each target market, the engine estimates a hedge notional and divides by usable dYdX leverage.

At a high level:

$$
M_{\text{raw}} = \sum_i \frac{N_i}{L_i}
$$

where:

- $N_i$ is the target hedge notional for market $i$
- $L_i$ is the max usable leverage for the long token hedge
- $M_{\text{raw}}$ is the raw margin requirement for market $i$

##### 3.2.3 Quote-derived hedge notional

The engine does **not** reserve margin from a static rule like “hedge 50% of package capital”.

Instead, for each market it:

1. chooses a deposit token
2. simulates a GM deposit for the target USD value
3. simulates a full GM withdrawal of that resulting GM amount
4. reads the withdrawal outputs:
   - `long_amount_out`
   - `short_amount_out`
5. converts those outputs to USD
6. maps them into hedge notional

If the short collateral is stable:

$$
N_i = V^{\text{long}}_i
$$

If the short collateral is non-stable:

$$
N_i = V^{\text{long}}_i + V^{\text{short}}_i
$$

This is implemented by `hedge_notional_from_withdrawal_values(...)`.

This is quote-derived. The live hedge tracks the actual collateral exposure implied by the GM position instead of a fixed fraction of notional capital.

##### 3.2.4 Low leverage guard

The engine also keeps track of the smallest usable leverage across hedged markets:

$$
L_{\min} = \min_i L_i
$$

and adds a portfolio-level safety floor:

$$
G_{\text{low-lev}} = \frac{0.25 \cdot V_{\text{investable}}}{L_{\min}}
$$

Then it blends the low-leverage guard with the raw margin requirement:

$$
M_{\text{guard}} = 0.75 \cdot M_{\text{raw}} + G_{\text{low-lev}}
$$

and finally:

$$
M_{\text{required}} = \max(M_{\text{raw}}, M_{\text{guard}})
$$

Thus:

$$
M_{\text{required}} = \max(M_{\text{raw}}, 0.75 \cdot M_{\text{raw}} + 0.25 \cdot \frac{V_{\text{investable}}}{L_{\min}})
$$

This is a heuristic safety mechanism. It prevents the reserve requirement from being overly small in regimes where a majority of capital is deployed in GM markets collateralized by tokens with high leverage requirements on dYdX (e.g. BTC, ETH), only to then need to be rapidly increased during a shift to GM markets with low-leverage collateral tokens. Maintaining a stable dYdX reserve requirement allows more long term efficiency by avoiding the fees associated with excessive transfers between dYdX and Arbitrum.

##### 3.2.5 Required equity and free collateral

Once required margin is determined, the engine targets extra equity above the bare minimum margin:

$$
E_{\text{required}} = 1.5 \cdot M_{\text{required}}
$$

Then required free collateral is derived as:

$$
F_{\text{required}} = E_{\text{required}} - M_{\text{required}} = 0.5 \cdot M_{\text{required}}
$$

So the intended dYdX state is:

- enough equity to fully cover required margin
- plus an additional $50\%$ of that margin as free collateral buffer

#### 3.3 Gas reserve target

Separately, the engine also targets a native gas reserve:

$$
V_{\text{gas-target-usd}} = \text{arbitrum\\_value\\_usd} \cdot g
$$

$$
Q_{\text{gas-target-eth}} =
\begin{cases}
\frac{V_{\text{gas-target-usd}}}{P_{\text{ETH}}}, & P_{\text{ETH}} > 0 \\
0, & \text{otherwise}
\end{cases}
$$

where $g$ is `planner_config.gas_reserve_pct`.

### 4. Pulling Excess Capital Back from dYdX

Before new GM deployment, the engine checks whether the dYdX subaccount is overfunded relative to current reserve requirements.

It computes:

$$
E_{\text{excess}} = E_{\text{subaccount}} - E_{\text{required}}
$$

$$
F_{\text{excess}} = F_{\text{subaccount}} - F_{\text{required}}
$$

$$
X = \min(E_{\text{excess}}, F_{\text{excess}})
$$

Then it checks how much capital Arbitrum is short relative to investable capital:

$$
S = V_{\text{investable}} - V_{\text{arbitrum}}
$$

If both $X$ and $S$ exceed the planner threshold, it withdraws:

$$
W = \min(X, S)
$$

from the dYdX subaccount and then out to Arbitrum.

#### Why this stage exists

Without this step, capital could remain stranded on dYdX while GM markets remain underfunded, even though hedge reserve requirements do not justify that much capital sitting off-chain.

### 5. Gas Reserve Management

The engine ensures the wallet holds enough native ETH to continue transacting.

#### If native ETH is too low

If current ETH is below target:

1. unwrap WETH first
2. if still short, try to buy ETH from stablecoins
3. if even stable balances are insufficient, try to pull capital from dYdX first

The order is intentional:

- unwrapping WETH is cheapest operationally
- using local stable balances is cheaper than a cross-system dYdX withdrawal
- dYdX withdrawals are used when needed

#### If native ETH is too high

If ETH exceeds target by more than the minimum USD threshold, the excess is sold back into the preferred stable token.

### 6. Target Market Values and Market Deltas

Once reserve sizing is done, the remaining capital is allocated across target markets:

$$
V_i^{\text{target}} = w_i^{\text{target}} \cdot V_{\text{investable}}
$$

The engine then compares current market value to target market value:

$$
\Delta V_i = V_i^{\text{target}} - V_i^{\text{current}}
$$

and converts to GM token quantity delta using current GM mid price:

$$
\Delta Q_i^{\text{GM}} = \frac{\Delta V_i}{P_i^{\text{GM}}}
$$

#### Thresholding

A delta is ignored only if **both** conditions are true:

$$
|\Delta V_i| < \text{min\\_value\\_usd}
$$

and

$$
|\Delta w_i| < \text{min\\_weight\\_delta}
$$

This prevents noise-sized trades while still allowing meaningful rebalances to proceed.

### 7. GM Shift Stage

Before the engine performs withdrawals and fresh deposits, it tries to directly shift GM inventory between markets that share the same collateral pair.

Markets are grouped by:

$$
(\text{long\\_token}, \text{short\\_token})
$$

Within each group:

- sellers are markets with negative delta
- buyers are markets with positive delta

The engine greedily matches seller surplus against buyer deficit:

$$
Q_{\text{shift}} = \min(Q_{\text{seller available}}, Q_{\text{buyer needed}})
$$

#### Why shifts come first

This is cheaper (from a fee perspective) and operationally cleaner than:

1. withdraw GM from one market
2. convert inventory back to underlying tokens
3. deposit again into another market

If the collateral pair is the same, a shift is the closest thing to an internal rebalance.

### 8. GM Withdrawal Stage

After shifts, remaining negative deltas are handled through direct GM withdrawals.

For any market with negative delta:

$$
Q_i^{\text{withdraw}} = \min(-\Delta Q_i^{\text{GM}}, Q_i^{\text{current}})
$$

where $Q_i^{\text{current}}$ is the current wallet GM token balance.

#### Why withdrawals come before deposits

This ordering frees capital before the engine tries to fund new deposits. It reduces the chance of unnecessary spot swaps and makes the deposit stage operate on a cleaner post-withdrawal inventory.

### 9. Deposit Stage

The deposit stage is the most operationally involved part of execution.

It has three jobs:

1. choose deposit tokens
2. clean up inventory that is unlikely to be useful
3. fund and execute GM deposits

#### 9.1 Choosing the preferred stable token

The engine chooses a preferred stable denomination using `get_preferred_stable_token(...)`.

Priority order:

1. `USDC` if configured
2. `USDC.e` if configured
3. otherwise the stablecoin with the highest current USD balance

This is a denomination preference, not just a balance preference.

#### 9.2 Choosing the deposit token for each market

At the start of the deposit stage, the engine has a fresh portfolio snapshot. It uses that fresh snapshot to build an initial per-market deposit-token cache before any new swaps or deposits are executed in this stage.

For each market, the engine estimates the all-in cost of depositing via the long token vs the short token.

For each candidate side:

$$
\text{all-in cost} = \text{GM deposit fee estimate} + \text{swap fee estimate if funding token missing}
$$

Specifically:

- GM deposit fee is estimated by simulating a $\\$1$-equivalent GM deposit on that side
- swap fee is estimated only if the wallet does not already hold enough of that token

Then the engine compares the two total estimated costs.

Let:

$$
C_{\text{long}} = \text{long GM fee estimate} + \text{long swap fee estimate}
$$

and

$$
C_{\text{short}} = \text{short GM fee estimate} + \text{short swap fee estimate}
$$

The decision rule is:

1. if

$$
|C_{\text{long}} - C_{\text{short}}| > 0.001
$$

choose the cheaper side directly

2. otherwise, treat the two options as effectively tied and use a stable-side preference:
   - if short token is stable, prefer short
   - else if long token is stable, prefer long
   - else choose the lower estimated total cost

The threshold above is an absolute fee-difference threshold of `0.001`, i.e. `10` bps.

#### Why this structure exists

Execution should not blindly deposit via a fixed side. On some markets, the deposit penalty and swap penalty differ materially by side, so the cheaper route should be chosen dynamically.

At the same time, the cost estimates are still approximations based on small quotes, so when the two options are very close the engine falls back to the operationally cleaner stable-side preference instead of overreacting to noisy micro-differences.

#### Cache invalidation during the deposit loop

The initial cache is trustworthy at the beginning of the stage because it is built from a fresh snapshot. However, once the engine starts swapping into or depositing non-base-stable tokens, some future cached choices can become stale.

The engine therefore treats the initial cache as the default plan, but invalidates cached entries selectively as execution progresses.

If a successful action changes the balance of a non-base-stable token $T$, the engine invalidates every market whose long or short token is $T$.

Then, when one of those invalidated markets is reached later in the loop, the engine recomputes that market's deposit-token choice using live balances for that market pair:

$$
\\{ \text{balance(long token)}, \text{balance(short token)} \\}
$$

This gives the engine a hybrid model:

- fast initial cache construction from a fresh snapshot
- no need to rebuild the full snapshot after every swap
- live recomputation only for markets whose cached decision has been invalidated by earlier balance changes

#### 9.3 Cleaning up idle non-stable tokens

Before executing deposits, the engine tries to liquidate non-stable tokens that are not useful for the set of desired deposits.

It keeps:

- the preferred stable token
- all stablecoins
- any token selected as a preferred deposit token for some pending target market

Non-stable tokens not in the keep-set are sold into the preferred stable if their USD value exceeds the minimum threshold.

##### Why cleanup happens before deposits

This concentrates inventory into a smaller set of tokens and reduces the amount of ad hoc funding logic required while deposits are being executed. Additionally, this reduced the probability of any situation where there is a lack of the prefered stablecoin for a given deposit.

#### 9.4 Funding the selected deposit token

For each deposit target:

1. compute required USD value from GM delta and GM price
2. check whether the market's cached deposit-token choice has been invalidated by earlier non-base-stable balance changes
3. if invalidated, recompute the deposit-token choice from live balances for that market pair
4. convert required USD value into required deposit-token amount
5. inspect the live current balance of the chosen deposit token
6. if short:
   - wrap native ETH into WETH if the deposit token is WETH, but only from the ETH balance above the gas reserve target
   - otherwise swap a stable source token into the deposit token

Then the engine deposits the available amount into GM.

When the engine needs to choose which stable token to swap from, it uses live stable balances rather than stale stage-entry balances.

If WETH funding is needed, the engine does not allow the deposit stage to consume protected gas ETH. Instead it limits wrapping to:

$$
Q^{\text{wrappable ETH}} = \max(0, Q^{\text{native ETH}} - Q^{\text{gas target ETH}})
$$

and then wraps at most:

$$
Q^{\text{wrap}} = \min(Q^{\text{shortfall}}, Q^{\text{wrappable ETH}})
$$

This preserves the gas reserve set earlier in the execution cycle.

#### Important implementation detail

The deposit amount is clipped to the available balance:

$$
Q^{\text{deposit}} = \min(Q^{\text{available}}, Q^{\text{needed}})
$$

So the engine does not fail the entire cycle just because the exact intended amount could not be fully sourced.

#### 9.5 Final non-stable cleanup

After the deposit loop finishes, the engine performs one final cleanup pass and liquidates any remaining non-stable tokens back into the preferred stable token, subject to the minimum-value threshold.

This ensures the cycle does not end with unnecessary residual non-stable inventory sitting idle on Arbitrum.

### 10. Funding dYdX Reserve

After the GM-side rebalance has been executed, the engine ensures enough USDC sits inside the dYdX subaccount.

It calculates:

$$
S_E = \max(E_{\text{required}} - E_{\text{subaccount}}, 0)
$$

$$
S_F = \max(F_{\text{required}} - F_{\text{subaccount}}, 0)
$$

$$
S = \max(S_E, S_F)
$$

If $S$ is material:

1. use existing dYdX main-account USDC first
2. if that is insufficient, deposit the remainder via `dydx_deposit(...)`

#### Why this comes after GM positioning

The engine first decides how much capital should live on Arbitrum for GM deployment, and only then finalizes how much must sit inside dYdX. This ordering avoids overfunding dYdX before the GM rebalance is complete.

### 11. Hedge Adjustment Stage

The hedge stage is the final portfolio-shaping step.

For each market with:

- positive GM balance
- positive target weight
- non-stable long token

the engine does the following.

#### 11.1 Estimate actual current collateral exposure

The engine requests a full GM withdrawal quote for the wallet’s current GM balance:

$$
Q^{\text{GM current}}_i
$$

and reads:

- `long_amount_out`
- `short_amount_out`

Those are converted into USD:

$$
V^{\text{long}}_i = Q^{\text{long out}}_i \cdot P^{\text{long}}_i
$$

$$
V^{\text{short}}_i = Q^{\text{short out}}_i \cdot P^{\text{short}}_i
$$

Then hedge notional is:

$$
N_i =
\begin{cases}
V^{\text{long}}_i, & \text{short token is stable} \\
V^{\text{long}}_i + V^{\text{short}}_i, & \text{short token is non-stable}
\end{cases}
$$

#### 11.2 Convert notional to target perp size

The hedge is always expressed in units of the long-token dYdX perp:

$$
Q_i^{\text{target hedge}} = -\frac{N_i}{P_i^{\text{long}}}
$$

The negative sign reflects that the hedge is intended to be short the non-stable exposure.

#### 11.3 Compare to current dYdX position

Let the current dYdX perp size be:

$$
Q_i^{\text{current hedge}}
$$

Then the adjustment is:

$$
\Delta Q_i^{\text{hedge}} = Q_i^{\text{target hedge}} - Q_i^{\text{current hedge}}
$$

If:

$$
|\Delta Q_i^{\text{hedge}}| \cdot P_i^{\text{long}} < \text{min\\_value\\_usd}
$$

the hedge adjustment is skipped.

Otherwise:

- if $\Delta Q_i^{\text{hedge}} > 0$, submit a buy order
- else, submit a sell order

This sign convention matches the dYdX client interface used by the code, not a textual interpretation of “long” or “short”.

#### Why the hedge stage comes last

The intended hedge should match the **final** GM position, not an intermediate state. If hedges were placed before withdrawals/deposits/shifts finished, the portfolio would spend more of the cycle in an intentionally mismatched state.

### 12. Action Execution and Trade Logging

All concrete actions are represented by `TradeAction`.

Supported action types:

- `GmDeposit`
- `GmWithdrawal`
- `GmShift`
- `SpotSwap`
- `HedgeOrder`

Each action is:

1. executed
2. converted into a `TradeStatus`
3. logged to the `trades` table

The engine uses `execute_and_log(...)` so that execution and persistence are kept coupled. This avoids scattered patterns where a trade might execute but not be logged, or vice versa.

### 13. End-of-Cycle Snapshot Persistence

After the final stage, the engine records:

- aggregate portfolio snapshot
- per-market GM positions
- per-token wallet balances
- native ETH balance
- dYdX hedge positions

The snapshot PnL is computed relative to the most recent persisted snapshot:

$$
\text{pnl\\_usd} = V_{\text{total now}} - V_{\text{total previous}}
$$

This is bookkeeping-oriented portfolio accounting, not strategy attribution accounting.

## Design Rationale

### Why the flow is staged instead of fully optimized globally

The engine is structured as a deterministic pipeline rather than a single global optimizer. The main reasons are:

1. The action space mixes very different systems
   - GMX deposits/withdrawals/shifts
   - Paraswap spot swaps
   - dYdX perp orders
   - dYdX reserve transfers

2. Some stages naturally unlock later stages
   - withdrawals free capital for deposits
   - deposits determine final hedge size
   - reserve funding depends on final GM positioning

3. Operational safety matters more than theoretical optimality
   - the engine prefers an understandable execution order over a harder-to-debug global planner

### Why same-collateral shifts are prioritized

If two markets share the same collateral pair, shifting is operationally superior to full withdrawal + redeposit because it avoids unnecessary token churn.

### Why hedge sizing is quote-derived

The engine sizes hedges from quote-derived withdrawal outputs because that is a closer proxy for actual underlying collateral exposure.

This is especially important for:

- stable-short pools, where true long-token exposure is not necessarily equal to half the capital package
- pools whose composition changes over time

### Why gas reserve is managed explicitly

The engine treats native ETH as operational infrastructure rather than just another asset. A portfolio can be economically correct but operationally broken if it cannot pay gas.

### Why dYdX reserves are buffered above bare margin

The engine does not target “just enough” margin. It targets margin plus extra free collateral because:

- perp marks move
- balances move across stages
- hedge changes may not happen instantly
- operating exactly at the line is fragile

### Why trade logging is embedded in execution

Execution without durable trade records is not acceptable for debugging, reconciliation, or post-mortem analysis. Logging is therefore part of the execution path rather than an optional sidecar.

## Practical Caveats

The execution layer is pragmatic, not perfect. A few implementation details are worth keeping in mind:

1. Native ETH is tracked separately for gas management, but its USD value is included in `arbitrum_value_usd` and `total_value_usd`.
2. Deposit-token selection is fee-estimate-based, not slippage-optimized across full trade size.
3. Hedge sizing is quote-derived, but still a proxy for true economic risk rather than a full Greeks-based hedge.
4. Market deltas are based on GM mid prices and current wallet balances, not order-book-aware execution simulation.
5. The reserve model is a heuristic safety model, not an exchange-native margin engine replica.
6. `stable_only` is a durable operator override. While it is active, the strategy engine may continue producing runs, but the executor will not deploy those targets.
7. Returning to `normal` does not immediately re-enter positions. The system waits for the next fresh strategy run after the recorded resume point.

## File Map

- `crypto-yield-farming-bot/src/execution/engine.rs`
  - orchestration, reserve sizing, gas management, deposit/withdraw/hedge execution, logging
- `crypto-yield-farming-bot/src/execution/planner.rs`
  - target normalization, target values, market deltas, GM shift planning
- `crypto-yield-farming-bot/src/execution/types.rs`
  - core execution structs and action enums
- `crypto-yield-farming-bot/src/bin/executor.rs`
  - long-running executor runtime, watchdog, control-state handling, strategy-run gating, stable-only retry loop
- `crypto-yield-farming-bot/src/bin/executor_ctl.rs`
  - operator CLI for `status`, `stay-stable`, and `resume-normal`
- `crypto-yield-farming-bot/src/db/schema/execution_control_state.sql`
  - singleton durable execution mode row
- `crypto-yield-farming-bot/src/db/schema/execution_control_events.sql`
  - append-only audit history of operator-issued execution mode changes
