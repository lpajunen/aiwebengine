-- Remove script initialization tracking columns
-- These columns are tracked in-memory only and not persisted to database
-- Init errors are already logged to log_messages table with FATAL severity

ALTER TABLE scripts DROP COLUMN IF EXISTS initialized;
ALTER TABLE scripts DROP COLUMN IF EXISTS init_error;
ALTER TABLE scripts DROP COLUMN IF EXISTS last_init_time;

-- Remove unused id column
-- Scripts are uniquely identified by uri column which serves as the primary key
ALTER TABLE scripts DROP COLUMN IF EXISTS id;
