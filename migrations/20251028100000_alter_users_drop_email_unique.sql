-- Drop unique constraint on email to allow same email from different providers
-- This prepares for future user identity merging functionality

ALTER TABLE users DROP CONSTRAINT users_email_key;