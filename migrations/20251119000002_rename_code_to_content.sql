-- Rename code column to content in scripts table
-- This makes the naming consistent with assets table which uses 'content'

ALTER TABLE scripts RENAME COLUMN code TO content;
