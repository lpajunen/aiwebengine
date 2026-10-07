-- What a script recorded as having happened, which it can add to and cannot
-- take back.
--
-- `logs` is a diagnostic: it is pruned to a count, and `clear_logs` empties it,
-- so a line a script wrote about a refund or a role change can be gone by the
-- time anyone asks. This table holds what a script calls `audit.record` with,
-- and nothing a script or an operation can reach deletes from it: rows leave
-- only by age (`logs.audit_retention_days`) or with the script itself.
--
-- Who did it is the engine's to say, not the script's: `actor_kind` and
-- `actor_id` come from the execution's principal and `client_ip` from the
-- address the edge judged, so a script recording an event cannot attribute it
-- to somebody else.

CREATE TABLE IF NOT EXISTS script_audit_events (
    id          BIGSERIAL PRIMARY KEY,
    script_uri  TEXT NOT NULL REFERENCES scripts(uri) ON UPDATE CASCADE ON DELETE CASCADE,
    action      TEXT NOT NULL,
    details     JSONB,
    actor_kind  TEXT NOT NULL,
    actor_id    TEXT,
    client_ip   TEXT,
    request_id  TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX IF NOT EXISTS idx_script_audit_events_script
    ON script_audit_events (script_uri, id DESC);
CREATE INDEX IF NOT EXISTS idx_script_audit_events_created_at
    ON script_audit_events (created_at);

COMMENT ON TABLE script_audit_events IS 'Events a script recorded with audit.record. Append-only: rows leave by age or with their script, never by request.';
COMMENT ON COLUMN script_audit_events.actor_kind IS 'Who the execution acted for, from its principal: caller, delegated, contained or engine.';
COMMENT ON COLUMN script_audit_events.actor_id IS 'The account the execution acted for, or the engine actor''s label; NULL for an anonymous caller.';
