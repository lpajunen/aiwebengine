-- A script and its assets become one tree.
--
-- `scripts.code` (later `scripts.content`) has been a column since the first
-- migration, while a script's other files are rows in `assets` keyed
-- `(script_uri, uri)`. One schema decision, two of everything above it: two
-- halves of every read, write, patch and listing surface, a `write_assets`
-- that has to carry "and also the root" as a special case, and a `git_sync`
-- that flattens the two into a directory on the way out and re-splits them on
-- the way in. The history carries the split too — `script_revisions.root_sha256`
-- sits beside `script_revision_files` as a distinct thing rather than as one
-- entry in the manifest.
--
-- The root becomes the file named `main.<ext>` in the tree, which is the name
-- a repository already gives it: `git_sync::ENTRY_NAMES` has been
-- `main.{ts,js,tsx,jsx}` since it shipped, so a pulled repository and what the
-- engine stores now agree rather than being translated between.
--
-- `scripts` keeps what it is for: identity, display name, ownership (through
-- the foreign keys pointing at `uri`) and init status. It stops holding
-- content.

-- ---------------------------------------------------------------------------
-- The root's file name
-- ---------------------------------------------------------------------------
--
-- Carried over from the script URI's last segment, because that extension is
-- load-bearing: `transpiler::needs_transpilation` reads it and never reads the
-- stem, so a TypeScript root landing under a `.js` name would reach the
-- runtime untranspiled. Anything that is not one of the four extensions the
-- bundler distinguishes becomes `main.js`, which is what those scripts already
-- were — `needs_transpilation` is true for `.ts`, `.tsx` and `.jsx` alone, so
-- `core`, `x.mjs` and `x.cjs` all already ran as plain JavaScript.
CREATE OR REPLACE FUNCTION pg_temp.root_file_name(script_uri text)
RETURNS text AS $$
    SELECT CASE lower(coalesce(substring(script_uri from '\.([A-Za-z0-9]+)$'), ''))
        WHEN 'ts'  THEN 'main.ts'
        WHEN 'tsx' THEN 'main.tsx'
        WHEN 'jsx' THEN 'main.jsx'
        ELSE 'main.js'
    END
$$ LANGUAGE sql IMMUTABLE;

CREATE OR REPLACE FUNCTION pg_temp.root_mimetype(file_name text)
RETURNS text AS $$
    SELECT CASE
        WHEN file_name LIKE '%.ts' OR file_name LIKE '%.tsx' THEN 'text/typescript'
        ELSE 'text/javascript'
    END
$$ LANGUAGE sql IMMUTABLE;

-- ---------------------------------------------------------------------------
-- Move the content
-- ---------------------------------------------------------------------------
--
-- `DO UPDATE` rather than `DO NOTHING` on the conflict. A script that already
-- owns an asset literally named `main.ts` has two candidate roots, and only
-- one of them is the file the engine has been executing. The column wins,
-- because that is the version every request has been served; the asset that
-- shared the name was reachable only as `import "./main.ts"`, which is a
-- module importing what is about to become itself.
INSERT INTO assets (script_uri, uri, name, mimetype, content, created_at, updated_at)
SELECT s.uri,
       pg_temp.root_file_name(s.uri),
       pg_temp.root_file_name(s.uri),
       pg_temp.root_mimetype(pg_temp.root_file_name(s.uri)),
       convert_to(s.content, 'UTF8'),
       s.created_at,
       s.updated_at
FROM scripts s
ON CONFLICT (script_uri, uri) DO UPDATE
SET content = EXCLUDED.content,
    mimetype = EXCLUDED.mimetype,
    updated_at = EXCLUDED.updated_at;

-- ---------------------------------------------------------------------------
-- Move the history
-- ---------------------------------------------------------------------------
--
-- Every stored revision gains a manifest entry for its root, so a revision
-- read back after this migration describes the same tree it described before
-- it. The blob is already in `asset_blobs` — `root_sha256` referenced it — so
-- nothing is copied, only named.
--
-- `WHERE NOT EXISTS` guards the same collision as above: a revision of a
-- script that held both a root and an asset called `main.ts` already has a
-- manifest row under that path, and the manifest row is the one whose digest
-- the revision's own files agree with.
INSERT INTO script_revision_files (revision_id, uri, sha256, mimetype, name)
SELECT r.id,
       pg_temp.root_file_name(r.script_uri),
       r.root_sha256,
       pg_temp.root_mimetype(pg_temp.root_file_name(r.script_uri)),
       pg_temp.root_file_name(r.script_uri)
FROM script_revisions r
WHERE NOT EXISTS (
    SELECT 1 FROM script_revision_files f
    WHERE f.revision_id = r.id
      AND f.uri = pg_temp.root_file_name(r.script_uri)
);

-- ---------------------------------------------------------------------------
-- Drop the split
-- ---------------------------------------------------------------------------

-- `idx_script_revisions_root_sha` exists to let the blob collector ask which
-- revisions cite a digest as their root. That is now the same question it asks
-- of every other file, answered by `idx_script_revision_files_sha`, so the
-- index goes with the column that Postgres drops it along with.
ALTER TABLE script_revisions DROP COLUMN root_sha256;

ALTER TABLE scripts DROP COLUMN content;
