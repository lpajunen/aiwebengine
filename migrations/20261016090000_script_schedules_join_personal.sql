-- Script schedules move onto the path personal schedules already take.
--
-- One table: a row with a user_id belongs to that person and runs as them; a
-- row without one belongs to the script, is declared in init(), and runs in
-- script context. Firing is the same for both — a due row becomes a task in
-- script_tasks — so scheduler_jobs, which claimed and ran its own rows, goes.
-- Its rows are not carried over: every script's init() declares its schedules
-- again at startup.
ALTER TABLE personal_schedules RENAME TO schedules;
ALTER INDEX idx_personal_schedules_due RENAME TO idx_schedules_due;
ALTER INDEX idx_personal_schedules_user RENAME TO idx_schedules_user;

ALTER TABLE schedules ALTER COLUMN user_id DROP NOT NULL;

-- One key per owner, the script counting as one owner: NULL is not distinct.
ALTER TABLE schedules
    DROP CONSTRAINT personal_schedules_script_uri_user_id_schedule_key_key;
ALTER TABLE schedules
    ADD CONSTRAINT schedules_owner_key
        UNIQUE NULLS NOT DISTINCT (script_uri, user_id, schedule_key);

-- Nobody can resume a script's schedule but its code, so it never pauses.
ALTER TABLE schedules
    ADD CONSTRAINT schedules_only_personal_pause
        CHECK (user_id IS NOT NULL OR state = 'active');

-- When init() last declared a script's schedule. What an init() did not
-- declare is removed after it, so a schedule it keeps keeps its next slot
-- across restarts and deployments instead of starting over each time.
ALTER TABLE schedules ADD COLUMN IF NOT EXISTS declared_at TIMESTAMPTZ;

DROP TABLE IF EXISTS scheduler_jobs;
