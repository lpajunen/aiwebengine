-- A sync and a follow name a ref, not only a branch: a branch, a tag or one
-- commit, stored as git spells it (refs/heads/<name>, refs/tags/<name>, or a
-- full sha).
--
-- And an engine follows a repository once. The ref and the prefix are what the
-- follow is at, not what it is, so pulling another ref moves the follow there
-- rather than leaving the old one to pull its ref back over the new one.

ALTER TABLE script_git_sync RENAME COLUMN branch TO git_ref;
UPDATE script_git_sync SET git_ref = 'refs/heads/' || git_ref;

COMMENT ON COLUMN script_git_sync.git_ref IS 'The ref last synced: refs/heads/<branch>, refs/tags/<tag>, or a full commit sha.';

DROP INDEX IF EXISTS idx_script_git_sync_remote;
CREATE INDEX idx_script_git_sync_remote ON script_git_sync(remote);

ALTER TABLE git_follows RENAME COLUMN branch TO git_ref;
UPDATE git_follows SET git_ref = 'refs/heads/' || git_ref;

-- Where one repository was followed at several branches or prefixes, keep the
-- scheduled follow, else the newest.
DELETE FROM git_follows
 WHERE ctid IN (
     SELECT ctid FROM (
         SELECT ctid,
                row_number() OVER (
                    PARTITION BY remote
                    ORDER BY (every_minutes IS NOT NULL) DESC, updated_at DESC
                ) AS n
           FROM git_follows
     ) ranked
     WHERE n > 1
 );

ALTER TABLE git_follows DROP CONSTRAINT git_follows_pkey;
ALTER TABLE git_follows ADD PRIMARY KEY (remote);

COMMENT ON TABLE git_follows IS 'The repositories this engine follows, one row each: at which ref, under which prefix, and which of its scripts (NULL scripts is the whole repository).';
COMMENT ON COLUMN git_follows.git_ref IS 'The ref followed: refs/heads/<branch>, refs/tags/<tag>, or a full commit sha.';
