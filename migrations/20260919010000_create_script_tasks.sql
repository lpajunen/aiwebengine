-- A durable queue of work a script enqueued for itself.
--
-- Distinct from scheduler_jobs, which it deliberately does not extend. That
-- table has UNIQUE(script_uri, job_key) because it models one row per *named
-- declaration*: a script says in init() what it runs on a schedule, and saying
-- it again replaces what it said. A queue is the other thing — a row exists
-- because something happened, it carries what happened as a payload, and two
-- of them are two pieces of work rather than one restatement. Neither the
-- uniqueness nor the wipe-and-redeclare on init() is right for that, so this
-- is its own table and its own lifetime.
CREATE TABLE IF NOT EXISTS script_tasks (
    task_id UUID PRIMARY KEY,
    script_uri TEXT NOT NULL,
    handler_name TEXT NOT NULL,
    -- What the work is about. JSONB rather than text so the column is validated
    -- by Postgres and readable by whoever is debugging the queue.
    payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    -- 'pending' waiting for run_at, 'running' claimed by a worker, 'failed'
    -- out of attempts, 'cancelled' stopped by a person. A task that succeeds
    -- has no state because its row is gone: what it did is in the script's log
    -- under its own invocation id, and keeping a row per success would make
    -- this table grow without bound for the one outcome nobody is debugging.
    state TEXT NOT NULL DEFAULT 'pending'
        CHECK (state IN ('pending', 'running', 'failed', 'cancelled')),
    attempts INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL,
    -- Why the last attempt failed, so a failed task explains itself without
    -- anyone having to correlate timestamps against the script's log.
    last_error TEXT,
    run_at TIMESTAMPTZ NOT NULL,
    locked_by TEXT,
    locked_at TIMESTAMPTZ,
    lock_expires_at TIMESTAMPTZ,
    -- Who enqueued it. A task runs in script context, so this is a record of
    -- where the work came from rather than an authority it runs under.
    enqueued_by TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- The worker's claim query: pending rows that are due, oldest first.
CREATE INDEX IF NOT EXISTS idx_script_tasks_claimable
    ON script_tasks (run_at)
    WHERE state IN ('pending', 'running');

-- Listing and cancelling are both per script.
CREATE INDEX IF NOT EXISTS idx_script_tasks_script_uri
    ON script_tasks (script_uri, created_at DESC);
