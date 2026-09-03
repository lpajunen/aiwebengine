-- Recovery codes for accounts the engine authenticates itself.
--
-- These accounts deliberately have no verified email address — that is the
-- point of them — so the reset link every other system sends is not available
-- here. What was left was `--set-password`, which needs the machine the engine
-- runs on: an answer for a personal install and for an operator, and no answer
-- at all for someone who forgot their password on a solution they merely use.
--
-- A recovery code is a second credential for the same account, issued ahead of
-- time and written down. It proves the same thing a password proves, and the
-- only thing it can do is set a new password.
CREATE TABLE IF NOT EXISTS local_recovery_codes (
    -- SHA-256 of the code, hex. The code itself is shown once, when the set is
    -- issued, and never again. No salt and no work factor on purpose: the
    -- engine generates these with about a hundred bits of entropy, so there is
    -- nothing to guess and nothing to pre-compute — the same reasoning as
    -- `oauth_refresh_tokens.token_hash`, and the reason redeeming one is a
    -- single indexed lookup rather than a verification against every row a
    -- user holds.
    code_hash TEXT PRIMARY KEY,
    user_id TEXT NOT NULL REFERENCES users(user_id) ON DELETE CASCADE,
    issued_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    -- Non-null once spent. Kept rather than deleted so that a code which was
    -- used and a code which never existed are the same answer to the endpoint
    -- and different answers in the log.
    used_at TIMESTAMPTZ
);

-- Issuing a set replaces the one before it, and the account page reports how
-- many are left, so both read by user.
CREATE INDEX IF NOT EXISTS idx_local_recovery_codes_user ON local_recovery_codes(user_id);
