CREATE TABLE IF NOT EXISTS trades (
    id SERIAL PRIMARY KEY,
    timestamp TIMESTAMPTZ NOT NULL,
    action_type TEXT NOT NULL,
    strategy_run_id INTEGER REFERENCES strategy_runs(id) ON DELETE SET NULL,
    market_id INTEGER REFERENCES markets(id),
    from_token_id INTEGER REFERENCES tokens(id),
    to_token_id INTEGER REFERENCES tokens(id),
    amount_in NUMERIC,
    amount_out NUMERIC,
    usd_value NUMERIC,
    fee_usd NUMERIC,
    tx_hash TEXT,
    status TEXT NOT NULL,
    details TEXT
);

CREATE INDEX IF NOT EXISTS idx_trades_timestamp
    ON trades(timestamp);
