CREATE TABLE IF NOT EXISTS portfolio_snapshots (
    id SERIAL PRIMARY KEY,
    strategy_run_id INTEGER REFERENCES strategy_runs(id) ON DELETE SET NULL,
    timestamp TIMESTAMPTZ NOT NULL,
    total_value_usd NUMERIC,
    arbitrum_value_usd NUMERIC,
    market_value_usd NUMERIC,
    asset_value_usd NUMERIC,
    native_balance NUMERIC,
    native_value_usd NUMERIC,
    dydx_main_usdc NUMERIC,
    dydx_subaccount_equity NUMERIC,
    dydx_free_collateral NUMERIC,
    pnl_usd NUMERIC
);

CREATE INDEX IF NOT EXISTS idx_portfolio_snapshots_timestamp
    ON portfolio_snapshots(timestamp);

CREATE INDEX IF NOT EXISTS idx_portfolio_snapshots_strategy_run
    ON portfolio_snapshots(strategy_run_id);
