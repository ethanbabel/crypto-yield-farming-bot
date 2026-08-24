# Hedging Module

The hedging layer is the bridge between GMX spot-like inventory on Arbitrum and dYdX perpetual exposure on the Cosmos side.

Its job is not to generate a separate alpha stream. Its job is to make the rest of the system comfortable holding GM positions by neutralizing unwanted directional exposure and keeping hedge capital in the right place.

The main code lives in:

- `src/hedging/dydx_client.rs`
- `src/hedging/hedge_utils.rs`
- `src/hedging/skip_go.rs`

## Why This Module Exists

The core portfolio thesis in this repo is "own GM markets for fee yield, not for naked token direction."

That created two practical requirements:

- map each relevant GM long-token exposure to a hedge instrument
- move USDC between Arbitrum and dYdX when hedge margin needs to change

Those concerns are operationally messy enough that they deserved their own module instead of being spread across strategy or execution logic.

## Main Responsibilities

### dYdX client lifecycle

`DydxClient` wraps the dYdX node client, the indexer client, wallet derivation, and the local account configuration needed to place and monitor perp orders.

The client is built from:

- shared repo config from `src/config.rs`
- the same mnemonic used elsewhere in the system
- a local TOML file referenced by `DYDX_CONFIG_PATH`

### Hedge-market discovery

`hedge_utils.rs` contains the token-symbol normalization used to map GMX assets onto dYdX perps.

Examples:

- `WETH` and `wstETH` map to `ETH-USD`
- `WBTC.b` and `tBTC` map to `BTC-USD`
- stablecoins are explicitly treated as non-hedged inventory

This normalization is what lets the strategy and execution layers ask a simple question: "does this token currently have an active hedge venue?"

### Perp execution and monitoring

The dYdX client is responsible for:

- fetching perp market metadata
- summarizing account equity and free collateral
- placing hedge orders
- polling open orders until they fill, cancel, or time out
- retrying timed-out orders when the surrounding caller decides that is still safe

This polling behavior is why the client tracks active background tasks rather than pretending all hedge actions settle synchronously.

### Cross-chain funding

Hedge capital lives on dYdX, but most inventory and stable balances start on Arbitrum.

`skip_go.rs` exists because the system sometimes has to:

- top up dYdX margin before opening or resizing hedges
- withdraw idle USDC back to Arbitrum when hedge demand falls

The SkipGo wrapper is intentionally thin. It mainly standardizes request shapes, retries transient failures, and gives `DydxClient` a clean place to fetch routes and transfer metadata.

## Relationship To Execution

This module does not decide portfolio targets, and it does not decide the overall sequence of portfolio actions.

Instead, execution asks it to perform specific hedge-side tasks once execution has already decided things like:

- how much GM exposure should exist
- how much hedge notional is required
- whether dYdX needs more or less USDC
- whether the system is unwinding toward a stable-only portfolio

That separation matters because the asynchronous parts of hedging, especially cross-chain transfers, create state that must be tracked durably elsewhere. The execution layer handles that durable orchestration, while this module focuses on the actual dYdX and SkipGo interactions.

## Design Intent

The hedging code was written around a few practical assumptions:

- hedge availability can disappear, so other layers should treat it as a live capability check
- perp orders and bridge transfers are not instant, so the code should expose polling and task tracking explicitly
- stablecoins are inventory, not hedge targets
- cross-chain funding is a support function for risk management, not an end in itself

That keeps the module aligned with the larger purpose of the repo: capture GMX fee opportunities while minimizing the amount of unplanned market risk the system carries.
