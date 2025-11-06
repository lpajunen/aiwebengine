-- Add log_level column to logs table
ALTER TABLE logs ADD COLUMN log_level TEXT;

-- Update existing rows to have a default log level, e.g., 'INFO'
UPDATE logs SET log_level = 'INFO' WHERE log_level IS NULL;

-- Make log_level NOT NULL after setting defaults
ALTER TABLE logs ALTER COLUMN log_level SET NOT NULL;

-- Create index on log_level for filtering
CREATE INDEX idx_logs_log_level ON logs(log_level);