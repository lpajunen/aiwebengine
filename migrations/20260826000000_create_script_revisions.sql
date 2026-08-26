-- Record what a script's files were, every time they change.
--
-- Until now a write replaced the rows it touched and the previous content was
-- gone. An edit made through `/engine/assets` — increasingly made by an agent
-- with no checkout to fall back on — had nothing behind it to return to.
--
-- The unit recorded here is the *script*, not the file. An asset has no
-- identity apart from its script (the primary key is (script_uri, uri) and the
-- foreign key cascades), one write already stores several files in a single
-- transaction, and the changes worth undoing span modules. Per-file history is
-- a query over these manifests; the reverse does not hold, because a manifest
-- cannot be reassembled from independent per-file logs without guessing which
-- versions were current together.

-- Content, addressed by digest and stored once however many revisions cite it.
--
-- This is what makes a full manifest per revision affordable: a revision of a
-- forty-file script that changed one module adds one blob and forty narrow
-- rows, not forty copies of the content.
CREATE TABLE asset_blobs (
    sha256  TEXT PRIMARY KEY,
    bytes   INTEGER NOT NULL,
    content BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

COMMENT ON TABLE asset_blobs IS 'Content-addressed storage shared by every revision that cites a digest. Rows are never updated; a digest always names the same bytes.';
COMMENT ON COLUMN asset_blobs.bytes IS 'Length of content, denormalised so a manifest listing can report sizes without reading the blobs.';

-- One revision of one script: its root source plus the manifest below.
CREATE TABLE script_revisions (
    id          BIGSERIAL PRIMARY KEY,
    script_uri  TEXT NOT NULL REFERENCES scripts(uri) ON DELETE CASCADE,
    -- Per script, starting at 1. Callers say "revision 41 of myapp"; a global
    -- id would make them carry a number that means nothing on its own.
    revision    INTEGER NOT NULL,
    -- The revision this one was computed against, or NULL for the first.
    -- Not derivable as `revision - 1`: a revert records the revision it
    -- restored from, which is how a history reads as a graph rather than a
    -- line that silently doubles back.
    parent      INTEGER,
    root_sha256 TEXT NOT NULL REFERENCES asset_blobs(sha256),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- User id of the writer, or NULL for writes with no authenticated caller
    -- (startup bootstrap, migrations).
    created_by  TEXT,
    -- How the revision was made: post | batch | patch | delete | script |
    -- revert | bootstrap. Kept as free text rather than an enum so a new
    -- write path does not need a migration to record itself honestly.
    origin      TEXT NOT NULL,
    -- Applied after the fact, by whoever decides this revision was worth
    -- naming. A snapshot API that must be called *before* a risky change is
    -- one you remember only after the change went wrong.
    label       TEXT,
    -- Whether the script's init() succeeded on the write that produced this
    -- revision. This is what git cannot offer: the newest revision that
    -- initialised cleanly is a rollback target the engine can name itself.
    init_ok     BOOLEAN,
    init_error  TEXT,

    UNIQUE (script_uri, revision),
    -- A label names one revision per script, so "restore the labelled one" has
    -- a single answer. Postgres allows repeated NULLs, so unlabelled revisions
    -- are unconstrained.
    UNIQUE (script_uri, label)
);

CREATE INDEX idx_script_revisions_script ON script_revisions (script_uri, revision DESC);

COMMENT ON TABLE script_revisions IS 'Numbered revisions of a script, one per write. Deleting a script deletes its history, as it deletes the script''s assets and tables.';

-- The files a revision consisted of, in full.
--
-- Full manifest rather than a delta against the parent: reading a revision is
-- then one join with no replay, and a file that a revision does not contain is
-- represented by its absence rather than by a tombstone the reader has to
-- interpret. Deltas would save rows that already cost little, at the price of
-- making every read depend on the integrity of every earlier revision.
CREATE TABLE script_revision_files (
    revision_id BIGINT NOT NULL REFERENCES script_revisions(id) ON DELETE CASCADE,
    uri         TEXT NOT NULL,
    sha256      TEXT NOT NULL REFERENCES asset_blobs(sha256),
    mimetype    TEXT NOT NULL,
    name        TEXT,

    PRIMARY KEY (revision_id, uri)
);

-- Answers "which revisions contained this exact content", which is what a
-- per-file history and a blob garbage collector both need.
CREATE INDEX idx_script_revision_files_sha ON script_revision_files (sha256);

COMMENT ON TABLE script_revision_files IS 'Complete file manifest of one revision. Content lives in asset_blobs; these rows carry only the identity of each file.';

-- Retention is deliberately not implemented here. Every write making a
-- revision is a slow leak without one, and the policy (keep labelled
-- revisions, keep the newest that initialised cleanly, keep N days, then GC
-- unreferenced blobs) belongs with the code that can enforce all four
-- conditions together rather than in a trigger that sees one row at a time.
