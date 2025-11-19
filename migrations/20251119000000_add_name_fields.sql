-- Add user-friendly name fields to scripts and assets tables
-- These provide human-readable labels for UI display while uri/asset_name remain unique identifiers

-- Add name field to scripts table
ALTER TABLE scripts ADD COLUMN IF NOT EXISTS name TEXT;

-- Auto-populate script names by extracting the last segment from URI
-- Example: "https://example.com/core.js" -> "core.js"
UPDATE scripts 
SET name = CASE 
    WHEN uri LIKE '%/%' THEN substring(uri from '[^/]+$')
    ELSE uri
END
WHERE name IS NULL;

-- Create index on name for UI filtering and sorting
CREATE INDEX IF NOT EXISTS idx_scripts_name ON scripts(name);

-- Add name field to assets table
ALTER TABLE assets ADD COLUMN IF NOT EXISTS name TEXT;

-- Auto-populate asset names from asset_name
-- Users can later update these to more descriptive labels
UPDATE assets 
SET name = asset_name
WHERE name IS NULL;

-- Create index on name for UI filtering and sorting
CREATE INDEX IF NOT EXISTS idx_assets_name ON assets(name);

-- Notes:
-- - name is NOT UNIQUE (allows duplicate display names)
-- - uri/asset_name remain the unique technical identifiers
-- - name can be NULL (optional field for backward compatibility)
-- - names are auto-populated but users can customize them
