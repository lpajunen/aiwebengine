# Security work still to do

Security functionality the engine does not have yet, kept here so that removing
a setting nothing read is never mistaken for deciding the engine does not need
the thing the setting named.

Each item says what is true today, what should be true, and how you would know
the work is done. Anything finished should leave this file rather than sit here
marked done.

## 1. MCP prompt handlers are the last request path without an owner check

`execute_mcp_prompt_handler` now runs as its caller. What it does not do is
check whether that caller may reach _this_ script — there is no ownership or
publication check the way `engine_api` applies one for script management.

Worth deciding whether a prompt registered by one solution should be callable
by anyone who can reach `/mcp` on that host, or only by someone the script's
owner would recognise.

## 2. Engine-entered contexts still run as an administrator

Four invocations construct `UserContext::admin(...)` because there is no caller
to attribute: startup script execution (`lib.rs`), route discovery, `init()`,
and the scheduler (`js_engine.rs`).

That is defensible — none of them is reachable from a request — but a scheduled
job arguably should run as the identity that registered it rather than as an
engine-wide administrator, so that a solution's background work cannot do more
than the solution itself. This needs a notion of "the script's owner" at
scheduling time, which the engine does not currently record.

## 3. There is no machine-to-machine credential, on purpose

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
