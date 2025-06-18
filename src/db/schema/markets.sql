CREATE TABLE IF NOT EXISTS markets (
    id SERIAL PRIMARY KEY,
    address TEXT NOT NULL UNIQUE,
    index_token_id INTEGER NOT NULL REFERENCES tokens(id),
    long_token_id INTEGER NOT NULL REFERENCES tokens(id),
    short_token_id INTEGER NOT NULL REFERENCES tokens(id)
);