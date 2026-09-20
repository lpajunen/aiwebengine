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
