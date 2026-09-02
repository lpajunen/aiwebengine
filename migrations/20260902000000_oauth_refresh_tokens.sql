-- Refresh tokens issued by this engine's OAuth2 authorization server.
--
-- The token endpoint used to answer with the session token in both the
-- `access_token` and the `refresh_token` field, so the two were the same
-- credential: rotation was impossible, and a leaked "refresh token" was a
-- leaked access token carrying the same audience and the same roles.
--
-- A refresh token is a different thing from a session. It authenticates
-- nothing on its own — it is only ever presented at the token endpoint, by the
-- client it was issued to, to mint a fresh session. So it is stored here,
-- hashed, rather than being a session row.
CREATE TABLE IF NOT EXISTS oauth_refresh_tokens (
    -- SHA-256 of the token, hex. The token itself is shown once, at issue.
    token_hash TEXT PRIMARY KEY,
    -- The rotation chain this token belongs to. Redeeming a token issues its
    -- successor in the same family; presenting a token that was already spent
    -- means either the client replayed it or someone else has a copy, and
    -- neither is distinguishable from the other, so the whole family goes.
    family_id TEXT NOT NULL,
    user_id TEXT NOT NULL,
    -- The token is bound to the client it was issued to: another client
    -- presenting it is refused even if it holds the string.
    client_id TEXT NOT NULL,
    -- What the session minted from this token will carry, so a refresh cannot
    -- widen what the original authorization granted.
    audience TEXT,
    scope TEXT,
    issued_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    expires_at TIMESTAMPTZ NOT NULL,
    -- Non-null once redeemed. Kept rather than deleted, because a row that is
    -- gone and a row that was spent look the same to the endpoint, and only
    -- the second one is evidence of replay.
    consumed_at TIMESTAMPTZ
);

CREATE INDEX IF NOT EXISTS idx_oauth_refresh_tokens_family ON oauth_refresh_tokens(family_id);
CREATE INDEX IF NOT EXISTS idx_oauth_refresh_tokens_user ON oauth_refresh_tokens(user_id);
CREATE INDEX IF NOT EXISTS idx_oauth_refresh_tokens_expiry ON oauth_refresh_tokens(expires_at);
