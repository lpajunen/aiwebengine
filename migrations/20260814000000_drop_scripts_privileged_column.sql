-- Drop the per-script privileged flag.
--
-- All scripts are equal: every script sees the same engine-internal API
-- surface, and each call is authorized against the calling user (their
-- capabilities, ownership of the target script via script_owners, and their
-- role). The column has been unread and unwritten since the engine stopped
-- gating secretStorage's *ForUri functions on it.
--
-- Added by 20241114000003_add_privileged_to_scripts.sql.
ALTER TABLE IF EXISTS scripts
DROP COLUMN IF EXISTS privileged;
