-- Rename asset_name column to uri in assets table for consistency with scripts table
-- Both tables now use 'uri' as the unique technical identifier

ALTER TABLE assets RENAME COLUMN asset_name TO uri;

-- Update index name for consistency
DROP INDEX IF EXISTS idx_assets_mimetype;
CREATE INDEX IF NOT EXISTS idx_assets_uri ON assets(uri);
CREATE INDEX IF NOT EXISTS idx_assets_mimetype ON assets(mimetype);

-- The assets table now has:
-- - uri (TEXT primary key) - Unique identifier for the asset (e.g., "logo.svg", "editor.css")
-- - name (TEXT) - Optional user-friendly display name (e.g., "Company Logo")
-- - mimetype (TEXT NOT NULL) - MIME type of the asset
-- - content (BYTEA NOT NULL) - Binary content of the asset
-- - created_at (TIMESTAMPTZ NOT NULL)
-- - updated_at (TIMESTAMPTZ NOT NULL)
