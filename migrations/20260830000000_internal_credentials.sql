-- Internal identities: accounts the engine authenticates itself, with no
-- third-party provider behind them.
--
-- Two kinds share this shape. A *guest* has no credential at all — the engine
-- mints an identity and hands out a session, so a solution's user gets stable
-- storage and a display name without surrendering an email address. A *local*
-- account has a username and password held here. A guest becomes a local
-- account by attaching a credential to the same user_id, which is why the
-- credential lives in its own table rather than in a column on `users`.

-- A guest has no email address. The unique constraint went in
-- 20251028100000; this drops the last assumption that every identity is
-- reachable by mail.
ALTER TABLE users ALTER COLUMN email DROP NOT NULL;

CREATE TABLE IF NOT EXISTS local_credentials (
    user_id TEXT PRIMARY KEY REFERENCES users(user_id) ON DELETE CASCADE,
    -- Folded to lower case on write; the column is what login looks up, so
    -- uniqueness and lookup agree on one spelling of a name.
    username TEXT NOT NULL,
    -- Argon2id PHC string: algorithm, parameters and salt travel with it, so
    -- the cost can be raised later without a migration.
    password_hash TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

CREATE UNIQUE INDEX IF NOT EXISTS idx_local_credentials_username
    ON local_credentials (username);
