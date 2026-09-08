-- Which version of the URI composition a sync row was written by.
--
-- `uri_base` already catches a pull that would land somewhere else entirely.
-- It does not catch a change to how the *last segment* is built, because the
-- base is the same either way — and that is exactly the change being made
-- alongside this migration, where a script stops being named `main.js` and
-- takes the name of the directory or repository it came from.
--
-- Without this the shortcut would look at an unmoved commit and an unchanged
-- base, declare itself up to date, and leave every already-synced script under
-- its old name with nothing in the answer to say why. That is the second time
-- the same shape of bug would have shipped, so the fix is a version rather
-- than another special case: composition changes, the constant goes up, and
-- every row written before it stops matching.
--
-- NULL for rows predating the column, which never match, so the first pull
-- after this migration does the work.
ALTER TABLE script_git_sync ADD COLUMN mapping_version INTEGER;

COMMENT ON COLUMN script_git_sync.mapping_version IS 'Version of the URI composition rules this row was written by. A pull whose rules differ does not take the up-to-date shortcut.';
