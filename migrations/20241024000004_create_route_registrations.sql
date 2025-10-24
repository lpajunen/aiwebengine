-- Create route_registrations table to store script route handlers
CREATE TABLE IF NOT EXISTS route_registrations (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    script_uri TEXT NOT NULL,
    route_path TEXT NOT NULL,
    http_method TEXT NOT NULL,
    handler_name TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    
    -- Ensure unique combinations of script + path + method
    UNIQUE(script_uri, route_path, http_method)
);

-- Create index on script_uri for lookups by script
CREATE INDEX idx_route_registrations_script_uri ON route_registrations(script_uri);

-- Create composite index for route matching
CREATE INDEX idx_route_registrations_route_method ON route_registrations(route_path, http_method);
