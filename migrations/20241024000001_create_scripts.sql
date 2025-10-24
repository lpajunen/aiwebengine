-- Create scripts table
CREATE TABLE IF NOT EXISTS scripts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    uri TEXT NOT NULL UNIQUE,
    code TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    initialized BOOLEAN NOT NULL DEFAULT FALSE,
    init_error TEXT,
    last_init_time TIMESTAMPTZ
);

-- Create index on URI for faster lookups
CREATE INDEX idx_scripts_uri ON scripts(uri);

-- Create index on created_at for chronological queries
CREATE INDEX idx_scripts_created_at ON scripts(created_at DESC);
