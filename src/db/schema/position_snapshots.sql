CREATE TABLE IF NOT EXISTS position_snapshots (
    id SERIAL PRIMARY KEY,
    portfolio_snapshot_id INTEGER NOT NULL REFERENCES portfolio_snapshots(id) ON DELETE CASCADE,
    position_type TEXT NOT NULL,
    market_id INTEGER REFERENCES markets(id),
    token_id INTEGER REFERENCES tokens(id),
    symbol TEXT,
    size NUMERIC,
    usd_value NUMERIC
);

CREATE INDEX IF NOT EXISTS idx_position_snapshots_snapshot
    ON position_snapshots(portfolio_snapshot_id);
