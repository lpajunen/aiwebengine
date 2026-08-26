-- Index the digest a revision's root source is stored under.
--
-- The foreign key to asset_blobs does not create one, and garbage collection
-- asks the question it answers: for every blob, is any revision's root still
-- this content? Without the index that is a sequential scan of every revision
-- per blob considered, which is the shape that makes a collector too expensive
-- to run often enough to matter.
--
-- The manifest side already has idx_script_revision_files_sha.
CREATE INDEX idx_script_revisions_root_sha ON script_revisions (root_sha256);
