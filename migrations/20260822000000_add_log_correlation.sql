-- Correlate log lines with the invocation that emitted them.
--
-- Until now a log row knew only which script wrote it, so the lines from one
-- route call, one scheduler tick or one stream connection were interleaved with
-- every other invocation's output and could not be separated again. These
-- columns carry the identity of the invocation down to each line: `request_id`
-- groups the lines one invocation emitted, `kind` says what sort of invocation
-- it was (httpRoute, scheduled, streamCustomization, …) and `route` names the
-- registered route pattern rather than the concrete path, so filtering by it
-- aggregates every call to that handler.
--
-- All three are nullable: rows written before this migration have no invocation
-- to attribute, and engine-internal writes (startup, transpiler diagnostics)
-- legitimately have none.

ALTER TABLE logs ADD COLUMN request_id TEXT;
ALTER TABLE logs ADD COLUMN kind TEXT;
ALTER TABLE logs ADD COLUMN route TEXT;

-- A stable total order for log rows.
--
-- `created_at` alone cannot order a listing: two lines written in the same
-- microsecond tie, and the tie is broken arbitrarily on every query. `id` is a
-- random UUID, so it cannot break the tie either. `seq` gives every row a
-- monotonic identity that both orders the listing and works as a cursor.
ALTER TABLE logs ADD COLUMN seq BIGSERIAL;

-- Existing rows were numbered by the sequence in physical order; renumber them
-- by timestamp so the pre-migration history reads in the order it was written.
WITH ordered AS (
    SELECT id, ROW_NUMBER() OVER (ORDER BY created_at, seq) AS position
    FROM logs
)
UPDATE logs SET seq = ordered.position
FROM ordered
WHERE logs.id = ordered.id;

SELECT setval(
    pg_get_serial_sequence('logs', 'seq'),
    GREATEST((SELECT COALESCE(MAX(seq), 0) FROM logs), 1)
);

CREATE UNIQUE INDEX idx_logs_seq ON logs(seq);

-- Fetching one invocation's lines is the query this whole change exists for.
CREATE INDEX idx_logs_request_id ON logs(request_id) WHERE request_id IS NOT NULL;

-- Filtering to one handler's output across invocations.
CREATE INDEX idx_logs_route ON logs(route) WHERE route IS NOT NULL;

COMMENT ON COLUMN logs.request_id IS 'Identifies the invocation that emitted this line: the x-request-id of an HTTP request, or a generated id for a scheduler tick, stream customization call or other non-HTTP invocation. NULL for engine-internal writes.';
COMMENT ON COLUMN logs.kind IS 'What sort of invocation emitted this line: httpRoute, graphqlQuery, scheduled, streamCustomization, mcpTool, init, eval, test. NULL for engine-internal writes.';
COMMENT ON COLUMN logs.route IS 'Registered route pattern the invocation was serving, e.g. /virtual-world/:id/move, or the job/stream/tool name for non-HTTP invocations. NULL when the invocation has no such name.';
COMMENT ON COLUMN logs.seq IS 'Monotonic write order, used to break created_at ties and as a listing cursor.';
