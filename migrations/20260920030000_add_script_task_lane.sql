-- Which tasks must not run beside each other.
--
-- Claiming is `FOR UPDATE SKIP LOCKED` across whatever is due, and the worker
-- spawns each claimed run rather than awaiting it, so two tasks enqueued a
-- moment apart run at the same time. For most work that is the point. For
-- work that belongs to one person it is a correctness bug: two prompts to an
-- agent become two runs interleaving turn for turn, each reading and
-- overwriting the same personalStorage and the same notes.
--
-- A script can work around it with a claimed-status table of its own, and
-- some do. But every script queueing per-person work has the same bug, and
-- serialising work that shares state belongs to the queue rather than to each
-- of its callers.
--
-- NULL means no lane, which is every task that existed before this and
-- remains the default for `scriptTasks`: unconstrained, claimed and run
-- alongside anything else. A lane is per script, like a job key, because two
-- solutions both using the lane "inbox" are not talking about the same thing.
ALTER TABLE script_tasks
    ADD COLUMN IF NOT EXISTS lane TEXT;

-- What the claim asks: is anything in this lane running *right now*. Partial
-- on the state, because a lane is only blocked by a live run — a row left
-- 'running' by a worker that died has a lapsed lease, is claimable again, and
-- must not hold its lane shut in the meantime.
--
-- `lock_expires_at` is in the index so the claim's freshness test is answered
-- from it rather than by visiting the heap.
CREATE INDEX IF NOT EXISTS idx_script_tasks_lane_running
    ON script_tasks (script_uri, lane, lock_expires_at)
    WHERE state = 'running' AND lane IS NOT NULL;
