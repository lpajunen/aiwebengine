-- A script's identifier becomes a name that can change.
--
-- `scripts.uri` is the natural key every other table points at, so it could
-- never be anything but permanent: nineteen tables name a script by it, and
-- only seven of them said so with a foreign key. The other twelve held the
-- string and trusted nothing to disagree with it, which is also why deleting a
-- script left rows behind in some of them.
--
-- Every table that names a script now references it, and every reference
-- follows a rename (`ON UPDATE CASCADE`), so renaming a script is one
-- `UPDATE scripts SET uri = ...` and the rest follows in the same statement.
--
-- Two things stay as they are on purpose:
--
-- * `logs.script_uri`. The engine writes its own log lines under `server`,
--   which is not a script, so a foreign key there would refuse them. A rename
--   moves a script's lines explicitly instead.
-- * `script_tables.physical_table_name`. The physical name is stored and read
--   back, never recomputed from the URI, so a renamed script keeps its tables
--   under the names they already have. What changes is how a *new* table is
--   named: from `scripts.id`, which does not change (see the repository).

-- ---------------------------------------------------------------------------
-- A name that does not change
-- ---------------------------------------------------------------------------
--
-- `scripts.id` existed in the first migration and was dropped when `uri` became
-- the primary key. It comes back as the one thing about a script that is not a
-- name: physical table names are derived from it, so a script that is renamed
-- and a different script that later takes the old name cannot hash to the same
-- table.

ALTER TABLE scripts
    ADD COLUMN IF NOT EXISTS id UUID NOT NULL DEFAULT gen_random_uuid();
CREATE UNIQUE INDEX IF NOT EXISTS scripts_id_key ON scripts (id);

-- ---------------------------------------------------------------------------
-- The seven that already referenced `scripts(uri)`: follow a rename
-- ---------------------------------------------------------------------------

DO $$
DECLARE
    fk record;
BEGIN
    FOR fk IN
        SELECT conrelid::regclass AS tbl, conname
        FROM pg_constraint
        WHERE contype = 'f' AND confrelid = 'scripts'::regclass
    LOOP
        EXECUTE format('ALTER TABLE %s DROP CONSTRAINT %I', fk.tbl, fk.conname);
    END LOOP;
END $$;

-- ---------------------------------------------------------------------------
-- And the rest, after removing what belongs to a script that is gone
-- ---------------------------------------------------------------------------
--
-- A row naming a script that does not exist is not reachable by anything: no
-- script can read it, and a script created later under the same name would
-- inherit it. Removing them is what the foreign key would otherwise refuse.

DO $$
DECLARE
    tbl text;
BEGIN
    FOREACH tbl IN ARRAY ARRAY[
        'assets', 'script_hosts', 'script_owners', 'script_revisions',
        'script_deployments', 'script_git_sync', 'script_tables',
        'mcp_tasks', 'scheduler_jobs', 'script_channel_identities',
        'script_channel_link_tokens', 'script_delegations', 'script_limits',
        'script_properties', 'script_secrets', 'script_tasks',
        'user_properties', 'user_secrets'
    ]
    LOOP
        EXECUTE format(
            'DELETE FROM %I WHERE script_uri NOT IN (SELECT uri FROM scripts)', tbl);
        EXECUTE format(
            'ALTER TABLE %I ADD CONSTRAINT %I FOREIGN KEY (script_uri) '
            'REFERENCES scripts(uri) ON UPDATE CASCADE ON DELETE CASCADE',
            tbl, tbl || '_script_uri_fkey');
    END LOOP;
END $$;
