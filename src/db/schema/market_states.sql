CREATE TABLE IF NOT EXISTS market_states (
    id SERIAL PRIMARY KEY,
    market_id INTEGER NOT NULL REFERENCES markets(id),
    timestamp TIMESTAMPTZ NOT NULL DEFAULT now(),

    borrowing_factor_long NUMERIC NOT NULL,
    borrowing_factor_short NUMERIC NOT NULL,

    pnl_long NUMERIC NOT NULL,
    pnl_short NUMERIC NOT NULL,
    pnl_net NUMERIC NOT NULL,

    gm_price_min NUMERIC NOT NULL,
    gm_price_max NUMERIC NOT NULL,
    gm_price_mid NUMERIC NOT NULL,

    pool_long_amount NUMERIC NOT NULL,
    pool_short_amount NUMERIC NOT NULL,
    pool_long_token_usd NUMERIC NOT NULL,
    pool_short_token_usd NUMERIC NOT NULL,

    open_interest_long NUMERIC NOT NULL,
    open_interest_short NUMERIC NOT NULL,
    open_interest_long_via_tokens NUMERIC NOT NULL,
    open_interest_short_via_tokens NUMERIC NOT NULL,

    utilization NUMERIC NOT NULL,

    swap_volume NUMERIC NOT NULL,
    trading_volume NUMERIC NOT NULL,

    fees_position NUMERIC NOT NULL,
    fees_liquidation NUMERIC NOT NULL,
    fees_swap NUMERIC NOT NULL,
    fees_borrowing NUMERIC NOT NULL,
    fees_total NUMERIC NOT NULL
);