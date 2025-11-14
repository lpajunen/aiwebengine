-- Rename script_storage table to shared_storage
ALTER TABLE script_storage RENAME TO shared_storage;

-- Rename indexes to match new table name
ALTER INDEX idx_script_storage_script_uri RENAME TO idx_shared_storage_script_uri;
ALTER INDEX idx_script_storage_key RENAME TO idx_shared_storage_key;
ALTER INDEX idx_script_storage_script_uri_key RENAME TO idx_shared_storage_script_uri_key;
ALTER INDEX idx_script_storage_updated_at RENAME TO idx_shared_storage_updated_at;