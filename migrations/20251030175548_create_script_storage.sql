-- Create script_storage table for persistent key-value storage per script
CREATE TABLE IF NOT EXISTS script_storage (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    script_uri TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(script_uri, key)
);

-- Create index on script_uri for faster lookups
CREATE INDEX idx_script_storage_script_uri ON script_storage(script_uri);

-- Create index on key for faster key-based queries
CREATE INDEX idx_script_storage_key ON script_storage(key);

-- Create composite index for script-specific key lookups
CREATE INDEX idx_script_storage_script_uri_key ON script_storage(script_uri, key);

-- Create index on updated_at for cleanup operations
CREATE INDEX idx_script_storage_updated_at ON script_storage(updated_at DESC);