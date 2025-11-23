-- Create personal_storage table for persistent key-value storage per script per user
CREATE TABLE IF NOT EXISTS personal_storage (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    script_uri TEXT NOT NULL,
    user_id TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(script_uri, user_id, key)
);

-- Create index on script_uri and user_id for faster lookups
CREATE INDEX idx_personal_storage_script_uri_user_id ON personal_storage(script_uri, user_id);

-- Create index on key for faster key-based queries
CREATE INDEX idx_personal_storage_key ON personal_storage(key);

-- Create composite index for script-specific user-specific key lookups
CREATE INDEX idx_personal_storage_script_uri_user_id_key ON personal_storage(script_uri, user_id, key);

-- Create index on updated_at for cleanup operations
CREATE INDEX idx_personal_storage_updated_at ON personal_storage(updated_at DESC);
