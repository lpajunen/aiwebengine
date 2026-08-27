-- Attribute a log line to the revision of the script that emitted it.
--
-- A log row already knows which invocation produced it — `request_id`, `kind`,
-- `route`. What it could not say is which *version* of the script was running,
-- and that is the question an operator actually has when something starts
-- failing: not "which request", but "did this begin when I deployed?".
--
-- Answering it by wall clock means eyeballing a deploy time against a burst of
-- errors and hoping the two line up. With the revision on the row, "revision 41
-- is when the 500s started" is a query, and it is what turns a rollback from
-- something you can do into something you can decide to do.
--
-- Nullable, because plenty of lines legitimately have no revision: those
-- written before this migration, engine-internal output with no script behind
-- it, and anything logged for a script whose history has not been read yet.
ALTER TABLE logs ADD COLUMN revision INTEGER;

-- Filtering a script's log down to one revision is the whole point, and the
-- listing already orders by seq. Partial, since most rows will carry no
-- revision until scripts are written again.
CREATE INDEX idx_logs_script_revision ON logs (script_uri, revision, seq)
    WHERE revision IS NOT NULL;

COMMENT ON COLUMN logs.revision IS 'Revision of the script that was running when the line was written, as this instance understood it. Null when there is no script revision to attribute the line to.';
