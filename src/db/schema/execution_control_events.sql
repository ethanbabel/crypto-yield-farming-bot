CREATE TABLE IF NOT EXISTS execution_control_events (
    id SERIAL PRIMARY KEY,
    timestamp TIMESTAMPTZ NOT NULL,
    event_type TEXT NOT NULL,
    mode TEXT NOT NULL CHECK (mode IN ('normal', 'stable_only')),
    target_stable_symbol TEXT,
    min_strategy_run_id_to_execute INTEGER,
    updated_by TEXT,
    reason TEXT
);

CREATE INDEX IF NOT EXISTS idx_execution_control_events_timestamp
    ON execution_control_events(timestamp DESC);
