CREATE TABLE IF NOT EXISTS market_states (
    id SERIAL PRIMARY KEY,
    market_id INTEGER NOT NULL REFERENCES markets(id),
    timestamp TIMESTAMPTZ NOT NULL DEFAULT now(),

    borrowing_factor_long NUMERIC,
    borrowing_factor_short NUMERIC,

    pnl_long NUMERIC,
    pnl_short NUMERIC,
    pnl_net NUMERIC,

    gm_price_min NUMERIC,
    gm_price_max NUMERIC,
    gm_price_mid NUMERIC,

    pool_long_amount NUMERIC,
    pool_short_amount NUMERIC,
    pool_impact_amount NUMERIC,
    pool_long_token_usd NUMERIC,
    pool_short_token_usd NUMERIC,
    pool_impact_token_usd NUMERIC,

    open_interest_long NUMERIC,
    open_interest_short NUMERIC,
    open_interest_long_amount NUMERIC,
    open_interest_short_amount NUMERIC,
    open_interest_long_via_tokens NUMERIC,
    open_interest_short_via_tokens NUMERIC,

    utilization NUMERIC,

    swap_volume NUMERIC,
    trading_volume NUMERIC,

    fees_position NUMERIC,
    fees_liquidation NUMERIC,
    fees_swap NUMERIC,
    fees_borrowing NUMERIC,
    fees_total NUMERIC
);

CREATE INDEX IF NOT EXISTS idx_market_states_market_timestamp 
    ON market_states(market_id, timestamp);
