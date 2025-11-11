-- Restructure assets table to use asset_name instead of public_path
-- Assets will now be stored by asset_name only, and public paths will be registered at runtime via registerPublicAsset()

-- Create new assets table structure
CREATE TABLE IF NOT EXISTS assets_new (
    asset_name TEXT PRIMARY KEY,
    mimetype TEXT NOT NULL,
    content BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Migrate data from old table if it exists
-- Use public_path as the asset_name (it was essentially the identifier anyway)
INSERT INTO assets_new (asset_name, mimetype, content, created_at, updated_at)
SELECT public_path, mimetype, content, created_at, updated_at
FROM assets
ON CONFLICT (asset_name) DO NOTHING;

-- Drop old table
DROP TABLE IF EXISTS assets;

-- Rename new table
ALTER TABLE assets_new RENAME TO assets;

-- Create index on mimetype for filtering
CREATE INDEX idx_assets_mimetype ON assets(mimetype);

-- The assets table now has:
-- - asset_name (TEXT primary key) - Unique identifier for the asset (e.g., "logo.svg", "editor.css")
-- - mimetype (TEXT NOT NULL) - MIME type of the asset
-- - content (BYTEA NOT NULL) - Binary content of the asset
-- - created_at (TIMESTAMPTZ NOT NULL)
-- - updated_at (TIMESTAMPTZ NOT NULL)
--
-- Public HTTP paths are registered at runtime using registerPublicAsset(path, asset_name) in init() functions
