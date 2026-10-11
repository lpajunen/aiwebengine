-- A claim's fencing epoch. Every claim of a row increments it, and everything
-- the claimant writes afterwards (renewal, success, failure) is guarded by the
-- value it claimed with. `locked_by` names an instance, which can claim the
-- same row twice; the epoch names one claim.
ALTER TABLE script_tasks ADD COLUMN IF NOT EXISTS claim_epoch BIGINT NOT NULL DEFAULT 0;
ALTER TABLE scheduler_jobs ADD COLUMN IF NOT EXISTS claim_epoch BIGINT NOT NULL DEFAULT 0;
