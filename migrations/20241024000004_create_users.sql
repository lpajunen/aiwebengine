-- Create users table
CREATE TABLE IF NOT EXISTS users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id TEXT NOT NULL UNIQUE,
    email TEXT NOT NULL UNIQUE,
    name TEXT,
    provider TEXT NOT NULL,
    is_admin BOOLEAN NOT NULL DEFAULT FALSE,
    is_editor BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    last_login_at TIMESTAMPTZ
);

-- Create index on email for fast lookups
CREATE INDEX idx_users_email ON users(email);

-- Create index on user_id for authentication
CREATE INDEX idx_users_user_id ON users(user_id);

-- Create index on provider for filtering
CREATE INDEX idx_users_provider ON users(provider);

-- Create composite index for role queries
CREATE INDEX idx_users_roles ON users(is_admin, is_editor);
