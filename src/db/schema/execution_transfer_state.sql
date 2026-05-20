CREATE TABLE IF NOT EXISTS execution_transfer_state (
    singleton_id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton_id),
    direction TEXT,
    tx_hash TEXT,
    chain_id TEXT,
    amount_usd NUMERIC,
    source_balance_before NUMERIC,
    destination_balance_before NUMERIC,
    expected_time_to_complete_secs BIGINT,
    initiated_at TIMESTAMPTZ
);

INSERT INTO execution_transfer_state (
    singleton_id,
    direction,
    tx_hash,
    chain_id,
    amount_usd,
    source_balance_before,
    destination_balance_before,
    expected_time_to_complete_secs,
    initiated_at
)
VALUES (
    TRUE,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL,
    NULL
)
ON CONFLICT (singleton_id) DO NOTHING;
