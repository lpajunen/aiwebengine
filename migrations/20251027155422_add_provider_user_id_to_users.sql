-- Add provider_user_id field to users table
ALTER TABLE users ADD COLUMN IF NOT EXISTS provider_user_id TEXT;

-- Create index on provider + provider_user_id for authentication lookups
CREATE INDEX IF NOT EXISTS idx_users_provider_user_id ON users(provider, provider_user_id);

-- Update existing records to have provider_user_id (if any exist)
-- This is a no-op for new installations