CREATE TABLE IF NOT EXISTS token_prices (
    id SERIAL PRIMARY KEY,
    token_id INTEGER NOT NULL REFERENCES tokens(id),
    timestamp TIMESTAMPTZ NOT NULL DEFAULT now(),
    min_price NUMERIC NOT NULL,
    max_price NUMERIC NOT NULL,
    mid_price NUMERIC NOT NULL
);

-- Optimizes latest-per-token lookups with ORDER BY token_id, timestamp DESC.
CREATE INDEX IF NOT EXISTS idx_token_prices_token_timestamp_desc
    ON token_prices(token_id, timestamp DESC);

-- Optimizes timestamp range scans used by strategy slice loading.
CREATE INDEX IF NOT EXISTS idx_token_prices_timestamp_token
    ON token_prices(timestamp, token_id);
    
