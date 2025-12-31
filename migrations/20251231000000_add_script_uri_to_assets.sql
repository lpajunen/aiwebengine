-- Add script_uri column to assets table to link assets to their owning scripts
-- Default to core script for existing and bootstrap assets

ALTER TABLE assets ADD COLUMN script_uri TEXT NOT NULL DEFAULT 'https://example.com/core';

-- Add foreign key constraint to ensure script_uri references a valid script
ALTER TABLE assets ADD CONSTRAINT fk_assets_script_uri FOREIGN KEY (script_uri) REFERENCES scripts(uri) ON DELETE CASCADE;

-- Create index on script_uri for efficient queries
CREATE INDEX idx_assets_script_uri ON assets(script_uri);