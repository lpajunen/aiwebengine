-- Which directory of the repository a script is.
--
-- A pulled script used to be found again by *inverting its URI*: the URI was
-- `{base}/{directory}.{ext}`, so what followed the base said which directory it
-- came from. A script's name is now its own — it can be renamed, and was never
-- going to contain a directory once it stopped being a URL — so the directory
-- is recorded instead of being read back out of the name.
--
-- '' is the repository root (the repository is itself the script); NULL is a
-- row that has never been resolved, which no pull matches.

ALTER TABLE script_git_sync ADD COLUMN IF NOT EXISTS repo_dir TEXT;

COMMENT ON COLUMN script_git_sync.repo_dir IS 'The repository directory this script is: empty for the repository root, NULL for a row not yet resolved. Recorded so a renamed script is still found by the pull that wrote it.';

-- Rows written under the URI composition: the base is on record, so the
-- directory is whatever followed it, without the extension.
UPDATE script_git_sync
   SET repo_dir = CASE
         WHEN starts_with(script_uri, uri_base || '/')
           THEN regexp_replace(substr(script_uri, length(uri_base) + 2), '\.(tsx|jsx|ts|js)$', '')
         WHEN starts_with(script_uri, uri_base)
           THEN ''
       END
 WHERE repo_dir IS NULL AND uri_base IS NOT NULL;
