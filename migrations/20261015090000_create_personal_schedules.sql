-- A schedule that belongs to a person: "every morning at seven, as me".
--
-- A row says *when*. When it is due, the engine inserts an ordinary delegated
-- task into `script_tasks`, and from there it is what `personalTasks.enqueue`
-- already gives: the person's lane, retries, the job budget, and the grant read
-- again when it runs.
--
-- Its own table rather than a recurring flag on `script_tasks`: a schedule is a
-- restatable declaration, unique per key, and a task is a piece of work, where
-- two are two. Mixed, one person's "every morning" could become two.
--
-- The foreign keys do the cleanup: deleting the account or the script removes
-- the schedules, and renaming a script carries them.
CREATE TABLE IF NOT EXISTS personal_schedules (
    schedule_id UUID PRIMARY KEY,
    script_uri TEXT NOT NULL
        REFERENCES scripts(uri) ON UPDATE CASCADE ON DELETE CASCADE,
    user_id TEXT NOT NULL
        REFERENCES users(user_id) ON DELETE CASCADE,
    schedule_key TEXT NOT NULL,
    handler_name TEXT NOT NULL,
    payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    -- Exactly one of the two.
    interval_ms BIGINT,
    cron TEXT,
    time_zone TEXT,
    lane TEXT,
    next_run_at TIMESTAMPTZ NOT NULL,
    last_run_at TIMESTAMPTZ,
    -- The task the last firing created, to skip a slot while it is unfinished.
    last_task_id UUID,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    state TEXT NOT NULL DEFAULT 'active' CHECK (state IN ('active', 'paused')),
    paused_reason TEXT CHECK (paused_reason IN ('expired', 'failing', 'person')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (script_uri, user_id, schedule_key),
    CHECK ((interval_ms IS NULL) <> (cron IS NULL)),
    CHECK ((cron IS NULL) = (time_zone IS NULL)),
    CHECK ((state = 'paused') = (paused_reason IS NOT NULL))
);

CREATE INDEX IF NOT EXISTS idx_personal_schedules_due
    ON personal_schedules (next_run_at) WHERE state = 'active';

CREATE INDEX IF NOT EXISTS idx_personal_schedules_user
    ON personal_schedules (user_id);

-- Which schedule a task came from, so its outcome can be written back, and the
-- slot it was fired for, which retries do not move as they move `run_at`.
ALTER TABLE script_tasks
    ADD COLUMN IF NOT EXISTS schedule_id UUID
        REFERENCES personal_schedules(schedule_id) ON DELETE SET NULL,
    ADD COLUMN IF NOT EXISTS scheduled_for TIMESTAMPTZ;
