CREATE TABLE IF NOT EXISTS strategy_targets (
    id SERIAL PRIMARY KEY,
    strategy_run_id INTEGER NOT NULL REFERENCES strategy_runs(id) ON DELETE CASCADE,
    market_id INTEGER NOT NULL REFERENCES markets(id),
    target_weight NUMERIC NOT NULL,
    expected_return_bps NUMERIC,
    variance_bps NUMERIC
);

CREATE INDEX IF NOT EXISTS idx_strategy_targets_run
    ON strategy_targets(strategy_run_id);
