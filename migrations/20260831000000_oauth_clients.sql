-- Clients registered with this engine's OAuth2 authorization server.
--
-- Registration previously built a client, returned its credentials and dropped
-- it on the floor, so the authorization endpoint had nothing to check a
-- `client_id` or a `redirect_uri` against and accepted both. Persisting them is
-- what lets `/auth/oauth2/authorize` refuse a client it never issued and a
-- redirect URI that client never registered.
CREATE TABLE IF NOT EXISTS oauth_clients (
    client_id TEXT PRIMARY KEY,
    -- NULL for a public client (`token_endpoint_auth_method = "none"`), which
    -- is what every MCP client registering dynamically is.
    client_secret_hash TEXT,
    client_secret_expires_at TIMESTAMPTZ,
    client_name TEXT,
    -- Matched exactly, so it is stored as the caller wrote it.
    redirect_uris TEXT[] NOT NULL,
    grant_types TEXT[] NOT NULL,
    response_types TEXT[] NOT NULL,
    token_endpoint_auth_method TEXT NOT NULL,
    scope TEXT,
    -- The full RFC 7591 metadata, so the registration response can be
    -- reproduced without a column per optional field.
    metadata JSONB NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- What a user has agreed a given client may do on their behalf.
--
-- Registration is open, so a client existing proves nothing about who created
-- it — an attacker can register one as easily as an editor can. The consent
-- recorded here is the thing that actually stands between a cross-site
-- navigation and an authorization code: the first time a client asks, a person
-- has to say yes on a page that names it.
--
-- One row per (user, client). Re-consent is required when the client asks for
-- a scope or resource the stored row does not already cover, so widening what a
-- client may do cannot happen silently.
CREATE TABLE IF NOT EXISTS oauth_client_grants (
    user_id TEXT NOT NULL,
    client_id TEXT NOT NULL,
    scope TEXT,
    resource TEXT,
    granted_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (user_id, client_id)
);

CREATE INDEX IF NOT EXISTS idx_oauth_client_grants_user ON oauth_client_grants(user_id);
