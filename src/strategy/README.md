# Strategy Engine

The strategy layer converts persisted GMX market observations into a durable target portfolio.

Its output is not a trade list. It is a weight vector plus the return and risk metadata needed for later inspection and execution.

The main entrypoint is `src/strategy/engine.rs`, and the long-running runtime that persists results lives in `src/bin/strategy_runner.rs`.

## Why This Module Exists

The repo needed a boundary between:

- deciding which GM markets were worth owning
- figuring out how to move real capital into or out of those markets

That split matters because the optimizer can stay focused on ranking and sizing opportunities, while the execution layer deals with gas, liquidity, dYdX margin, and transaction ordering.

## Inputs

The engine reads historical and current market state from Postgres through `DbManager`.

Each candidate market is represented as a `MarketStateSlice` with:

- historical timestamps and fee observations
- historical index-token prices
- current long and short open interest
- current pool composition
- display metadata and token addresses

The output is `PortfolioData`, which contains:

- ordered market addresses
- human-readable display names
- expected returns
- covariance matrix
- final target weights

## Pipeline

### 1. Data-quality filtering

The engine first rejects markets that do not have enough usable history. In the current implementation that means:

- at least one day of timestamp coverage
- at least one day of index-price coverage
- oldest observations that are old enough to form a trailing window
- newest observations that are recent enough to trust
- a minimum total open-interest threshold

This was a deliberate attempt to avoid sizing positions off partial or stale state.

### 2. Hedgeability filtering

After basic quality checks, the engine removes markets whose long token cannot be hedged on dYdX, unless that token is already treated as stable.

This matters because the portfolio thesis was "earn GMX market fees while flattening unwanted directional exposure," not "quietly accumulate unhedged beta when hedge venues disappear."

### 3. Exit-path filtering

The engine then checks whether both collateral sides can be swapped directly back to USDC through the swap stack.

This is another execution-aware filter. If the system cannot exit a position cleanly, that market should not survive the allocation stage.

### 4. Expected-return model

Expected return comes from `fee_model.rs`.

The model:

- buckets recent fee observations into hourly values
- applies an EWMA to emphasize recent conditions
- divides the expected hourly fee flow by current pool value

This is intentionally simple. The goal was not to build a grand forecasting model; it was to turn recent fee generation into a stable, comparable yield signal.

### 5. Risk model

Risk comes from `covariance.rs`.

The covariance matrix is built from index-token returns, then scaled by each market's net open-interest exposure relative to pool value. That scaling choice reflects the idea that a GM market's residual directional risk is not just token volatility in isolation; it depends on how imbalanced the live long-versus-short exposure is.

### 6. Allocation

The optimizer in `allocator.rs` is intentionally pragmatic rather than academically pure.

It:

- starts from individual Sharpe-like scores
- refines weights with gradient descent
- projects weights back into a long-only portfolio
- zeroes out tiny positions
- caps oversized positions

The result is a constrained, cleaner portfolio that is easier for execution to hold in practice.

## Persistence Model

`strategy_runner` subscribes to data-collection completion events, enforces a cadence, runs the strategy engine, then persists:

- one `strategy_runs` row containing portfolio-level metrics
- one `strategy_targets` row per selected market

That persistence step is important. Execution consumes a concrete saved run, not an in-memory portfolio object, which makes the strategy layer auditable and replayable after the fact.

## Design Intent

The strategy code is conservative in a few ways on purpose:

- it would rather reject a market than pretend missing data is harmless
- it only targets markets the rest of the stack can plausibly hedge and unwind
- it prefers interpretable heuristics over fragile complexity

That matches the role of this module inside the larger system: produce targets that are worth deploying and operationally survivable, not just mathematically attractive on paper.
