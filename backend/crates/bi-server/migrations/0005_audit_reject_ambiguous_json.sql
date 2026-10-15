-- #113: the previous audit_append cast document::jsonb before any shape
-- check. PostgreSQL's jsonb silently keeps only the last of two duplicate
-- keys (any JSON object, not only the root), so the byte-identical TEXT
-- this function stores could disagree with what the caller believed it
-- wrote. `IS JSON ... WITH UNIQUE KEYS` runs on the TEXT itself, before any
-- cast, and checks every object in the document, not only the root.
CREATE OR REPLACE FUNCTION bi_meta.audit_append(document TEXT)
RETURNS TABLE(sequence BIGINT, hash BYTEA)
LANGUAGE plpgsql SECURITY DEFINER SET search_path = pg_catalog AS $$
DECLARE
    event JSONB;
    shape JSONB;
    field TEXT;
    last_sequence BIGINT;
    last_hash BYTEA;
    next_hash BYTEA;
BEGIN
    IF document IS NULL OR octet_length(document) > 8192 THEN
        RAISE EXCEPTION 'audit_invalid_event' USING ERRCODE = '22023';
    END IF;
    -- Malformed JSON, a root that is not an object, and duplicate keys
    -- anywhere in the document (including metadata_redacted and
    -- query_shape, even with Unicode-escaped key spellings that decode to
    -- the same name) are all rejected together, before document::jsonb
    -- could otherwise lose a duplicate silently.
    IF document IS NOT JSON OBJECT WITH UNIQUE KEYS THEN
        RAISE EXCEPTION 'audit_invalid_event' USING ERRCODE = '22023';
    END IF;
    event := document::jsonb;
    IF jsonb_typeof(event) IS DISTINCT FROM 'object'
       OR NOT event ?& ARRAY['event_id', 'occurred_at', 'actor_id', 'actor_type',
           'session_id', 'action', 'resource_type', 'resource_id', 'result',
           'request_id', 'trace_id', 'policy_decision_id', 'source',
           'metadata_redacted', 'schema_version']
       OR event - ARRAY['event_id', 'occurred_at', 'actor_id', 'actor_type',
           'session_id', 'action', 'resource_type', 'resource_id', 'result',
           'request_id', 'trace_id', 'policy_decision_id', 'source',
           'metadata_redacted', 'schema_version'] <> '{}'::jsonb
       OR event->'schema_version' IS DISTINCT FROM '1'::jsonb
       OR event->>'actor_type' NOT IN ('user', 'service', 'anonymous')
       OR event->>'action' NOT IN ('login', 'logout', 'query', 'denial')
       OR event->>'result' NOT IN ('success', 'failure', 'denied')
       OR event->>'source' NOT IN ('http', 'cli', 'internal')
       OR jsonb_typeof(event->'occurred_at') IS DISTINCT FROM 'string'
       OR event->>'occurred_at' !~ '^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(\.\d{1,9})?Z$'
       OR event->>'trace_id' !~ '^[0-9a-f]{32}$'
       OR event->>'trace_id' = repeat('0', 32) THEN
        RAISE EXCEPTION 'audit_invalid_event' USING ERRCODE = '22023';
    END IF;
    -- Null values must not silently bypass three-valued SQL comparisons.
    FOREACH field IN ARRAY ARRAY['actor_type', 'action', 'result', 'source', 'trace_id'] LOOP
        IF jsonb_typeof(event->field) IS DISTINCT FROM 'string' THEN
            RAISE EXCEPTION 'audit_invalid_event' USING ERRCODE = '22023';
        END IF;
    END LOOP;
    PERFORM (event->>'occurred_at')::timestamptz;
    FOREACH field IN ARRAY ARRAY['event_id', 'actor_id', 'resource_type', 'resource_id', 'request_id', 'session_id', 'policy_decision_id'] LOOP
        IF field IN ('session_id', 'policy_decision_id') AND event->field = 'null'::jsonb THEN
            CONTINUE;
        END IF;
        IF jsonb_typeof(event->field) IS DISTINCT FROM 'string'
           OR event->>field !~ '^[A-Za-z0-9_.:-]{1,128}$' THEN
            RAISE EXCEPTION 'audit_invalid_event' USING ERRCODE = '22023';
        END IF;
    END LOOP;
    IF jsonb_typeof(event->'metadata_redacted') IS DISTINCT FROM 'object'
       OR NOT (event->'metadata_redacted') ? 'query_shape'
       OR (event->'metadata_redacted') - 'query_shape' <> '{}'::jsonb THEN
        RAISE EXCEPTION 'audit_invalid_metadata' USING ERRCODE = '22023';
    END IF;
    shape := event->'metadata_redacted'->'query_shape';
    IF shape <> 'null'::jsonb THEN
        IF jsonb_typeof(shape) IS DISTINCT FROM 'object'
           OR NOT shape ?& ARRAY['dimensions', 'measures', 'filters', 'limit']
           OR shape - ARRAY['dimensions', 'measures', 'filters', 'limit'] <> '{}'::jsonb THEN
            RAISE EXCEPTION 'audit_invalid_metadata' USING ERRCODE = '22023';
        END IF;
        FOREACH field IN ARRAY ARRAY['dimensions', 'measures', 'filters', 'limit'] LOOP
            IF jsonb_typeof(shape->field) IS DISTINCT FROM 'number'
               OR shape->>field !~ '^[0-9]+$'
               OR (shape->>field)::numeric > 4294967295 THEN
                RAISE EXCEPTION 'audit_invalid_metadata' USING ERRCODE = '22023';
            END IF;
        END LOOP;
    END IF;
    SELECT h.sequence, h.hash INTO STRICT last_sequence, last_hash
    FROM bi_meta.audit_head h WHERE singleton FOR UPDATE;
    next_hash := sha256(convert_to('bi-platform.audit.v1', 'UTF8') || decode('00', 'hex')
        || int8send(last_sequence + 1) || last_hash || convert_to(document, 'UTF8'));
    INSERT INTO bi_meta.audit_events(sequence, payload, previous_hash, hash)
    VALUES (last_sequence + 1, document, last_hash, next_hash);
    UPDATE bi_meta.audit_head h SET sequence = last_sequence + 1, hash = next_hash WHERE singleton;
    RETURN QUERY SELECT last_sequence + 1, next_hash;
END
$$;
