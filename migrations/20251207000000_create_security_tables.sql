-- Create OAuth2 authorization codes table
CREATE TABLE IF NOT EXISTS oauth_authorization_codes (
    code TEXT PRIMARY KEY,
    user_id TEXT NOT NULL,
    client_id TEXT NOT NULL,
    redirect_uri TEXT NOT NULL,
    code_challenge TEXT,
    code_challenge_method TEXT,
    scope TEXT,
    resource TEXT,
    expires_at TIMESTAMPTZ NOT NULL,
    used BOOLEAN NOT NULL DEFAULT FALSE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_oauth_codes_expires_at ON oauth_authorization_codes(expires_at);

-- Create rate limits table
CREATE TABLE IF NOT EXISTS rate_limits (
    key TEXT PRIMARY KEY,
    tokens DOUBLE PRECISION NOT NULL,
    last_refill TIMESTAMPTZ NOT NULL,
    total_requests BIGINT NOT NULL DEFAULT 0,
    rejected_requests BIGINT NOT NULL DEFAULT 0
);

-- Create failed authentication attempts table
CREATE TABLE IF NOT EXISTS failed_auth_attempts (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    identifier TEXT NOT NULL,
    attempt_time TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    type TEXT NOT NULL -- 'authentication' or 'authorization'
);

CREATE INDEX idx_failed_auth_identifier ON failed_auth_attempts(identifier);
CREATE INDEX idx_failed_auth_time ON failed_auth_attempts(attempt_time);

-- Create suspicious activity table
CREATE TABLE IF NOT EXISTS suspicious_activity (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    identifier TEXT NOT NULL,
    activity_type TEXT NOT NULL,
    severity_score DOUBLE PRECISION NOT NULL,
    details JSONB,
    timestamp TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE INDEX idx_suspicious_activity_identifier ON suspicious_activity(identifier);
CREATE INDEX idx_suspicious_activity_time ON suspicious_activity(timestamp);
