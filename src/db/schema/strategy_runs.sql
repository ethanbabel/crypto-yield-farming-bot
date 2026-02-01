CREATE TABLE IF NOT EXISTS strategy_runs (
    id SERIAL PRIMARY KEY,
    timestamp TIMESTAMPTZ NOT NULL,
    strategy_version TEXT NOT NULL,
    total_weight NUMERIC,
    expected_return_bps NUMERIC,
    volatility_bps NUMERIC,
    sharpe NUMERIC
);

CREATE INDEX IF NOT EXISTS idx_strategy_runs_timestamp
    ON strategy_runs(timestamp);
