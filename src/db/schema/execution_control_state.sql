CREATE TABLE IF NOT EXISTS execution_control_state (
    singleton_id BOOLEAN PRIMARY KEY DEFAULT TRUE CHECK (singleton_id),
    mode TEXT NOT NULL CHECK (mode IN ('normal', 'stable_only')),
    target_stable_symbol TEXT,
    min_strategy_run_id_to_execute INTEGER,
    updated_at TIMESTAMPTZ NOT NULL,
    updated_by TEXT,
    reason TEXT
);

INSERT INTO execution_control_state (
    singleton_id,
    mode,
    target_stable_symbol,
    min_strategy_run_id_to_execute,
    updated_at,
    updated_by,
    reason
)
VALUES (
    TRUE,
    'normal',
    NULL,
    NULL,
    NOW(),
    'schema_init',
    'initialized default execution control state'
)
ON CONFLICT (singleton_id) DO NOTHING;
