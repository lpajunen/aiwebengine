-- What a person has authorised a script to do as them while they are not here.
--
-- Background work that acts as somebody is a real grant, and the engine had no
-- model for it: a scheduled handler runs as UserContext::admin("scheduler"), so
-- it looks for a person's key under a user literally named `scheduler`, misses,
-- and falls back to the script-wide secret. The answer is not to widen that
-- context but to record what was actually consented to.
--
-- Shaped after oauth_client_grants, which answers the same question for an
-- OAuth client: one row per (person, thing they authorised), replaced when they
-- consent again, and revocable by them.
--
-- The expiry is NOT NULL on purpose. A delegation that outlives the tab that
-- created it by an unbounded amount is the thing nobody consented to, so there
-- is no "forever" to choose.
CREATE TABLE IF NOT EXISTS script_delegations (
    user_id TEXT NOT NULL,
    script_uri TEXT NOT NULL,
    -- What was authorised. A fixed vocabulary rather than free text, because
    -- each value has to name something the engine can actually gate on.
    scopes TEXT[] NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    granted_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (user_id, script_uri)
);

CREATE INDEX IF NOT EXISTS idx_script_delegations_user
    ON script_delegations (user_id);

-- Who a task acts as, when it acts as anybody.
--
-- NULL is script context, which is every task that existed before this and
-- remains the default: a task holds what the script holds. A user id here means
-- the task runs as that person, and only for as long as the grant above says
-- so — checked when the task runs, never trusted from this column alone.
ALTER TABLE script_tasks
    ADD COLUMN IF NOT EXISTS run_as TEXT;

CREATE INDEX IF NOT EXISTS idx_script_tasks_run_as
    ON script_tasks (run_as)
    WHERE run_as IS NOT NULL;
