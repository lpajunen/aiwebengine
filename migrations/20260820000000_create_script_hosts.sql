-- Create script_hosts junction table
--
-- Binds a script's HTTP registrations — routes, asset routes, streams, GraphQL
-- operations and MCP tools — to specific hostnames. A script with no rows here
-- publishes on the default host (server.base_url), which is what every script
-- did before this table existed, so no backfill is needed.
--
-- The reserved host '*' means "every configured host", for scripts such as a
-- shared about page that should answer on all of them. Only administrators can
-- change these bindings.

CREATE TABLE script_hosts (
    script_uri TEXT NOT NULL REFERENCES scripts(uri) ON DELETE CASCADE,
    host TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (script_uri, host)
);

-- Index for resolving a single script's hosts
CREATE INDEX idx_script_hosts_script_uri ON script_hosts(script_uri);

-- Index for listing the scripts published on a given host
CREATE INDEX idx_script_hosts_host ON script_hosts(host);

COMMENT ON TABLE script_hosts IS 'Binds scripts to the hostnames their registrations are published on. A script with no rows publishes on the default host (server.base_url). Administrator rights are required to change these bindings.';
COMMENT ON COLUMN script_hosts.script_uri IS 'Reference to the script URI';
COMMENT ON COLUMN script_hosts.host IS 'Hostname in Host-header form (hostname, or hostname:port for a non-default port). The reserved value ''*'' means every configured host.';
COMMENT ON COLUMN script_hosts.created_at IS 'When this binding was created';
