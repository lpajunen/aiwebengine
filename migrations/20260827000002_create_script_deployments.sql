-- What a script serves, when that is not simply its newest revision.
--
-- Until now writing a script's files *was* deploying them: there was nothing
-- between the write and what answered the next request. That is the right
-- default for one person editing their own script, and the wrong one as soon
-- as an agent is editing modules other people are using — every experiment
-- goes straight to production because there is nowhere else for it to go.
--
-- A row here pins a script to a revision. Writes still record revisions and
-- still advance head; they just stop being deployments. What is served changes
-- when somebody says so.
--
-- No row means follow head, which is exactly what every script does today, so
-- nothing changes for anyone who does not pin. That is deliberate: this is a
-- capability to opt into, not a workflow to impose.
CREATE TABLE script_deployments (
    script_uri  TEXT PRIMARY KEY REFERENCES scripts(uri) ON DELETE CASCADE,
    revision    INTEGER NOT NULL,
    deployed_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- User id of whoever deployed it, or NULL when the engine did.
    deployed_by TEXT,
    -- How init() went for this revision when it was deployed. The revision's
    -- own init_ok records how it went when it was *written*, which is a
    -- different event and can have a different answer — a revision can be
    -- written on an instance whose secrets or tables differ from the one it is
    -- later deployed on.
    init_ok     BOOLEAN,
    init_error  TEXT,

    -- The revision has to be one this script actually has.
    CONSTRAINT fk_script_deployments_revision
        FOREIGN KEY (script_uri, revision)
        REFERENCES script_revisions (script_uri, revision)
        ON DELETE RESTRICT
);

COMMENT ON TABLE script_deployments IS 'Pins a script to the revision it serves. No row means it serves its newest revision. Retention excludes a pinned revision from collection; the foreign key is the backstop for anything that tries anyway.';
COMMENT ON COLUMN script_deployments.revision IS 'The revision this script serves. Writes advance head without changing this.';
