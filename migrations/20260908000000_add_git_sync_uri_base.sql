-- Where a repository's scripts landed, so that a pull can tell when the answer
-- has changed.
--
-- The up-to-date check asked one question — has the remote moved since we last
-- agreed — and skipped the download when it had not. That is correct about the
-- repository and silent about this end: a change to how a pull composes a
-- script's URI, or to which files it takes, leaves the recorded commit exactly
-- where it was, so the next pull declares itself up to date and does nothing.
-- An engine upgraded across such a change goes on serving what it already had,
-- and nothing in the answer says why.
--
-- Recording the base a pull composed against turns that into something the
-- check can see. A base that no longer matches what this engine would compose
-- now means the mapping moved underneath the sync, and the shortcut does not
-- apply however still the repository has been.
--
-- Nullable, and a NULL deliberately does not match: rows written before this
-- column existed cannot say where they landed, and "cannot confirm" has to
-- behave like "does not match" or the first pull after this migration would
-- take a shortcut it has not earned.
ALTER TABLE script_git_sync ADD COLUMN uri_base TEXT;

COMMENT ON COLUMN script_git_sync.uri_base IS 'Origin and prefix the pull composed this script''s URI against. NULL for rows predating the column, which never match and so never allow the up-to-date shortcut.';
