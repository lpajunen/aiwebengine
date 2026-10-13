-- How a follow's last run went, in place of whether it went through: a pull
-- refused over a change made here, or by an engine too old for the repository,
-- is not the same as one GitHub did not answer.
ALTER TABLE git_follows
    ADD COLUMN last_state TEXT
        CHECK (last_state IN ('ok', 'blocked', 'incompatible', 'failing', 'paused'));

UPDATE git_follows SET last_state = CASE
    WHEN last_ok THEN 'ok'
    WHEN last_outcome LIKE 'Paused%' THEN 'paused'
    WHEN last_ok = false THEN 'failing'
END;

ALTER TABLE git_follows DROP COLUMN last_ok;
