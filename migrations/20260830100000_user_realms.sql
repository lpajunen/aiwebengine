-- Which host an identity is a principal on.
--
-- A session cookie is scoped to a host by the browser, but a session token
-- presented in an `Authorization: Bearer` header is scoped by nothing — and
-- both the engine API middleware and the MCP endpoint accept one. So an
-- account created by a solution's sign-up form was a principal on every host
-- the process serves, including a management host the cookie would never have
-- been sent to. Capabilities bound what such an account could *do*; nothing
-- bound where it existed.
--
-- The realm is that bound. `*` means every host, and is only ever set
-- deliberately by an administrator. Anything else names the one host the
-- account authenticates on.
--
-- Existing rows get the empty string: created before realms existed, and
-- therefore not yet scoped. An empty realm authorizes nothing — the next
-- successful sign-in records the host it happened on, so this costs everyone
-- one re-authentication rather than a recreated account. It is deliberately
-- not treated as `*`: a column added for scoping must not default to
-- unscoped.
ALTER TABLE users ADD COLUMN IF NOT EXISTS realm TEXT NOT NULL DEFAULT '';

CREATE INDEX IF NOT EXISTS idx_users_realm ON users(realm);
