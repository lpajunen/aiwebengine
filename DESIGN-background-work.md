# Background Work: Tasks, Delegation, and What Replaces the Scheduler

A design for the gap [TODO-agent.md](TODO-agent.md) names in items 1–3: an
in-engine agent cannot do work that outlives the request that started it. This
note is the answer to that, written before any of it is built, so the parts that
are cheap and the part that is a consent decision can be told apart.

The shape it starts from is a pair of interfaces mirroring the storage split —
a script-context worker and a personal-context one — replacing
`schedulerService`, callable outside `init()`, with a budget measured in an
hour rather than ten seconds. That instinct is right in its most important
part, and wrong in two specific ones. What follows says which is which.

## The distinction the API turns on is declared versus enqueued

Not one-off versus recurring. That is the axis `schedulerService` already
splits on, and it is the wrong one.

A **declaration** is part of the program. It is keyed by a name, it is
idempotent, and it should be wiped and re-declared when the script changes —
which is exactly what `script_init.rs:235` does before every `init()`. A cron
schedule is code; it belongs to the version of the code that declared it, and a
schedule surviving the deletion of the handler it names is a bug, not a
feature.

An **enqueued task** exists because something happened. It carries a payload,
it must survive `init()` and deploys, and it has attempts, backoff and a
terminal state. Nothing about it is idempotent or re-derivable, because the
event that produced it has already gone.

`scheduler_jobs` is built for the first of those and cannot be relaxed into the
second. The table has `UNIQUE(script_uri, job_key)` — one row per named
declaration — so enqueueing two instances of _process this document_ is not
slow or awkward, it is impossible: the second upserts over the first. It has no
payload column, and `registerOnce` takes a handler name with no arguments, so
there is nothing for the work to be _about_. It has no attempt counter and no
last-error, so a task that fails has nowhere to record why.

So `schedulerService` stays as it is: the declarative cron surface, phase-gated
at `secure_globals.rs:4629`, correctly wiped on init. The queue is a new table
and a new API beside it. Replacing one with the other would mean either losing
"the declared schedule follows the code" or gaining durable orphan jobs from a
version of the script that no longer exists.

## An hour is the wrong number, and thirty seconds is a live bug

`DB_LOCK_TTL_SECONDS = 30` (`scheduler/mod.rs:21`). The row claim sets
`lock_expires_at = now + 30s` and nothing renews it. A job running longer than
that has its row re-claimed by another instance, which then reaches
`pg_try_advisory_lock`, finds the lock held, and returns — leaving `locked_by`
naming an instance that is not running the job. The finishing instance's
`DELETE ... AND locked_by = $2` (`scheduler/mod.rs:730`) then matches no rows.
A successful one-off that took over thirty seconds is therefore never deleted,
and runs again.

The advisory lock is what stops that from being concurrent double-execution.
It does not stop it from being repeated execution, and the row bookkeeping is
wrong either way. Separately, a failing one-off requeues at a flat two seconds
(`DB_ONE_OFF_RETRY_DELAY_SECONDS`) with no cap and no backoff, so a task that
cannot succeed retries forever. Both are defects today, at a ten-second
ceiling where almost nothing reaches them. A budget measured in an hour makes
them the ordinary case.

The deeper objection is that an hour is the wrong _shape_, not just the wrong
value. A long execution holds a runtime, a blocking thread from
`execution_slots`, and possibly a database connection, for its whole duration —
and it does not survive a rolling deploy, which kills it and restarts it from
the top with no record of what it had already done. What an agent wants is a
checkpoint: do one step, enqueue the next. The queue gives that for free and it
survives restarts, where a single long execution buys nothing that a restart
does not take back.

What an hour would buy that minutes do not is waiting out one slow model call.
That is a real constraint, and the answer to it is TODO-agent items 4 and 5 —
a streaming response body and concurrent `fetch` — not a larger ceiling.
`within_host_budget` (`database.rs:115`) clamps a host call to the budget
remaining, so what a worker budget has to exceed is _one_ `fetch`, not a whole
turn of several.

So: minutes, with the lock renewed on a heartbeat for as long as the run holds
it, and durability carried by the chain of enqueues rather than by the length
of any one execution.

### Per-script limits belong in the same change

`ExecutionLimits` is already a struct passed to `create_sandboxed_runtime`; the
process-wide `OnceLock` is only where the default comes from. Threading a
per-script override through is plumbing, not a restructure, and it is what
stops one agent script from setting the ceiling for twenty ordinary solutions
on the same engine. Worth doing while the limit plumbing is already open.

## The personal half is a consent record wearing a worker's clothes

"Runs under that user's context, so authenticated users only" understates what
is being decided. Whether the person was authenticated at enqueue time is not
the question. The question is what they authorised, for which script, for how
long, and how they see and revoke it. A task enqueued during a request and run
ten minutes later under that person's full capability set is a stored
credential with no expiry and no visibility — which is a thing the engine has
refused to build everywhere else it has come up.

Every piece of the right answer is already written down in this codebase:

- **`oauth_client_grants` is the shape.** A record of what a person approved,
  per `(user, script, scope, expires)`, written through an explicit consent
  page. The grant is the thing a task runs under; the task carries a reference
  to it, not a copy of a `UserContext`.
- **Re-read roles and realm at run time and intersect with the grant.** Do not
  snapshot the capability set at enqueue. This is exactly the argument
  `auth::refresh_tokens` already makes for minting a fresh session on every
  refresh: a revocation that happened in between has to take effect rather than
  being copied forward. A task queued this morning and run this evening must
  not be the one place a withdrawn role still holds.
- **`/auth/sessions` is where it surfaces.** A background job acting as you is
  the same question as a session acting as you, and that page already exists,
  already lists what is acting as the account, and already offers a revoke. A
  pending personal task is another row on it.
- **`delete_sessions_for_user` cancels pending personal tasks**
  (`security/session.rs:72`), the same way it already drops refresh tokens, and
  for the same reason: everywhere else has to include the ways back in that a
  session list cannot show.

That is a real chunk of work, and it is the argument for shipping the
script-context queue first and alone. The script queue needs none of it —
nobody's credential is involved — and it is what unblocks the "start work,
return, pick it up later" shape that items 1–3 are really about.

## Async dispatch is delivery, not a third API

`dispatcher.sendMessage` runs its listeners inline, on the sender's budget and
under the sender's context. The reasoning for that context choice
(`secure_globals.rs:5079`) is sound and should not be disturbed: a listener is
part of serving the invocation that dispatched to it, so it holds what the
sender held, no more and no less.

What is missing is the asynchronous form — `dispatcher.post(type, data)`,
which enqueues onto the task queue instead of running listeners now. Small
change, real value, and it is what makes the dispatcher usable for fan-out
rather than only for synchronous delegation within one execution.

It raises the same "under whose context" question, and the answer should be the
conservative one: an asynchronous dispatch runs in **script context**. Personal
context requires the explicit grant above. Keeping the escalation story to one
sentence is worth more than the convenience of the alternative.

## Naming

`scriptWorker` / `personalWorker` parallels `scriptStorage` / `personalStorage`
neatly, and reusing a distinction solution developers already have is the right
instinct. But _worker_ in JavaScript means Web Worker — a concurrent thread
reached by `postMessage` — and this is not that. A developer reading
`scriptWorker.start()` will expect parallelism and find a queue.

`scriptTasks.enqueue(...)` / `personalTasks.enqueue(...)` says what it does and
cannot be misread as threads.

## A queue nobody can see is a queue nobody can debug

The new table needs a payload, an attempt count, a last error, a next-attempt
time and a terminal state, and all of that needs a surface. The engine's
pattern for this is settled — `/engine/revisions`, `/engine/deploy` — so:
`/engine/tasks` to list and inspect, plus `list_tasks` and `cancel_task` MCP
tools. Without it, a failing agent task is invisible except as whatever it
managed to log before it died.

One thing to settle deliberately rather than inherit: **which version of the
script a task runs against.** A task enqueued while the script was at revision
40 and run after it has been pinned to 41 is running against a different
program than the one that queued it. Consistency with everything else says
resolve through `deployments::serving_view` (`deployments.rs:116`) at run time,
so a task behaves like every other execution of that script.

_Settled as built:_ that is what happens, and it needed no code — a task loads
its script through `repository::fetch_script`, which already answers with what
the script serves rather than head, for the reason written at
`repository.rs:5170`. The consequence stands and is worth saying out loud: a
long-lived task can be picked up by code that has never seen its payload
shape. Keeping payloads small, and naming stored data rather than copying it,
is what makes that survivable.

## Order

All six items are built.

1. ~~**Fix the secret template**~~ _(done)_ (TODO-agent item 7,
   `http_client.rs:463`) and
   the `.d.ts` that documents a third syntax that also does not work. One-line
   change. Without it a personal task cannot put a per-user key in
   `Authorization: Bearer …`, which is most model APIs — so it is a
   prerequisite for any of the rest being useful, and the cheapest item here.
2. ~~**`javascript.job_timeout_ms`**~~ _(done)_, with the lock renewed for the
   length of a run, an attempt cap, and real backoff. Fixes an existing bug and
   gives immediate relief. The precedent is `init_timeout_ms`, already separate
   for the same reason: a job is not answering a request.

   One thing the note got wrong: deriving the claim's length from the job's
   budget is not enough, because the wait for an execution slot happens after
   the claim and is bounded by nothing. The lease is renewed on a timer
   instead, which needs no guess about how long the work takes and returns a
   dead worker's jobs in one lease rather than one budget.

3. ~~**`scriptTasks`**~~ _(done)_ — new table, payload, attempts, last error,
   state; enqueue from any phase; survives `init()`; `/engine/tasks` and the
   MCP tools. See `docs/SCRIPT_TASKS.md`.

   The open question below answered itself: `repository::fetch_script` already
   resolves through the deployment pin, so a task runs against what the script
   _serves_ rather than against head, with no code of its own. A task also
   keeps no row when it succeeds — one row per success grows the table for the
   outcome nobody debugs, and the script's log already has it under the run's
   invocation id — while a failure keeps its row and its last error, which is
   the one somebody has to read.

4. ~~**`dispatcher.post`**~~ _(done)_ onto that queue. The listener is the same
   registration and the same code either way, which needed a `kind` on the
   queued row: a posted message is handed to its handler as `messageType` and
   `messageData`, the shape an inline `sendMessage` uses. Without that a
   listener would behave differently depending on how the message reached it,
   and reusing the dispatcher's registrations is the only reason to post
   rather than enqueue directly.
5. ~~**The delegation record**~~ _(done)_ — consent page, grant table,
   run-time intersection, `/auth/sessions` surfacing, cancellation on revoke —
   and then `personalTasks` on top of it. See `docs/DELEGATION.md`.

   Two things the note left open. The "intersection" turned out better stated
   as a _cap_: a delegated task runs at the authenticated tier whatever the
   person holds, rather than computing an intersection with their roles — one
   sentence instead of a rule that has to be re-derived every time roles
   change, and narrowing in the only direction that is safe. And a refusal is
   terminal rather than retried: a withdrawn grant does not clear by waiting,
   and retrying is the engine repeatedly asking to act as somebody who has
   said no.

6. ~~**Per-script limits.**~~ _(done)_ See `docs/SCRIPT_LIMITS.md`. The note
   called this plumbing and it was, but it named the wrong beneficiary:
   raising a ceiling for an agent is the obvious use and _lowering_ one is the
   valuable one, since containing a script that has started holding execution
   slots previously meant an engine-wide config change and a restart. The
   authorisation is an administrator rather than the script's owner — this is
   a claim on slots, threads and memory shared with every other tenant, and an
   owner who could raise their own ceiling would be back to one script setting
   the policy for all of them.

Items 1–4 are mechanical and unblock the in-engine agent. Item 5 is the design
decision, and it is worth arriving at deliberately rather than by adding a
parameter.

What remains from `TODO-agent.md` is items 4, 5, 6 and 8 of that note —
streaming `fetch`, parallelism inside a turn, an MCP transport that carries
sampling, and a non-interactive credential. None of them is background work;
they are what an agent needs _within_ one turn, and they are the right next
thing to look at now that work can outlive a request at all.
