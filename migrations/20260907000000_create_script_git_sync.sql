-- Where a script's files came from, when they came from a git repository.
--
-- The engine's write surface is an API: an agent calls `write_file` and
-- `write_assets` and the script changes. That is the right shape for one person
-- editing their own solution, and it has no answer for the moment a second
-- person wants the same solution — there is nowhere to publish it to and
-- nothing to pull it from. This table is the other half: a script can name a
-- repository as where its content comes from.
--
-- One row per script rather than per repository, because a repository holding
-- several scripts syncs each of them separately — they are written in separate
-- batches, they initialise separately, and one of them can fail while the rest
-- succeed. The repository is a column, not the key.
CREATE TABLE script_git_sync (
    -- The script this describes. Cascades, because a sync record for a script
    -- that no longer exists describes nothing.
    script_uri       TEXT PRIMARY KEY REFERENCES scripts(uri) ON DELETE CASCADE,
    -- 'owner/repo', canonicalised from whatever form the caller named it in.
    remote           TEXT NOT NULL,
    branch           TEXT NOT NULL,
    -- The commit whose tree was written here.
    --
    -- This is what makes a re-pull cheap: a repository that has not moved is
    -- two small API calls rather than an archive download. It is also the
    -- fixed point a divergence check needs — "has the remote moved since we
    -- last agreed" is unanswerable without recording what we last agreed on.
    last_commit      TEXT NOT NULL,
    -- The revision that write produced, or NULL when the pull changed nothing.
    --
    -- Recorded alongside the commit rather than derived from it, because the
    -- question a push will ask is whether the script has moved *since* the
    -- sync, and that compares revisions rather than commits.
    revision_at_sync INTEGER,
    synced_at        TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- User id of whoever ran the pull. NULL only for a sync with no
    -- authenticated caller, which no current path produces.
    synced_by        TEXT
);

COMMENT ON TABLE script_git_sync IS 'Which repository, branch and commit each git-synced script last agreed with.';

-- Answering "which scripts does this repository own here, and where do they
-- stand" without scanning: the question every pull asks before downloading.
CREATE INDEX idx_script_git_sync_remote ON script_git_sync(remote, branch);
