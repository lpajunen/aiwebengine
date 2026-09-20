-- Which sender, on which channel, may set a person's delegated work going.
--
-- A delegation says an app may act for somebody while they are away. It does
-- not say that anyone who can reach that app's public routes may choose when
-- and with what payload it acts as them — and an inbound webhook hands
-- exactly that to the internet. So a grant on its own is not enough to let a
-- script name a person: the person also has to have said which sender may
-- trigger it.
--
-- That turns the dangerous shape inside out. Without this table a script
-- would name a user id, and a script that trusted the wrong field in a
-- request body could name *anybody*. With it, a script can only name a sender
-- as the channel reports it, and the engine resolves that to a person. An
-- unbound sender resolves to nobody and nothing runs.
--
-- The primary key is what makes resolution unambiguous: one sender on one
-- channel is at most one person *for this script*. Two accounts claiming the
-- same Telegram id would be a question the engine cannot answer, so the
-- second is refused rather than guessed at — and refused rather than allowed
-- to replace the first, since silently moving a binding is a takeover.
--
-- What this cannot do is verify that the message really came from that
-- sender. Telegram and Slack sign their webhooks; email largely does not.
-- Checking the signature is the script's job, and no schema can do it for
-- them.
CREATE TABLE IF NOT EXISTS script_channel_identities (
    user_id TEXT NOT NULL,
    script_uri TEXT NOT NULL,
    -- A short slug for where the message came from: telegram, slack, email.
    -- Not a fixed vocabulary, because the engine gates on the binding rather
    -- than on which channel it names, and a list here would be one more thing
    -- to change before a solution could use a new one.
    channel TEXT NOT NULL,
    -- The sender as that channel names them. Compared exactly: a Telegram id
    -- is digits and a Slack one is case-sensitive, so normalising would make
    -- two different senders look like one.
    identity TEXT NOT NULL,
    granted_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (script_uri, channel, identity)
);

-- For the account page, and for dropping a person's bindings when their
-- delegation goes.
CREATE INDEX IF NOT EXISTS idx_script_channel_identities_user
    ON script_channel_identities (user_id, script_uri);
