-- Create sessions table
CREATE TABLE IF NOT EXISTS sessions (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    session_id TEXT NOT NULL UNIQUE,
    user_id TEXT NOT NULL,
    data JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL,
    last_accessed_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Create index on session_id for fast lookups
CREATE INDEX idx_sessions_session_id ON sessions(session_id);

-- Create index on user_id for user session queries
CREATE INDEX idx_sessions_user_id ON sessions(user_id);

-- Create index on expires_at for cleanup queries
CREATE INDEX idx_sessions_expires_at ON sessions(expires_at);

-- Create composite index for active session queries
-- Note: Cannot use NOW() in WHERE clause as it's not IMMUTABLE
-- Filter at query time instead
CREATE INDEX idx_sessions_user_active ON sessions(user_id, expires_at);
