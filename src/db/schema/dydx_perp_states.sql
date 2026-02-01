CREATE TABLE IF NOT EXISTS dydx_perp_states (
    id SERIAL PRIMARY KEY,
    dydx_perp_id INTEGER NOT NULL REFERENCES dydx_perps(id),
    timestamp TIMESTAMPTZ NOT NULL,
    funding_rate NUMERIC,
    initial_margin_fraction NUMERIC,
    maintenance_margin_fraction NUMERIC,
    oracle_price NUMERIC,
    open_interest NUMERIC
);

CREATE INDEX IF NOT EXISTS idx_dydx_perp_states_perp_timestamp
    ON dydx_perp_states(dydx_perp_id, timestamp);
