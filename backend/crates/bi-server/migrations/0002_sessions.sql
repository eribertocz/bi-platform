CREATE TABLE bi_meta.sessions (
    token_hash BYTEA PRIMARY KEY CHECK (octet_length(token_hash) = 32),
    user_id TEXT NOT NULL,
    display_name TEXT NOT NULL,
    -- G1 identities authorize synthetic fixtures only, never real sources.
    data_scope TEXT NOT NULL CHECK (data_scope = 'synthetic'),
    created_at TIMESTAMPTZ NOT NULL DEFAULT clock_timestamp(),
    expires_at TIMESTAMPTZ NOT NULL,
    revoked_at TIMESTAMPTZ
);

CREATE INDEX sessions_expires_at ON bi_meta.sessions (expires_at);
