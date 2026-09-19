# What the engine needs to host agents

Notes from building [aiwebengine-agent](https://github.com/lpajunen/aiwebengine-agent) — a prompt page, an agent
loop, per-person LLM keys — against the engine as it stands. Everything below is
something that solution hit, not something imagined; each item says what is
true today, what it costs, and what changing it would mean.

There are two audiences, and they want different things. An **external agent**
(an MCP client driving `/engine/*`) is nearly served today. An **in-engine
agent** (a script that is itself the agent) is blocked in specific, nameable
ways, and the list below is those in order of how much each unblocks.

The agent's own list of what it would build on top is
[TODO-improvements.md](https://github.com/lpajunen/aiwebengine-agent/blob/main/TODO-improvements.md)
in that repository. Most of it is script work. What is left over is this.

## What got built

The first four items this file carried are done, and the shape each took is
worth keeping because the remaining items lean on them.

- **A budget for work that is not a request.** `javascript.job_timeout_ms`
  (`config.rs:305`), 60 s by default, with `script_limits.rs` making it
  per-script so one agent does not set the ceiling for twenty ordinary
  solutions.
- **Background work that acts as a person.** `delegation.rs` — a grant per
  `(user, script)`, a fixed scope vocabulary, a mandatory expiry, withdrawal on
  `/auth/account`, and the tier capped at `authenticated` however much the
  person holds.
- **Something durable to enqueue.** `tasks.rs` — a queue a handler can write to
  from any phase, with attempts, backoff and a visible state, surviving `init()`
  because a task is an event rather than a declaration.
- **The secret template anywhere in a header value**
  (`http_client.rs:652`), which put every bearer-token API back in reach of a
  per-person key.

Those four are what let the agent's run move out of the browser and into the
engine. What follows is what it hit next.

## The blockers, in order

### 1. A script cannot run with fewer capabilities than it holds

This is the largest one, and it is new: it did not surface until the agent had
tools worth restricting.

`UserContext` carries `capabilities: HashSet<Capability>`
(`capabilities.rs:8`), built by one of four fixed constructors and inherited by
everything the execution does. It only ever flows downward from the caller.
There is no way for a script to say _run this next part holding less than I do_.

Two things want exactly that, and neither is reachable without it.

**Code as the action.** The most interesting shape an agent can take here is
the model writing JavaScript rather than emitting a tool call, because the
composition comes free and the tool list stops growing. Every other project
adopting that pattern has to build a sandbox first. This engine _is_ one. What
is missing is the ability to evaluate model-authored code in a sub-context
holding a chosen subset — `eval_script` exists, but as an administrator's
engine endpoint, not as something a script can call on itself with **less**
than it has.

**A plan mode that is enforced rather than offered.** Every harness that ships
one — Claude Code, Cline, Cursor — implements it as a filtered tool list plus a
prompt asking the model nicely. None can actually enforce read-only. This
engine could, because capabilities are checked under the JavaScript rather than
inside the agent loop: a planning turn would simply run in a context holding
the reads and none of the writes, and no amount of cleverness in the transcript
reaches past it. Without this primitive, a plan mode written in a script is
exactly as weak as everybody else's.

The structural part looks small — a `UserContext` with an arbitrary subset is
already representable. The design work is the surface: what a script may ask
for, whether attenuation is permanent for the execution or scoped to a call,
and how the reduced context is proved to the things that check it.

### 2. A script cannot enqueue work for anyone but its caller

`personalTasks.enqueue` sets `run_as: Some(user_id)` from the calling person's
auth (`secure_globals.rs:5211`), and the worker resolves that person's grant
again when the task is claimed (`tasks.rs:735`, `delegation.rs:341`). Both
halves are right. The gap is that the only person a script can name is the one
making the request.

This is what blocks the whole of the channels idea — an agent reachable from
Telegram, email, Slack, the places people already are, which is the single
biggest thing missing from an in-engine agent. **An inbound webhook arrives
with nobody signed in.** There is no session, so there is no person, so there
is nothing to enqueue against, and every downstream piece — normalising the
message, running the turn, replying on the same channel — is script work sitting
behind this.

What makes it tractable is that `run_as` already exists on the row and the
grant is already re-checked at claim time. The missing piece is narrow: a way
to enqueue against a person the script names rather than the one it is serving,
with the engine verifying a live grant for that person at enqueue time the same
way it does at claim time.

The security question is the real work, and it is not the mechanism. Anyone who
can reach a webhook could try to start somebody else's agent, so the script has
to establish that this sender is that account — and the engine has to decide
whether a grant is enough on its own or whether delegation needs to name the
channel identity it may be triggered by. Deciding that badly makes a webhook
into a way to spend other people's tokens.

### 3. The scope vocabulary has nouns and no verbs

`Scope` is `PersonalStorage` and `Secrets` (`delegation.rs:67`) — two nouns,
naming _what_ a delegation reaches. Nothing names _what it may do with it_.

The file's own comment states the rule this has to respect: each value gates
something real, and adding a name without adding the check that enforces it
would be a promise the engine does not keep. So this is not a matter of
extending an enum. A read-only scope needs an enforcement point, which is item
1; that is why these two are one piece of work approached from opposite ends.

Two things want it. A **plan approved in advance** — which is what a delegation
grant already is for background work, except that it cannot currently say "may
read, may not write". And **input from a stranger**: once channels exist, a
message from an unknown sender should resolve to a narrower context than the
same words typed by the account owner on their own page, and a scope that can
express the difference is how that gets written down rather than remembered.

### 4. No streaming, in either direction

`fetch` buffers to `MAX_RESPONSE_SIZE` (`http_client.rs:29`, 10 MB) and hands
back a string. A script cannot read an SSE or chunked response, so it cannot
consume a model's token stream — an agent page updates per turn, and a turn is
as long as the whole model call.

The outbound half is already built: `stream_manager` and
`routeRegistry.sendStreamMessage` will push to a person's open page today. What
is missing is precisely a `fetch` that hands back a reader, and a bridge
between the two, which would turn a silent minute into visible progress.

The same limit applies to anything else that streams — an events API, a log
tail on another service.

### 5. The queue has no lane key

`NewTask` carries `script_uri`, `handler_name`, `payload`, `run_at`,
`max_attempts`, `enqueued_by`, `kind` and `run_as` (`tasks.rs:142`). Nothing
says _this task must not run beside that one_, and claiming is
`FOR UPDATE SKIP LOCKED` across whatever is due.

For an agent that is a correctness bug rather than a missing feature. Two
prompts from one person become two runs interleaving turn for turn, each
reading and overwriting the same `personalStorage` and the same notes. The
script can work around it with a claimed-status table of its own, and the
agent's list says to — but every script queueing per-person work has this same
bug, and a lane key belongs to the queue rather than to each of its callers.

Cloudflare's Agents SDK gets this free by making the session the unit of
execution. Here it would be a column and a predicate on the claim: at most one
running task per lane, others left pending.

### 6. Parallelism inside a turn

`fetch` is a synchronous host call. `Promise.all` over three of them sequences
them, and each holds an execution slot, a blocking thread and possibly a
database connection for its whole round trip. An agent that wants to run three
tool calls at once cannot. (This is TODO.md's "async support" item seen from the
agent side; noting it here because for an agent it is not ergonomics, it is
wall-clock against a hard ceiling.)

### 7. MCP is POST-only

`/mcp` is `axum::routing::post(mcp_handler)` (`lib.rs:2708`). No SSE transport,
so:

- no progress notifications during a long tool call,
- no server→client **sampling**, which is the interesting one: with it, a
  script's MCP tool could ask the _calling_ agent's model instead of the engine
  holding an API key at all. That is the cleanest answer to "who pays for the
  tokens" — the caller does, with their own client, and the engine never sees a
  credential.
- no elicitation, so a tool cannot ask the person a question mid-run.

### 8. No non-interactive credential

There is no client-credentials grant and no static API key, so an external
agent has to complete a browser OAuth flow once and then live on refresh
tokens. For an agent running on someone's laptop that is fine; for one running
in CI it is a genuine obstacle.

The engine's stance here — nothing unattended, the credential always belongs to
somebody present — is deliberate and stated in `git_sync`'s design. It just
collides with agents, and it is worth deciding on purpose which side wins.

## What external agents already have

Worth recording, because it is the part that works: an agent driving the engine
from outside gets the loop it needs.

- The native MCP tools and the same operations as REST, with
  `/engine/openapi.json` describing them.
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

The gap for this audience is item 7 (transport) and item 8 (credential), not
capability.

One thing this set cannot currently be turned into: an **in-engine** agent that
edits solutions. Those tools want ownership or an administrator, and
`delegation.rs` caps background work at `authenticated` on purpose, precisely so
that a delegated task is not the way to obtain a context that can author one.
Both cannot hold. Such an agent would have to run inside its owner's own
request, under their own editor rights, scoped to the scripts they own — which
is a narrower thing than the agent already built here, and the narrowness is the
point.
