# What the engine needs to host agents

Notes from building [aiwebengine-agent](https://github.com/lpajunen/aiwebengine-agent) — a prompt page, an agent
loop, per-person LLM keys — against the engine as it stands. Everything below is
something that solution hit, not something imagined; each item says what is
true today, what it costs, and what changing it would mean.

There are two audiences, and they want different things. An **external agent**
(an MCP client driving `/engine/*`) is nearly served today. An **in-engine
agent** (a script that is itself the agent) is blocked in specific, nameable
ways, and items 1–4 are that list in order of how much each unblocks.

## The blockers, in order

### 1. A budget for work that is not a request

`javascript.execution_timeout_ms` (10 s by default) bounds every execution, and
a scheduled handler gets the same one: `execute_scheduled_handler` builds its
runtime from `current_execution_limits()` like everything else. `fetch`'s own
timeout is then clamped to what is left (`within_host_budget`), so a script
cannot wait out one model call, let alone a turn of several.

The number is also engine-wide. An operator hosting one agent alongside twenty
ordinary solutions has to raise the ceiling for all of them.

**What would help, smallest first:**

- `javascript.job_timeout_ms` — a separate budget for scheduled handlers, the
  way `init_timeout_ms` is already separate for `init()`. The precedent and the
  reasoning are identical: a job is not answering a request, so a per-request
  number is answering a question it is not asking.
- A per-script or per-route ceiling, so one slow script does not set the policy
  for every other one. This is the larger change — the limits are a process-wide
  `OnceLock` — but it is the one that makes hosting agents alongside ordinary
  solutions reasonable.

### 2. Background work cannot act as a user

This is the one that decides the architecture. `fetch` resolves
`{{secret:...}}` against `self.user_context.user_id`, captured when globals are
installed. A scheduled handler runs as `UserContext::admin("scheduler")`, so it
looks for the person's key under a user literally named `scheduler`, misses, and
falls back to the script-wide secret. Same for `personalStorage`, which reads
`context.request.auth.userId` and throws `SecurityError` without one.

So anything that needs a person's credential or a person's storage has to run
inside that person's request. An agent can only work while its owner's tab is
open.

This is not an oversight to patch — it is a consent question the engine has not
been asked yet. A background job holding somebody's API key is a real grant, and
it needs a real model: what the person authorised, for which script, for how
long, and how they revoke it. Worth designing deliberately rather than
arriving at by adding a parameter.

Sketch: a delegation record — `(user, script, scope, expires)` — written by the
person through an explicit consent page, and a `UserContext` a job can assume
that carries exactly that and nothing else. The engine already has the shape of
this in `oauth_client_grants`.

### 3. Nothing durable to enqueue

`schedulerService.registerOnce` is phase-gated: called outside `init()` it
returns a string saying nothing was registered (`secure_globals.rs:4629`). So a
handler cannot schedule its own continuation, and the "start work, return, pick
it up later" shape has no expression at all.

`dispatcher.sendMessage` is not the escape hatch it looks like — listeners run
inline in the sender's execution, on the sender's budget and under the sender's
context.

What is missing is a queue: something a request can write to and a worker can
pick up, with attempts, backoff and a visible state. Either that, or make
`registerOnce` work from a handler — but a queue is the more honest primitive,
because the scheduler's registrations are in-memory and re-registered at every
`init()`, which is right for cron and wrong for "this one piece of work".

### 4. No streaming, in either direction

`fetch` buffers up to 10 MB and hands back a string. A script cannot read an
SSE or chunked response, so it cannot consume a model's token stream — which
means an agent page updates per turn, and a turn is as long as the whole model
call. A `fetch` that could hand back a reader, with each chunk pushed into an
existing `stream_manager` stream, would turn a 10-second silence into visible
progress.

The same limit applies outbound to anything else streaming — an events API, a
log tail on another service.

## Smaller things that cost real time

### 5. Parallelism inside a turn

`fetch` is a synchronous host call. `Promise.all` over three of them sequences
them, and each holds an execution slot, a blocking thread and possibly a
database connection for its whole round trip. An agent that wants to run three
tool calls at once cannot. (This is TODO.md's "async support" item seen from
the agent side; noting it here because for an agent it is not ergonomics, it is
wall-clock against a hard ceiling.)

### 6. MCP is POST-only

`/mcp` is `axum::routing::post(mcp_handler)` — `initialize`, `tools/list`,
`tools/call`, `prompts/*`, `completion/complete`. No SSE transport, so:

- no progress notifications during a long tool call,
- no server→client **sampling**, which is the interesting one: with it, a
  script's MCP tool could ask the _calling_ agent's model instead of the engine
  holding an API key at all. That is the cleanest answer to "who pays for the
  tokens" — the caller does, with their own client, and the engine never sees a
  credential.
- no elicitation, so a tool cannot ask the person a question mid-run.

### 7. The secret template is narrower than documented

`http_client.rs:463` matches `starts_with("{{secret:") && ends_with("}}")` — the
template must be the **entire** header value, and only a header value.

`assets/aiwebengine.d.ts` documents something else: `@param url - URL to fetch
(supports {{SECRET_NAME}} syntax for secret injection)`, with an example
sending `"Authorization": "Bearer {{API_TOKEN}}"`. Neither form works. A script
written from the published example sends the literal text to the API and gets a
401 with nothing to explain it.

Two fixes, and both are worth doing:

- Substitute `{{secret:name}}` anywhere in a header value, not just as the whole
  of it. Without this, every bearer-token API — which is most of them — is out
  of reach of a per-user key.
- Correct the declarations either way, since they are what a solution developer
  reads.

### 8. No non-interactive credential

`grant_type` accepts `authorization_code` and `refresh_token` only
(`auth/routes.rs:4139-4148`). There is no client-credentials grant and no static
API key, so an external agent has to complete a browser OAuth flow once and then
live on refresh tokens. For an agent running on someone's laptop that is fine;
for one running in CI it is a genuine obstacle.

The engine's stance here — nothing unattended, the credential always belongs to
somebody present — is deliberate and stated in `git_sync`'s design. It just
collides with agents, and it is worth deciding on purpose which side wins.

## What external agents already have

Worth recording, because it is the part that works: an agent driving the engine
from outside gets the loop it needs.

- 47 native MCP tools (`engine_api.rs:8909`) and the same operations as REST,
  with `/engine/openapi.json` describing them.
- `search_files` → `read_file` → `edit_file` / `write_assets` → `check_script` →
  `run_tests` → `read_init_status` → `read_logs` → `list_revisions` /
  `diff_revisions` / `revert_script` → `deploy_script`.
- `check_script` and `run_tests` take **candidate** files through
  `source_view.rs`'s overlay, so a change spanning several modules is verified
  before any of it is stored. This is the most agent-shaped thing in the
  codebase.
- Blast-radius control: the agent gets its own account, `add_script_owner` for
  what it may touch, editor rather than administrator, and `deploy_script` to
  pin what serves while it experiments on head.
- `/engine/script_updates` and `/engine/script_logs/stream` for "someone else
  changed this" and "my deploy is throwing".

The gap for this audience is item 6 (transport) and item 8 (credential), not
capability.
