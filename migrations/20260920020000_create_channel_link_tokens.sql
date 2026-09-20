-- The proof that whoever is linking a sender can actually receive that
-- sender's messages.
--
-- Without this, `/auth/delegate?channel=telegram&identity=12345` is a URL
-- anybody can construct for anybody. Linking would then be first come, first
-- served on a guessable string, and the harm is not squatting — it is
-- interception. Bind a victim's Telegram id to your own account before they
-- do, and every message they send that bot is processed as *your* turn, with
-- their text landing in your storage.
--
-- So a person cannot name a sender. A script mints a token in response to a
-- message it actually received, and replies into that chat with the link. The
-- only way to reach the consent page for a sender is to be reading that
-- sender's messages, which is the property the whole scheme needs and the one
-- a query parameter could never carry.
--
-- Only the hash is stored, for the reason `oauth_refresh_tokens` gives: this
-- is engine-generated entropy rather than a password, so there is nothing to
-- guess and nothing to pre-compute, and a copy of the table is not a set of
-- usable links.
CREATE TABLE IF NOT EXISTS script_channel_link_tokens (
    token_hash TEXT PRIMARY KEY,
    script_uri TEXT NOT NULL,
    channel TEXT NOT NULL,
    identity TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Minting replaces whatever was outstanding for the same sender, which both
-- bounds the table at one live row per sender and makes asking for a new link
-- invalidate the old one. This index is what makes that delete cheap.
CREATE INDEX IF NOT EXISTS idx_channel_link_tokens_sender
    ON script_channel_link_tokens (script_uri, channel, identity);

-- For the sweep that goes with each mint: an unredeemed token is litter
-- rather than a liability, but litter nobody collects is still a growing
-- table.
CREATE INDEX IF NOT EXISTS idx_channel_link_tokens_expiry
    ON script_channel_link_tokens (expires_at);
