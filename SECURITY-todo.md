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

## 3. `security.api_key` is a single shared secret

`AuthManager::validate_api_key` compares one configured value in constant time.
There is no rotation, no per-client key, and no way to revoke one caller without
changing it for everyone. Fine for a personal install; worth revisiting before
anything depends on it for machine-to-machine access at scale.
