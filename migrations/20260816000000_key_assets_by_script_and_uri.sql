-- Key assets by their owning script as well as their path.
--
-- Assets have belonged to a script since script_uri was added, but the table
-- kept the primary key it was created with: the path alone. Two scripts could
-- therefore never both own `index.html` or `server/utils.ts`, and an upsert of
-- a path another script already owned did not fail — it reassigned that
-- script's asset to the writer, content and all, because the UPDATE matched on
-- the path alone. Every other asset query already scopes by script_uri.
--
-- Widening the key cannot conflict with existing rows: anything unique on
-- (uri) is unique on (script_uri, uri) too.

-- The current key is named after the table it was created on
-- (assets_new_pkey, from the rename in 20251111000000). Look it up rather than
-- trusting that name, so a database whose history differs still migrates.
DO $$
DECLARE
    pk_name text;
BEGIN
    SELECT conname INTO pk_name
    FROM pg_constraint
    WHERE conrelid = 'assets'::regclass AND contype = 'p';

    IF pk_name IS NOT NULL THEN
        EXECUTE format('ALTER TABLE assets DROP CONSTRAINT %I', pk_name);
    END IF;
END $$;

ALTER TABLE assets ADD PRIMARY KEY (script_uri, uri);
