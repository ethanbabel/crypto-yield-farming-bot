CREATE TABLE IF NOT EXISTS token_prices (
    id SERIAL PRIMARY KEY,
    token_id INTEGER NOT NULL REFERENCES tokens(id),
    timestamp TIMESTAMPTZ NOT NULL DEFAULT now(),
    min_price NUMERIC NOT NULL,
    max_price NUMERIC NOT NULL,
    mid_price NUMERIC NOT NULL
);