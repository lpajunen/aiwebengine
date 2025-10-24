-- Create logs table
CREATE TABLE IF NOT EXISTS logs (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    script_uri TEXT NOT NULL,
    message TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Create index on script_uri for filtering logs by script
CREATE INDEX idx_logs_script_uri ON logs(script_uri);

-- Create index on created_at for chronological queries and pruning
CREATE INDEX idx_logs_created_at ON logs(created_at DESC);

-- Composite index for fetching recent logs for a specific script
CREATE INDEX idx_logs_script_uri_created_at ON logs(script_uri, created_at DESC);
