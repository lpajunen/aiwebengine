-- A script table may name the column that holds a person's account id, and an
-- account's erasure deletes that person's rows from every table that does.
--
-- Erasure deletes rows other rows may point at, so a reference column's
-- foreign key gets a rule for that: an optional reference is emptied
-- (ON DELETE SET NULL), a required one goes with the row it needs
-- (ON DELETE CASCADE). New reference columns are created that way; this
-- rewrites the ones that exist, keeping each constraint's name.

ALTER TABLE script_tables ADD COLUMN IF NOT EXISTS person_column TEXT;

COMMENT ON COLUMN script_tables.person_column IS 'The column holding a person''s account id, whose rows are erased with the account; NULL when the table names none.';

DO $$
DECLARE
    fk RECORD;
BEGIN
    FOR fk IN
        SELECT c.conname,
               src.relname AS source_table,
               dst.relname AS referenced_table,
               a.attname AS column_name,
               a.attnotnull AS required
        FROM pg_constraint c
        JOIN pg_class src ON src.oid = c.conrelid
        JOIN pg_class dst ON dst.oid = c.confrelid
        JOIN pg_attribute a ON a.attrelid = c.conrelid AND a.attnum = c.conkey[1]
        WHERE c.contype = 'f'
          AND array_length(c.conkey, 1) = 1
          AND src.relname IN (SELECT physical_table_name FROM script_tables)
          AND dst.relname IN (SELECT physical_table_name FROM script_tables)
    LOOP
        EXECUTE format('ALTER TABLE %I DROP CONSTRAINT %I', fk.source_table, fk.conname);
        EXECUTE format(
            'ALTER TABLE %I ADD CONSTRAINT %I FOREIGN KEY (%I) REFERENCES %I (id) ON DELETE %s',
            fk.source_table,
            fk.conname,
            fk.column_name,
            fk.referenced_table,
            CASE WHEN fk.required THEN 'CASCADE' ELSE 'SET NULL' END
        );
    END LOOP;
END $$;
