-- A follow can run on its own, every so many minutes.
--
-- It acts as the account that set the schedule (`acting_user`), re-read on
-- every run and narrowed to writing scripts that account owns. Anything that
-- changes what that account may do pauses it, the way it withdraws the
-- account's delegations. NULL `every_minutes` is a follow only pulled by hand.
--
-- `mode` is what a run does: 'check' records whether the repository has moved,
-- 'stage' pulls while every followed script keeps serving its current
-- revision, 'apply' pulls and serves what it pulled.

ALTER TABLE git_follows
    ADD COLUMN every_minutes INTEGER,
    ADD COLUMN mode          TEXT NOT NULL DEFAULT 'apply',
    ADD COLUMN acting_user   TEXT,
    ADD COLUMN next_due_at   TIMESTAMPTZ,
    ADD COLUMN last_run_at   TIMESTAMPTZ,
    ADD COLUMN last_ok       BOOLEAN,
    ADD COLUMN last_outcome  TEXT,
    ADD CONSTRAINT git_follows_mode CHECK (mode IN ('check', 'stage', 'apply')),
    ADD CONSTRAINT git_follows_scheduled_acts_for_someone
        CHECK (every_minutes IS NULL OR (every_minutes > 0 AND acting_user IS NOT NULL));

COMMENT ON COLUMN git_follows.every_minutes IS 'How often the follow runs on its own; NULL when it is only pulled by hand.';
COMMENT ON COLUMN git_follows.acting_user IS 'The account a scheduled run acts as; NULL when nothing is scheduled.';

-- The question the runner asks every minute.
CREATE INDEX idx_git_follows_due ON git_follows (next_due_at) WHERE every_minutes IS NOT NULL;
