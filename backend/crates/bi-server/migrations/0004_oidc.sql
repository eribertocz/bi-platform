-- Only verified identity attributes are retained; OAuth tokens are never stored.
ALTER TABLE bi_meta.sessions ADD COLUMN federated_identity JSONB;

CREATE TABLE bi_meta.oidc_attempts (
    state_hash BYTEA PRIMARY KEY CHECK (octet_length(state_hash) = 32),
    browser_hash BYTEA NOT NULL CHECK (octet_length(browser_hash) = 32),
    nonce TEXT NOT NULL,
    verifier TEXT NOT NULL,
    previous_session BYTEA,
    expires_at TIMESTAMPTZ NOT NULL DEFAULT (clock_timestamp() + interval '5 minutes')
);
CREATE INDEX oidc_attempts_expires_at ON bi_meta.oidc_attempts (expires_at);
