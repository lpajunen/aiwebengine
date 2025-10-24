-- Create assets table
CREATE TABLE IF NOT EXISTS assets (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    public_path TEXT NOT NULL UNIQUE,
    mimetype TEXT NOT NULL,
    content BYTEA NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Create index on public_path for faster lookups
CREATE INDEX idx_assets_public_path ON assets(public_path);

-- Create index on mimetype for filtering
CREATE INDEX idx_assets_mimetype ON assets(mimetype);
