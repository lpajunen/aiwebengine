CREATE TABLE IF NOT EXISTS user_secrets (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    script_uri TEXT NOT NULL,
    user_id TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE(script_uri, user_id, key)
);
CREATE INDEX idx_user_secrets_script_uri_user_id ON user_secrets(script_uri, user_id);
CREATE INDEX idx_user_secrets_key ON user_secrets(key);
CREATE INDEX idx_user_secrets_script_uri_user_id_key ON user_secrets(script_uri, user_id, key);
CREATE INDEX idx_user_secrets_updated_at ON user_secrets(updated_at DESC);
