-- Create table to track script-owned database tables
-- This enables scripts to dynamically create and manage their own tables
-- with automatic cleanup on script deletion via CASCADE

CREATE TABLE IF NOT EXISTS script_tables (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    script_uri TEXT NOT NULL,
    logical_table_name TEXT NOT NULL,
    physical_table_name TEXT NOT NULL,
    schema_json JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    
    -- Foreign key with cascade delete to clean up metadata when script is deleted
    CONSTRAINT fk_script_tables_script_uri 
        FOREIGN KEY (script_uri) 
        REFERENCES scripts(uri) 
        ON DELETE CASCADE,
    
    -- Each script can only have one table with a given logical name
    UNIQUE(script_uri, logical_table_name),
    
    -- Physical table names must be globally unique
    UNIQUE(physical_table_name)
);

-- Index for fast lookups by script URI (for cleanup and listing)
CREATE INDEX idx_script_tables_script_uri ON script_tables(script_uri);

-- Index for fast lookups by physical table name (for validation)
CREATE INDEX idx_script_tables_physical_name ON script_tables(physical_table_name);

COMMENT ON TABLE script_tables IS 'Tracks dynamically created tables owned by scripts for isolation and cleanup';
COMMENT ON COLUMN script_tables.logical_table_name IS 'Table name as seen by the script (e.g., "users")';
COMMENT ON COLUMN script_tables.physical_table_name IS 'Actual PostgreSQL table name with script prefix (e.g., "script_abc123_users")';
COMMENT ON COLUMN script_tables.schema_json IS 'JSON schema definition of columns for reference';
