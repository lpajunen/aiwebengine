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
  (`http_client.rs:617`), which put every bearer-token API back in reach of a
  per-person key — and since extended to the URL _path_, for the APIs that put
  the key in the route rather than in a header, without letting it reach a log.

Those four are what let the agent's run move out of the browser and into the
engine.

The three below are the ones that followed. The first two this file called
one piece of work approached from opposite ends, and so they were; the third
turned out to depend on neither, and on a decision rather than a mechanism.

- **A script can run with fewer capabilities than it holds.** `sandbox.run`
  (`sandbox.rs`, `assets/sandbox_prelude.js`) — a sub-execution of the same
  script in a context holding a chosen subset, with JSON the only thing that
  crosses. Both open questions this file named are answered: attenuation is
  **scoped to the call** rather than permanent, and the reduced context is
  proved to the things that check it **by construction** — a separate QuickJS
  context cannot be handed a function by its parent, which is the leak a
  push/pop mask over the running context could not close.

  The mechanism was nearly free: `evaluate_snippet` already ran caller-authored
  source against a script's program with a caller-chosen `UserContext`,
  `dispatcher.sendMessage` already built a nested runtime inside a running host
  call, nested budgets were already clamped to the parent's remaining time, and
  a nested rollback was already a `SAVEPOINT`. What the work actually consisted
  of was **item 3** — the vocabulary. Attenuating the old `Capability` enum
  bought nothing an agent cares about: `fetch` was gated by nothing at all,
  `secretStorage` and `personalStorage` only by a delegation scope, and reads
  and writes of a script's tables shared one name, so read-only was
  inexpressible. The enum now names what a script _does_ as well as what a
  person may do to a solution — `use_network`, `read_secrets`/`write_secrets`,
  `read_storage`/`write_storage`, `enqueue_tasks`, `send_messages`, and
  `read_script_data`/`write_script_data` — each with the check that enforces
  it, and each held by every tier that could already do the thing, so adding
  them took nothing away. See `docs/CAPABILITY_ATTENUATION.md`.

- **A delegation can say "may read, may not write".** `Scope::Write`
  (`delegation.rs`) — the verb the vocabulary did not have, and could not have
  had before there was somewhere to enforce it. `resolve` no longer returns a
  fixed `authenticated` tier: it caps there and then attenuates to what the
  grant says, so a grant without the verb holds no write capability at all.
  The nouns keep their own mechanism, because the two answer different
  questions — a noun says whose things are in scope and answers as though
  they were not there, a verb is a capability and refuses at the gate every
  other caller meets. A migration adds `write` to grants that predate it,
  which restores rather than widens: the page those people saw offered "read
  and change". See `docs/DELEGATION.md`.

- **A script can enqueue work for a person it is not serving.**
  `personalTasks.enqueueFrom` and `script_channel_identities`
  (`delegation.rs`) — which is what an inbound webhook needed, since one
  arrives with nobody signed in.

  The mechanism was the small half, as this file predicted. The decision was
  the work, and it came out as: **a grant is not enough on its own.** A grant
  says an app may act for somebody; it does not say that whoever can reach
  the app's public routes may choose when and with what payload. So a person
  also links the sender that may trigger them, and a script names
  `{channel, identity}` rather than an account — there is no user id in the
  API at all. An unlinked sender resolves to nobody, so a handler that trusts
  the wrong field in a request body can be made to claim the wrong _sender_,
  and not to name a different account or enumerate one.

  Linking itself takes an invitation rather than a URL naming the sender,
  which was the second decision and nearly went the wrong way. A guessable
  `?channel=&identity=` link would have made binding first come first served
  — and the harm there is interception rather than squatting: bind a
  victim's chat id before they do and every message they send the bot
  becomes your turn, with their text in your storage. So a script mints a
  single-use token in reply to a message it received and replies into that
  chat, which makes "can read that sender's messages" the price of reaching
  the consent page.

  What the engine still cannot do is verify the message came from that
  sender. Telegram and Slack sign; email does not. That is the script's job
  and the documentation says so rather than pretending otherwise.

- **The queue has a lane key.** `script_tasks.lane` — at most one running
  task per `(script, lane)`, the rest left pending. `personalTasks` defaults
  it to the person, which is the correctness fix this file asked for rather
  than a feature: every script queueing per-person work had the same
  interleaving bug and had to work around it with a claimed-status table of
  its own.

  Holding the invariant took three pieces of the claim rather than the one
  predicate this file predicted, because there are three separate ways two
  tasks in a lane get claimed at once — a lane already busy, two in one
  batch, and two workers at the batch boundary. Each has a test that fails
  when its piece is removed. See `docs/SCRIPT_TASKS.md`.

- **`fetch` can run several at once, and can be read as it arrives.**
  `fetchAll` and `fetchStream` (`http_client.rs`). These were listed apart
  and are one change: both came from `fetch` being a single blocking host
  call that did the whole request and returned a string.

  `fetchAll` runs a batch on worker threads — the same `fetch` per request,
  so validation and secret substitution cannot diverge — with positional
  answers and per-request failures. The execution budget is read on the
  calling thread and armed again on each worker, or a script with two
  seconds left could have started a thirty-second request.

  `fetchStream` hands back status and headers as they arrive and the body in
  pieces, which is what makes a model's token stream consumable; bridged to
  `sendStreamMessage` it is the silent minute becoming visible progress.
  Bytes that do not yet form whole characters are kept between reads, since
  a chunk boundary regularly splits one and decoding each read alone would
  corrupt exactly the text a model generates.

  It is **not** an event loop, and the distinction matters for what is left
  below. A host call still blocks the script; what changed is how much one
  blocked call can be waiting on. See `docs/FETCH_CONCURRENCY.md`.

What follows is what the agent hit next.

## The blockers, in order

### 1. ~~A script cannot run with fewer capabilities than it holds~~ — done

See **What got built** above. The one thing worth carrying forward is what it
costs: a sub-execution is a fresh runtime and one re-evaluation of the script's
program, which is right for an agent turn and wrong for a loop. Narrow once
around the turn, not once around each tool call. If that ever becomes the
binding constraint, the answer is a cheaper way to build the same isolation
rather than a mask over the running context — the leak that ruled a mask out
does not get better with optimisation.

### 2. ~~A script cannot enqueue work for anyone but its caller~~ — done

See **What got built** above. What is left of this item is the part that was
always script work: normalising a message, running the turn, replying on the
same channel. The engine's half — a way to name a person it is not serving,
and a defensible answer to who may do the naming — is there.

One thing to hold on to. The engine gates the _link_ and the _grant_, and
cannot gate the **authenticity** of the message. A solution on a channel that
signs its webhooks should check the signature; on one that does not, the link
is the only thing between an inbound message and somebody's budget, and a
solution should treat a channel like email accordingly.

### 3. ~~The scope vocabulary has nouns and no verbs~~ — done

See **What got built** above. Of the two things this item said wanted it, one
is delivered and one is only half:

A **plan approved in advance** is what `Scope::Write` is — a grant that
authorises an app to go away and work out what to do without authorising it to
do the thing.

**Input from a stranger** still needs item 2. The scope that expresses the
difference now exists, so "this sender resolves to a narrower context than the
account owner typing the same words" is writable down; what is missing is a
way to have a stranger's message reach a person's delegation at all.

One cost is worth carrying forward: a read-only grant holds no `enqueue_tasks`,
and work longer than one budget is a chain of tasks, so a read-only delegation
cannot be a long one. Neither queueing nor dispatching can actually escalate
under a delegation — a queued personal task re-resolves the same grant — so if
that constraint ever bites, the answer is a third scope rather than moving
`enqueue_tasks` into the base.

### 4. ~~No streaming, in either direction~~ — done

See **What got built** above, and item 6 below: these were one piece of work
rather than two.

### 5. ~~The queue has no lane key~~ — done

See **What got built** above. Two limits are worth carrying forward.

A lane serialises **claiming**, not the work a handler does with what it
reaches. Two tasks in one lane cannot run at once, but a task and a _request_
still can — somebody typing on the agent's page while their queued turn runs
is not something the queue can order. Solutions where that matters still need
their own arrangement.

And a read-only delegation holds no `enqueue_tasks`, so it cannot chain
tasks, so a long read-only run is not expressible. That is item 3's cost
rather than this one's, but lanes are where it shows up: the natural way to
do long per-person work is a chain in one lane.

### 6. ~~Parallelism inside a turn~~ — partly

What this item asked for was three tool calls at once, and that is what
`fetchAll` does.

What it did **not** buy is general async. A host call still blocks the
script, so a fetch cannot overlap with the script's own computation, a
`setTimeout` still does not exist, and `await` is still sequencing sugar over
work that has already finished. What changed is how much work one blocked
call can be waiting on.

The rest remains TODO.md's "async support" item, and it is a different and
much larger change — an event loop under QuickJS with every host call
rewritten to yield rather than block. Worth being plain that this did not do
it: an agent fanning out over network calls is served, and one wanting to
interleave computation with them is not.

### 7. MCP is POST-only — the correctness half is done

`/mcp` is `axum::routing::post(mcp_handler)` (`lib.rs:2715`, `:2764`). Still no
event-stream response, so still no progress notifications during a long tool
call, no **elicitation**, and no server→client **sampling** — which remains the
interesting one, because with it a script's MCP tool could ask the _calling_
agent's model instead of the engine holding an API key at all. That is the
cleanest answer to "who pays for the tokens": the caller does, with their own
client, and the engine never sees a credential.

Three things this item had wrong or did not say.

**It is smaller than "no SSE transport" implies.** Streamable HTTP requires a
server to answer `POST` and leaves the `GET` stream _optional_ — a `405` to a
`GET /mcp`, which is what `routing::post` already returns, is conformant rather
than a gap. What sampling and elicitation actually need is permission for one
POST _response_ to be `text/event-stream`, so server→client requests can travel
on it before the final result. That is a handler-shaped change rather than a
transport-shaped one, and it does not need a standing channel or session
management. The item was filed at the wrong size, and the right size moves it
up rather than down.

**The engine was lying to clients about `listChanged`, and that is now fixed.**
`initialize` advertised `"listChanged": true` for both tools and prompts while
`grep` found no emitter anywhere in `src/` — the notification travels
server-to-client and there is no stream to carry it. A conforming client
therefore cached the tool list and waited to be told otherwise, which is exactly
backwards here: scripts register tools at runtime, so the list is one of the few
that genuinely does change. Both now advertise `false`, which makes a client
re-list instead of trusting a promise the engine cannot keep. When a response
can be a stream, this flips back — and by then it will be true.

**The protocol version was pinned, and that is now fixed.** `initialize` read
the client's `protocolVersion` into a discarded binding and answered
`2024-11-05` unconditionally — the pre-Streamable-HTTP revision — while
`mcp_client.rs` has the engine speaking `2025-11-25` as a _client_, so the two
halves of one codebase disagreed about what year it was.
`mcp::negotiate_protocol_version` now answers the client's own version when it
is one of `SUPPORTED_PROTOCOL_VERSIONS`, the newest we speak when it is not, and
the oldest to a client that names none — because omitting the field predates the
field.

What is left of this item is the stream, and with it sampling and elicitation.

### 8. No non-interactive credential — the stance held, the rotation fixed

**The decision was made on purpose, and the stance won.** Nothing unattended;
the credential always belongs to somebody present. There is no
client-credentials grant and no static API key, and an external agent still
completes a browser OAuth flow once and then lives on refresh tokens.

What changed is that living on refresh tokens no longer punishes exactly the
clients the stance leaves holding them. This item's own argument for the grant
was the weak one — refreshing re-issues with a fresh `max_session_age`
(`auth/routes.rs:5394`), so an agent that runs monthly never expires, and
expiry was never the obstacle. The real one was rotation: `redeem` was
single-use with **family revocation on replay**, so a crash between the server
spending a token and the client storing its successor, or two CI jobs sharing
one stored token and starting together, killed the whole chain. Recovery is a
human at a browser — which an unattended agent by definition does not have. The
cost of the replay rule fell entirely on the clients that could not pay it.

So a spent token is now forgiven for `REPLAY_GRACE_SECS` (30) seconds, and only
while the chain has not moved on: if any _later_ token in the family has itself
been spent, the presenter is behind a rotation somebody else is advancing, which
is the theft signal rather than a retry, and the family still goes. The window
is anchored to the first redemption — the write is `COALESCE(consumed_at, now)`
rather than an assignment — so replaying every twenty seconds cannot hold a
token alive. Each of the three pieces has a test that fails when it is removed.
The detection this table exists for survives; what it stopped doing is firing on
its own clients.

Two loose ends made the engine look like it had half-started the grant, and both
are closed:

- `security.api_key` was parsed (`config.rs`), threaded into `AuthManager`
  (`lib.rs`), and `validate_api_key` had **no callers** — the same class as
  `cors_allowed_origins` and `enable_security_headers` before they were wired,
  a setting an operator could set to no effect, and here one that reads as a
  machine credential the engine does not have. Deleted rather than wired: the
  stance above is the reason it has no callers.
- `client_registration.rs` accepted `client_credentials` in `grant_types` while
  the token endpoint answered `unsupported_grant_type` to it, so a client could
  register for a grant it could never exercise and find out one request later,
  from a refusal naming the wrong end. Registration now refuses it, which says
  so while the client is still choosing what to be.

If this is ever revisited, the question to answer first is not transport but
whose roles and realm a userless token carries — that, rather than the grant
mechanics, is what the engine has no answer for.

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
