CREATE TABLE IF NOT EXISTS portfolio_snapshots (
    id SERIAL PRIMARY KEY,
    timestamp TIMESTAMPTZ NOT NULL,
    total_value_usd NUMERIC,
    market_value_usd NUMERIC,
    asset_value_usd NUMERIC,
    hedge_value_usd NUMERIC,
    pnl_usd NUMERIC
);

CREATE INDEX IF NOT EXISTS idx_portfolio_snapshots_timestamp
    ON portfolio_snapshots(timestamp);
