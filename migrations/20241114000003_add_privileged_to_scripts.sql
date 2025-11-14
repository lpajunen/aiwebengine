-- Add privileged flag to scripts table
ALTER TABLE IF EXISTS scripts
ADD COLUMN IF NOT EXISTS privileged BOOLEAN NOT NULL DEFAULT FALSE;

-- Mark built-in bootstrap scripts as privileged by default
UPDATE scripts
SET privileged = TRUE
WHERE uri IN (
    'https://example.com/core',
    'https://example.com/cli',
    'https://example.com/editor',
    'https://example.com/admin',
    'https://example.com/auth'
);
