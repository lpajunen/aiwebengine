# Security work still to do

Security functionality the engine does not have yet, kept here so that removing
a setting nothing read is never mistaken for deciding the engine does not need
the thing the setting named.

Each item says what is true today, what should be true, and how you would know
the work is done. Anything finished should leave this file rather than sit here
marked done.

## 1. There is no machine-to-machine credential, on purpose

`security.api_key` used to be here as an item about rotation — one configured
value, no per-client key, no way to revoke one caller without changing it for
everyone. That reading gave it too much credit: `validate_api_key` had no
callers at all, so the setting was one an operator could set to no effect, in
the same class as `cors_allowed_origins` and `enable_security_headers` before
they were wired. It is deleted, along with `client_credentials` in the
registration endpoint's accepted `grant_types` — which the token endpoint
answered `unsupported_grant_type` to anyway, so a client could register for a
grant it could never exercise.

What remains true is the gap, and it is a decision rather than an omission:
nothing unattended, the credential always belongs to somebody present. An
external agent completes a browser OAuth flow once and lives on refresh tokens,
which `auth/refresh_tokens.rs` now makes survivable for a client nobody is
watching.

If this is revisited, the question to answer first is not the grant mechanics
but whose roles and realm a userless token carries. A session carries both and
everything downstream reads them; a client-credentials token has no account
behind it, and inventing one is the part the engine has no answer for.
