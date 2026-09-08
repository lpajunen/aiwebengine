-- A person's own credential for a git host.
--
-- Deliberately not `user_secrets`, which is the obvious home and the wrong one.
-- That table is keyed (script_uri, user_id, key) and it *is* the JavaScript
-- `secretStorage`: a token stored there is readable by the script it is keyed
-- to. It is also script-scoped, and a personal access token is not a script's
-- credential — it is the person's, and it is engine-wide.
--
-- Nothing here is reachable from the sandbox. The token is read in exactly one
-- place, by the Rust that talks to the git host, and no endpoint returns it.
-- Listing answers with metadata only, the same posture `list_secrets` takes.
CREATE TABLE user_git_credentials (
    -- Whose credential this is. A pull uses the acting user's own token, which
    -- is what keeps attribution honest: the account that reaches GitHub is the
    -- account that asked.
    user_id      TEXT NOT NULL,
    -- The host it authenticates against, so one person can hold a credential
    -- per host without them colliding.
    remote_host  TEXT NOT NULL,
    -- The token, AES-GCM encrypted under `security.secret_encryption_key` and
    -- stored as the serialized `EncryptedData` the rest of the engine uses.
    -- Never selected by anything that answers a request.
    token        TEXT NOT NULL,
    -- The account the token belongs to, captured when it was checked against
    -- the host. Shown in listings so a person can tell two tokens apart
    -- without any part of either being revealed.
    account      TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- When a pull last used it. The one signal that a stored credential is
    -- still doing something, and worth having before anyone has to guess
    -- whether a token is safe to remove.
    last_used_at TIMESTAMPTZ,
    PRIMARY KEY (user_id, remote_host)
);

COMMENT ON TABLE user_git_credentials IS 'Per-user, per-host git tokens, encrypted at rest and never exposed to scripts or returned by any endpoint.';
