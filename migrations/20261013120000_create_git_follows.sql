-- What this engine follows from a repository.
--
-- `script_git_sync` has one row per script: where that script last agreed
-- with its repository. What a pull should take is a question about the
-- repository, branch and prefix together, which no one script's row can
-- answer, so it has its own row here.
--
-- A repository holding many scripts can be followed in part. `scripts` lists
-- the top-level directories followed; NULL follows the whole repository,
-- including scripts added to it later.
CREATE TABLE git_follows (
    -- 'owner/repo', canonicalised as in script_git_sync.
    remote     TEXT NOT NULL,
    branch     TEXT NOT NULL,
    -- The prefix the scripts were pulled under; '' for none.
    uri_base   TEXT NOT NULL DEFAULT '',
    scripts    TEXT[],
    created_by TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (remote, branch, uri_base)
);

COMMENT ON TABLE git_follows IS 'Which repository, branch and prefix this engine follows, and which of its scripts: NULL scripts is the whole repository.';
