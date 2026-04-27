CREATE TABLE IF NOT EXISTS scheduler_jobs (
    job_id UUID PRIMARY KEY,
    script_uri TEXT NOT NULL,
    handler_name TEXT NOT NULL,
    job_key TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('one_off', 'recurring')),
    run_at TIMESTAMPTZ NOT NULL,
    interval_ms BIGINT,
    locked_by TEXT,
    locked_at TIMESTAMPTZ,
    lock_expires_at TIMESTAMPTZ,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(script_uri, job_key)
);

CREATE INDEX IF NOT EXISTS idx_scheduler_jobs_due
    ON scheduler_jobs (run_at);

CREATE INDEX IF NOT EXISTS idx_scheduler_jobs_lock_expires
    ON scheduler_jobs (lock_expires_at);

CREATE INDEX IF NOT EXISTS idx_scheduler_jobs_script_uri
    ON scheduler_jobs (script_uri);
