-- The repository's ignore rules as of a script's last sync, so the engine can
-- tell the files it keeps for itself from changes made here without fetching
-- the repository.
ALTER TABLE script_git_sync ADD COLUMN ignore_rules TEXT NOT NULL DEFAULT '';
