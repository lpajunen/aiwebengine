-- How many times a job has been run and failed.
--
-- A one-off that failed used to be requeued two seconds later with nothing
-- counting the requeues, so a job failing on something that would never change
-- retried for as long as the engine was up. Counting the attempts is what lets
-- the worker stop, and what lets the retry wait longer each time.
--
-- Reset to zero whenever a job is registered again: a re-registration is a new
-- piece of work that happens to reuse the key, not a continuation of the one
-- that was failing.
ALTER TABLE scheduler_jobs
    ADD COLUMN IF NOT EXISTS attempts INTEGER NOT NULL DEFAULT 0;
