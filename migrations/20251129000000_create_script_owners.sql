-- Create script_owners junction table
-- This table manages many-to-many relationships between scripts and their owners.
-- Bootstrap scripts (privileged=true) typically have no owners and can only be edited by admins.
-- User-created scripts automatically get their creator as the initial owner.
-- Additional owners can be added/removed by existing owners or administrators.

CREATE TABLE script_owners (
    script_id UUID NOT NULL REFERENCES scripts(id) ON DELETE CASCADE,
    user_id TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (script_id, user_id)
);

-- Index for finding all owners of a script
CREATE INDEX idx_script_owners_script_id ON script_owners(script_id);

-- Index for finding all scripts owned by a user
CREATE INDEX idx_script_owners_user_id ON script_owners(user_id);

-- Index for ownership verification queries
CREATE INDEX idx_script_owners_lookup ON script_owners(script_id, user_id);

COMMENT ON TABLE script_owners IS 'Junction table managing script ownership. Scripts can have multiple owners who can edit them. Administrators can edit any script regardless of ownership.';
COMMENT ON COLUMN script_owners.script_id IS 'Reference to the script UUID';
COMMENT ON COLUMN script_owners.user_id IS 'User ID from the users table (TEXT format, not UUID)';
COMMENT ON COLUMN script_owners.created_at IS 'When this ownership relationship was created';
