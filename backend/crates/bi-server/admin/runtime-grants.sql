-- Least-privilege grants for the bi-server runtime role in the default
-- restricted metadata mode (BI_METADATA_MODE=restricted, Issue #115).
--
-- Run it as the role that owns the metadata (the one that runs
-- `bi-server migrate`), after migrating, and again after an upgrade that
-- adds metadata tables. Every statement is idempotent.
--
-- The runtime role must already exist as a LOGIN role without SUPERUSER,
-- CREATEROLE, CREATEDB, REPLICATION or BYPASSRLS, created by a cluster
-- administrator, and must not be a member of the owner. No password is ever
-- stored here. `bi_runtime` is the example name: replace it with yours.
--
-- The runtime role gets nothing else. In particular it cannot read or
-- change bi_meta.audit_events or bi_meta.audit_head except through
-- audit_append, cannot create objects in bi_meta and cannot modify the
-- migration history: the server refuses to start admitting work otherwise.

DO $$
BEGIN
    EXECUTE format('GRANT CONNECT ON DATABASE %I TO bi_runtime', current_database());
END
$$;
GRANT USAGE ON SCHEMA bi_meta TO bi_runtime;
GRANT SELECT ON TABLE public._sqlx_migrations TO bi_runtime;
GRANT EXECUTE ON FUNCTION bi_meta.audit_append(text) TO bi_runtime;
GRANT SELECT, INSERT, UPDATE ON TABLE bi_meta.sessions TO bi_runtime;
-- DELETE also permits the LOCK ... IN SHARE ROW EXCLUSIVE MODE that
-- serializes OIDC attempts.
GRANT SELECT, INSERT, DELETE ON TABLE bi_meta.oidc_attempts TO bi_runtime;
