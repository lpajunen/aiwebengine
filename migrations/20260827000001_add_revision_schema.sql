-- What a script's tables looked like when a revision was current.
--
-- A revision records code, and only code. Reverting the modules that read a
-- table does not un-add the column, and a revert that dropped the column to
-- match would destroy data in order to restore code — so the engine must not
-- do that. What it can do is notice, and say so, which needs knowing what the
-- schema was.
--
-- Shape: an object keyed by the script's logical table name, each value the
-- `schema_json` that table carried at the time:
--
--   {"matches": {"columns": [{"name": "id", "type": "SERIAL", ...}]}}
--
-- Nullable, because revisions recorded before this column existed have no
-- fingerprint and a comparison against them can only say so.
ALTER TABLE script_revisions ADD COLUMN tables JSONB;

COMMENT ON COLUMN script_revisions.tables IS 'The script''s table schemas as they stood when this revision was recorded, keyed by logical table name. Advisory: reverting code never migrates data back.';
