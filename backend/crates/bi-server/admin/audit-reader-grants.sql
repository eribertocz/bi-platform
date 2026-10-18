-- Least-privilege grants for the audit reader role (Issue #159): a login
-- that only reads the audit trail, apart from the bi-server runtime role.
--
-- Run it as the role that owns the metadata (the one that runs
-- `bi-server migrate`), after migrating. Every statement is idempotent.
--
-- The reader role must already exist as a LOGIN role without SUPERUSER,
-- CREATEROLE, CREATEDB, REPLICATION or BYPASSRLS, created by a cluster
-- administrator, and must be a member of no other role: in particular not
-- of the owner nor of the runtime role. No password is ever stored here.
-- `bi_audit_reader` is the example name: replace it with yours, and give
-- the server its connection string in BI_AUDIT_READER_DATABASE_URL.
--
-- The reader gets SELECT on the chain and its head, and nothing else: no
-- other privilege on them, no grant option, no EXECUTE on audit_append, no
-- access to the sessions, no write to the migration history, no CREATE in
-- bi_meta. The server refuses every connection of a reader that can do
-- more, or less, and then keeps audit reading disabled.
--
-- To revoke it, as the owner; the chain itself is never touched:
--   REVOKE SELECT ON TABLE bi_meta.audit_events, bi_meta.audit_head
--       FROM bi_audit_reader;
--   REVOKE USAGE ON SCHEMA bi_meta FROM bi_audit_reader;
-- and REVOKE CONNECT ON DATABASE from it, or drop the role.

DO $$
BEGIN
    EXECUTE format('GRANT CONNECT ON DATABASE %I TO bi_audit_reader', current_database());
END
$$;
GRANT USAGE ON SCHEMA bi_meta TO bi_audit_reader;
GRANT SELECT ON TABLE bi_meta.audit_events, bi_meta.audit_head TO bi_audit_reader;
