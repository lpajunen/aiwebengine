CREATE TABLE IF NOT EXISTS script_secrets (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    script_uri TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(script_uri, key)
);
CREATE INDEX idx_script_secrets_script_uri ON script_secrets(script_uri);
CREATE INDEX idx_script_secrets_key ON script_secrets(key);
CREATE INDEX idx_script_secrets_script_uri_key ON script_secrets(script_uri, key);
CREATE INDEX idx_script_secrets_updated_at ON script_secrets(updated_at DESC);
