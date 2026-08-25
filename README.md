# CryptoYieldFarmingBot

This repository is the private codebase for a GMX yield strategy on Arbitrum with dYdX perpetual hedging and an execution layer that can move the live portfolio toward persisted targets.

It is organized around the main parts of the system: market-data ingestion, durable storage, portfolio construction, execution, and hedge management.

## What The System Does

At a high level, the system tries to hold GMX market exposure where fee yield is attractive while hedging directional risk on dYdX.

The steady-state loop is:

1. collect GMX and token data
2. persist snapshots into Postgres
3. run a portfolio construction pass that ranks and sizes GM markets
4. persist target weights as a durable strategy run
5. have the execution layer consume that run and move the live portfolio toward it
6. use dYdX perpetuals and occasional cross-chain USDC transfers to keep hedges funded

Redis is used as a wake-up bus between services. Postgres is the source of truth.

## Main Runtime Pieces

### Production binaries

- `data_collector`: listens for on-chain or near-real-time market data updates
- `data_recorder`: writes normalized market and token state into Postgres
- `strategy_runner`: computes target portfolio weights from persisted observations
- `executor`: applies the latest eligible strategy run to the live portfolio

### Operator / utility binaries

- `executor_ctl`: switches the executor between normal and `stable_only` modes
- `init_db_schema`: creates the application schema from the checked-in SQL
- `backtest`: offline strategy experimentation
- `see_balances`: wallet inspection helper
- `spot_swap`: explicit swap helper
- `transact_gm_tokens`: explicit GM deposit / withdrawal helper
- `dydx_transfer`: explicit helper for transfering funds between Arbitrum and dydx
- `dydx_trade_perps`: explicit dYdX perp trading helper
- `main`: development entrypoint

## Architecture

The codebase is intentionally split into stages that match the actual operating loop:

- `src/data_ingestion/`: fetch and normalize raw market data
- `src/db/`: schema, models, and query layer for durable state
- `src/strategy/`: turns historical market observations into target weights
- `src/execution/`: turns target weights into concrete portfolio actions
- `src/hedging/`: dYdX perp logic plus cross-chain funding via SkipGo
- `src/gmx/`: GMX-specific contract and event access
- `src/spot_swap/`: spot swap routing and execution support
- `src/gm_token_txs/`: explicit GM deposit / withdrawal / shift transaction helpers

Deep dives live in:

- [Execution Workflow](src/execution/README.md)
- [Strategy Engine](src/strategy/README.md)
- [Hedging Module](src/hedging/README.md)

## Design Choices

### The strategy is execution-aware

The strategy engine does not just rank markets by some abstract score and hand the rest to execution. It filters markets aggressively before optimization:

- markets need enough recent history
- long tokens need a live dYdX hedge venue unless they are already stable
- both long and short legs need a direct path back to USDC

That keeps the optimizer from producing targets the rest of the system cannot actually hold.

### Yield capture matters more than directional conviction

The strategy layer models expected return from recent fee generation relative to pool value, then estimates covariance from token-price returns scaled by net open-interest exposure. In other words, the portfolio is trying to own attractive fee streams while accounting for the residual risk left in each market. The goal of the strategy is to generate PnL primarily as a liquidity provider; dynamically allocating capital to GM markets according to the most jointly attracive weighting profile from a risk-adjusted perspective. 

### Execution owns capital movement, not the strategy layer

Strategy output is durable and abstract: a set of target weights. Execution owns the operational details:

- reserve sizing
- gas management
- GM deposits, withdrawals, and shifts
- dYdX hedge sizing
- cross-chain dYdX funding
- `stable_only` unwind behavior

This separation keeps portfolio logic distinct from transaction choreography.

### dYdX is treated as a hedge venue, not a second alpha engine

The dYdX integration exists to neutralize unwanted directional exposure and to manage hedge margin. The repo does not try to run a separate predictive perp strategy.
