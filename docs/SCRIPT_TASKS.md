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
a handler could not schedule its own continuation. The engine had no expression
at all for "start something, answer, finish it later" — which is most of what
an agent does.

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

## What must not run beside what

Two tasks enqueued a moment apart run at the same time. Claiming takes a batch
and the worker spawns each run rather than awaiting it, so this is the default
and for most work it is the point.

For work belonging to one person it is a bug. Two prompts to an agent become
two runs interleaving turn for turn, each reading and overwriting the same
`personalStorage` and the same notes. A **lane** says those two must not run
together: at most one task per `(script, lane)` runs at a time, and the rest
stay pending until it finishes.

```javascript
// One conversation at a time, several conversations at once.
scriptTasks.enqueue({
  handler: "runTurn",
  payload: { text },
  lane: `chat:${chatId}`,
});
```

**`personalTasks` defaults its lane to the person.** That is the correctness
fix rather than a convenience: essentially every solution queueing per-person
work has the interleaving bug, and holding it in the queue is better than
every caller writing the same claimed-status table. Name your own lane for a
finer one, or pass `lane: null` to opt out and let that person's tasks run in
parallel.

`scriptTasks` has no default, because a script task belongs to the solution
rather than to a person and there is nothing to infer one from — and
defaulting every script task into one lane would serialise the whole queue.

Three things worth knowing:

- **A lane is per script**, like a job key. Two solutions both using the lane
  `"inbox"` are not talking about the same thing.
- **A lane is not a queue of its own.** Order within it is by `run_at`, and a
  task waiting on a busy lane is picked up on a later tick rather than held
  in memory.
- **A lane unblocks when a worker dies.** "Busy" means a run whose lease is
  live, so a task left `running` by an instance that vanished stops holding
  its lane once the lease lapses. Without that, one crash would shut a
  person's agent for good.

The lane is on the row, so `list_tasks` and `/engine/tasks` answer "why has
this not run" with "it is behind another task in its lane" rather than
leaving you to guess.

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

Lanes hold across instances too, and that takes four separate pieces of the
claim rather than one. A lane already holding a live run is excluded by a
`NOT EXISTS`. Two tasks of one lane falling in a single batch are cut to one
by a window function, since neither is running yet and the `NOT EXISTS` cannot
see them. Two workers claiming at the same moment are kept apart by
`FOR UPDATE`, which locks every candidate a statement selected — except at the
batch boundary, where a lane's later tasks fall outside one worker's `LIMIT`
and an advisory lock on the lane closes the gap.

And an advisory lock alone does not close it, which is what a failing CI run
said. The lock answers "is anybody else choosing from this lane right now",
while the `NOT EXISTS` answers "was anything running in it as of this
statement's snapshot" — and under `READ COMMITTED` that snapshot is taken when
the statement begins, which may be before the other worker committed the claim
the lock was there to protect. So claiming is two statements in one
transaction: the first chooses the rows and holds their lanes, the second
re-checks each lane and claims what survives, with its snapshot taken once the
locks are held. Exclusion from the lock, freshness from the second statement;
neither alone is enough.
