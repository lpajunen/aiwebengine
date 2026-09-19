-- What one script may spend, when that differs from what the engine allows
-- everyone.
--
-- javascript.execution_timeout_ms and its neighbours are process-wide, so an
-- operator hosting one agent beside twenty ordinary solutions had to raise the
-- ceiling for all of them: the number that lets an agent wait out a model call
-- is also the number a runaway route handler now gets. These rows are the
-- exception, per script and in both directions — raised for the one that needs
-- it, lowered for the one that has been misbehaving.
--
-- Every column is nullable and NULL means "whatever the engine allows", so a
-- row can override one limit without restating the others, and a script with no
-- row behaves exactly as it did before this existed.
CREATE TABLE IF NOT EXISTS script_limits (
    script_uri TEXT PRIMARY KEY,
    -- Wall clock for one request-shaped invocation.
    timeout_ms BIGINT,
    -- Wall clock for one scheduled job or queued task. Separate for the same
    -- reason javascript.job_timeout_ms is separate from the request budget: a
    -- job is not answering a request.
    job_timeout_ms BIGINT,
    -- Heap ceiling for this script's runtime.
    max_memory_bytes BIGINT,
    -- Who set it and why, because a limit that differs from the engine's is a
    -- decision somebody made and the next person will want the reason.
    note TEXT,
    set_by TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
