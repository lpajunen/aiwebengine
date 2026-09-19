# Work That Outlives the Request

```javascript
// Accept the work, answer now.
function startExport(context) {
  const task = scriptTasks.enqueue({
    handler: "runExport",
    payload: { accountId: context.request.query.account },
  });
  return { status: 202, body: JSON.stringify({ task: task.taskId }) };
}

// Runs afterwards, on its own budget.
function runExport(context) {
  const { accountId } = context.meta.task.payload;
  // ...
}
```

A script could not do this. `schedulerService.registerOnce` is phase-gated, so
a handler could not schedule its own continuation; `dispatcher.sendMessage`
only looks like a way out, because its listeners run inline, on the sender's
budget and under the sender's context. The engine had no expression at all for
"start something, answer, finish it later" — which is most of what an agent
does.

`scriptTasks` is that expression: a durable queue a request writes to and a
background worker picks up.

## Why this is not the scheduler

The two look similar and are not, and the difference decides everything else.

`schedulerService` registers a **declaration**. A script says in `init()` what
it runs on a schedule; the row is keyed `UNIQUE(script_uri, job_key)` so saying
it again replaces what it said; and every re-initialisation wipes the script's
jobs before running `init()` again, because a schedule belongs to the version
of the code that declared it. Delete the handler and the schedule goes with it,
which is right.

A task is the other thing. It exists because something happened, it carries
what happened as a payload, and two of them are two pieces of work rather than
one restatement of the same intent. So:

- **It survives deployment.** Writing a new version of a script does not
  discard work already accepted. A scheduled job is wiped and re-declared; a
  task is not.
- **It can be enqueued from anywhere.** No phase gate, because the whole shape
  depends on a handler being able to enqueue.
- **It carries a payload**, which a scheduled job has nowhere to put —
  `registerOnce` takes a handler name and nothing else.

That is why this is a separate table rather than a relaxation of
`scheduler_jobs`. Relaxing the uniqueness would let you enqueue two of the same
work; keeping it means you cannot.

## What a handler gets

`context.meta.task` carries the payload, the attempt number and the ceiling.
`attempt` counts from one, so a value above one means this is a retry.

The budget is `javascript.job_timeout_ms` — the scheduled-handler budget, not
the per-request one. A task is not answering a request.

A task runs in **script context**. It holds what the script holds and nothing
belonging to whoever enqueued it, so `personalStorage` and a per-user secret
are not reachable from one. Acting as a person in the background is a grant
that person has to make — what they authorised, for which script, for how long,
and how they revoke it — and there is nowhere yet for them to make it.

## Failure, and what is kept

A task that throws is retried after a widening delay, and given up on after
`maxAttempts` (5 by default, at most 25). A given-up task keeps its row, its
attempt count and the error that ended it, and writes a `FATAL` line to the
script's log.

A task that **succeeds keeps no row**. Keeping one per success grows the table
for the one outcome nobody is debugging; what it did is in the script's log
under its own invocation id. So `scriptTasks.get` answering `null` means
"finished, or never existed", and the log is where you tell those apart.

## Work longer than one budget

Do not reach for a bigger timeout. Split the work and enqueue the next step:

```javascript
function processPage(context) {
  const { cursor } = context.meta.task.payload;
  const next = doOnePage(cursor);
  if (next) {
    scriptTasks.enqueue({ handler: "processPage", payload: { cursor: next } });
  }
}
```

A chain of tasks survives a restart, where one long execution does not — it is
killed mid-run and started again from the top, having recorded nothing. The
chain also frees its execution slot between steps, where a long run holds one,
a blocking thread and possibly a database connection for its whole duration.

Keep the payload small and put the data itself in storage with the task naming
it. A retry then reads what is current rather than a copy taken when the task
was enqueued.

## Posting a message instead of sending it

`dispatcher.sendMessage` runs every listener inline — in your execution, on
your budget, under your context — and does not return until they all have.
`dispatcher.post` fans out to the same listeners and queues one task each:

```javascript
function placeOrder(context) {
  const { queued } = dispatcher.post("order.placed", { orderId });
  return { status: 201, body: JSON.stringify({ orderId, queued }) };
}
```

The listener is the same registration and the same code either way — it still
reads `context.messageType` and `context.messageData`. That compatibility is
the whole reason to post rather than calling `scriptTasks.enqueue` directly; a
listener that had to be written twice would defeat it. A queued one also has
`context.meta.task` if it wants to know it is a retry.

Two differences. Listeners are resolved when you post, so the fan-out goes to
whoever is listening at that moment rather than whenever the queue reaches it.
And a queued listener runs in script context — it holds what its own script
holds, not what you hold — because by the time it runs there is no caller left
to borrow authority from.

## Seeing the queue

```bash
# What is waiting, what is running, what failed and why
curl "https://your-engine/engine/tasks?script=myapp"

# Stop one that has not started
curl -X DELETE "https://your-engine/engine/tasks?script=myapp&task=<id>"

# Clear out the failed and cancelled ones once you have read them
curl -X DELETE "https://your-engine/engine/tasks?script=myapp&finished=true"
```

The same through MCP: `list_tasks` and `cancel_task`. Reading takes what
reading the script takes; cancelling and discarding take what writing it takes.

Cancelling works only while a task is still pending. One already claimed by a
worker is running, and marking the row cancelled would not stop it — it would
only change the meaning of a row that worker is about to write to.

## Several instances

The queue is shared. Each worker claims a batch with `FOR UPDATE SKIP LOCKED`,
so instances take disjoint work rather than queueing behind each other, and
holds a lease it renews for as long as the run lasts ([`src/lease.rs`], shared
with the scheduler). A worker that dies stops renewing, and its tasks become
claimable again once the lease lapses — which is how work survives the instance
that was running it.
