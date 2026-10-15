# Personal Schedules

Both changes are built: personal schedules, and script schedules on the same
path (`scriptTasks.schedule`; `schedulerService` and `scheduler_jobs` are gone).
Where the build departed from this plan: one `schedules` table with `user_id`
NULL for a script's own; a script's schedules are swept after `init()` rather
than wiped before it, so one declared again keeps its next slot; a script's
runs get one attempt and never pause. The question this answers: how does a script run work **as
a person, on a schedule of that person's choosing**, while they are away — "every
morning at seven, summarise the news I follow", "twice a day, look for jobs
that match my profile"?

## What exists, and why it is not enough

Three things come close, and each misses in a different place.

- **`schedulerService.registerRecurring`** declares a schedule for the
  _script_. It runs as the engine (`Principal::Engine`), so it holds no
  person's storage or key, and its rows are wiped and re-declared on every
  `init()` because a schedule there belongs to the version of the code. A
  person's "every morning" must survive a deployment, and must not be one row
  shared by everybody.
- **`personalTasks.enqueue({ runAt })`** runs once, as the person, under their
  delegation. A handler can re-enqueue itself to make a chain, but nothing
  stops two chains for the same intent, nothing shows the person that one is
  running, a chain dies silently after its retries or when the grant lapses,
  and there is no local time of day.
- **A recurring job that fans out** to everyone who granted the script gives
  everybody the same cadence, and still shows nobody what runs as them.

The missing thing is a **declaration that belongs to a person**: keyed so that
saying it again replaces it, kept across deployments, bounded by the grant
that lets it act, and visible where that grant is.

## The shape

A personal schedule is a row in a new table, `personal_schedules`. It says
_when_. When a row is due, the engine inserts an ordinary delegated task into
`script_tasks`, and from there everything is what a `personalTasks.enqueue`
task already gets: the person's lane, retries, the job budget, and the grant
re-read when it runs. The schedule never runs JavaScript itself.

```text
personal_schedules ──(due: one transaction)──► script_tasks (run_as = person)
      ▲                                               │
      └──── outcome: consecutive failures, last run ◄─┘
```

Two tables rather than a recurring flag on `script_tasks` for the reason
`SCRIPT_TASKS.md` gives against extending `scheduler_jobs`: a schedule is a
restatable declaration (unique per key), a task is a piece of work (two are
two). Mixing them would let one person's "every morning" become two.

## The script API

The spec is one type, shared with script schedules (see
[One spec, one path](#one-spec-one-path-for-script-schedules-too)):

```ts
/** When, and what. The same for a script's schedule and a person's. */
interface ScheduleSpec {
  key: string;
  handler: string;
  payload?: Record<string, unknown>;
  /** Exactly one of `intervalMinutes`, `intervalMilliseconds` and `cron`. */
  intervalMinutes?: number;
  intervalMilliseconds?: number;
  cron?: string; // five fields, e.g. "0 7 * * *"
  /** IANA name; required with `cron`, refused with an interval. */
  timeZone?: string;
  /** The first run, when it should not wait a whole interval. */
  firstRunAfterMs?: number;
  lane?: string | null;
}

interface PersonalTasks {
  // ...existing members

  /**
   * Declare, or replace, this person's schedule under `key`. `lane` defaults
   * to the person, as in `enqueue`.
   */
  schedule(spec: ScheduleSpec): PersonalSchedule;

  /** As `schedule`, for a linked sender (a Telegram or Slack webhook). */
  scheduleFrom(
    spec: ScheduleSpec & { channel: string; identity: string },
  ): PersonalSchedule;

  /** This person's schedules for this script. */
  schedules(): PersonalSchedule[];

  /** Remove one. False if there was nothing under that key. */
  unschedule(key: string): boolean;
}

interface PersonalSchedule {
  key: string;
  handler: string;
  cron: string | null;
  intervalMinutes: number | null;
  timeZone: string | null;
  nextRunAt: string; // UTC
  lastRunAt: string | null;
  state: "active" | "paused";
  pausedReason?: "expired" | "failing";
}
```

The handler receives what a task handler receives, plus
`context.meta.schedule = { key, scheduledFor }`, so a run can tell which slot
it is for (and a digest can say "news since yesterday's 07:00").

**Who may call it.** The same rule as `enqueue` and `enqueueFrom`, so there is
one rule to learn:

- `schedule` and `unschedule` need someone signed in, `Capability::EnqueueTasks`,
  and a live grant from that person to this script. In the person's own
  request they hold `EnqueueTasks`; in a delegated run they hold it only if the
  grant includes `write`, which is the existing "a read-only delegation loses
  the queue" rule extended to schedules.
- `scheduleFrom` resolves a linked sender exactly as `enqueueFrom` does; the
  script never names an account.
- `schedules()` reads, so it needs a signed-in person and nothing else. There
  is no call that lists other people's schedules from a script.

Errors throw, as every prelude does: an invalid cron, an unknown time zone, an
interval below the floor, a key over its length, or the limits below.

## The table

```sql
CREATE TABLE personal_schedules (
    schedule_id UUID PRIMARY KEY,
    script_uri TEXT NOT NULL
        REFERENCES scripts(uri) ON UPDATE CASCADE ON DELETE CASCADE,
    user_id TEXT NOT NULL
        REFERENCES users(user_id) ON DELETE CASCADE,
    schedule_key TEXT NOT NULL,
    handler_name TEXT NOT NULL,
    payload JSONB NOT NULL DEFAULT '{}'::jsonb,
    -- Exactly one of the two.
    interval_ms BIGINT,
    cron TEXT,
    time_zone TEXT,
    lane TEXT,
    next_run_at TIMESTAMPTZ NOT NULL,
    last_run_at TIMESTAMPTZ,
    -- The task the last firing created, to skip a slot while it is unfinished.
    last_task_id UUID,
    consecutive_failures INTEGER NOT NULL DEFAULT 0,
    state TEXT NOT NULL DEFAULT 'active' CHECK (state IN ('active', 'paused')),
    paused_reason TEXT CHECK (paused_reason IN ('expired', 'failing')),
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    UNIQUE (script_uri, user_id, schedule_key),
    CHECK ((interval_ms IS NULL) <> (cron IS NULL))
);

CREATE INDEX idx_personal_schedules_due
    ON personal_schedules (next_run_at) WHERE state = 'active';

-- Which schedule a task came from, so its outcome can be written back.
ALTER TABLE script_tasks ADD COLUMN schedule_id UUID
    REFERENCES personal_schedules(schedule_id) ON DELETE SET NULL;
```

Foreign keys do most of the cleanup: deleting the account or the script
removes the schedules, and renaming a script carries them, as
`20261003090000_script_identity_follows_renames.sql` does for every other
table that names a script.

## Firing

A new module, `personal_schedules.rs`, is called from the task worker's poll
(`tasks.rs`, every 500 ms) before it claims tasks. One transaction:

1. `SELECT ... WHERE state = 'active' AND next_run_at <= now ORDER BY
next_run_at LIMIT n FOR UPDATE SKIP LOCKED`. Two workers in a cluster take
   different rows; no lock columns and no `NOTIFY` are needed, because firing
   is the whole transaction rather than a claimed run.
2. For each row, read the grant (`delegation::get`):
   - none → delete the row (revocation should already have; this catches a
     race);
   - lapsed → `state = 'paused', paused_reason = 'expired'`, no task;
   - live → continue.
3. If `last_task_id` still names a pending or running task, **skip this slot**:
   no task, log it, advance. Work that takes longer than its interval must not
   pile up behind itself.
4. Otherwise insert the task (`run_as = user_id`, the schedule's lane,
   `schedule_id`, `max_attempts = 3`) and set `last_task_id`, `last_run_at`.
5. Set `next_run_at` to the next slot **after now**, not after the slot that
   was due. After downtime a schedule runs once, not once per missed slot.

The task re-resolves the grant when it runs, as every delegated task does, so
the check in step 2 is about not queueing pointless work, not about authority.

**Outcome.** Where `tasks.rs` finishes a task that has a `schedule_id`:

- success → `consecutive_failures = 0`;
- given up (out of attempts) → `consecutive_failures + 1`, and at 3 →
  `state = 'paused', paused_reason = 'failing'`. A handler renamed away by a
  deployment, or a feed that is gone, then stops costing runs instead of
  failing every morning forever.

A paused schedule resumes when the script calls `schedule` again with the
same key, or when the person resumes it on the account page.

## Time

Cron is evaluated in the schedule's time zone, so "07:00 Europe/Helsinki"
stays at seven across daylight saving. Proposed crate: `croner` with
`chrono-tz` (check with `make deny`). The two DST edges, written into the
tests and the `.d.ts`:

- a local time that does not exist (the spring gap) runs at the first instant
  after it;
- a local time that happens twice (the autumn overlap) runs once, at the first.

`intervalMinutes` is plain elapsed time from the previous slot, in UTC.

## Limits

New keys in `[javascript]` beside `job_timeout_ms`:

| key                                    | default | why                                           |
| -------------------------------------- | ------- | --------------------------------------------- |
| `personal_schedule_min_interval_ms`    | 900000  | 15 minutes; one person's cron cannot hammer   |
| `personal_schedules_per_person_script` | 10      | a script restating keys in a loop hits a wall |
| `personal_schedules_per_person`        | 50      | the storage cap on the pilot readiness list   |

A cron whose two nearest slots are closer than the floor is refused when it is
declared (`* * * * *` fails; `0 7 * * *` passes). Each run also counts against
the script's limits like any task.

## Lifetime

What happens to a schedule when what it depends on changes:

| Event                                  | Schedule                                                  |
| -------------------------------------- | --------------------------------------------------------- |
| Script redeployed                      | Kept. It belongs to the person, not the code version.     |
| Script renamed                         | Follows (`ON UPDATE CASCADE`).                            |
| Script deleted                         | Deleted (FK).                                             |
| Grant withdrawn (`delegation::revoke`) | Deleted, with the pending tasks and linked senders.       |
| `revoke_all` (roles, realm, password)  | Deleted, all of the person's.                             |
| Grant lapsed                           | Paused, `expired`.                                        |
| Grant given again                      | Paused-`expired` ones resume; the consent page lists them |
| Account deleted                        | Deleted (FK), and erased with everything else.            |
| Handler gone or keeps failing          | Paused, `failing`, after 3 given-up runs.                 |

Deleting on withdrawal but only pausing on expiry is deliberate. Withdrawing is
the person saying stop; expiry is a deadline passing, and making them set up
their mornings again because they renewed a day late would teach people to
grant the longest expiry offered.

`revoke` and `revoke_all` in `delegation.rs` gain the delete, next to
`cancel_delegated` and the channel unlinking. `delegation.rs` is
security-critical, so the change goes through `SECURITY.md` review.

## What the person sees

The account page already lists grants beside sessions. Under each grant, its
schedules: key, "daily at 07:00 (Europe/Helsinki)" or "every 6 hours", next
run, last run, and paused with the reason. Two buttons: **pause/resume** and
**delete**. There is no edit: changing what runs is the app's to offer.

The consent page, when the script has paused schedules for this person, says
"These will start again: …". Granting is then agreeing to that, which is the
same reasoning `grant()` gives for replacing rather than merging.

Create and delete write audit lines, with `acting` set when a delegated run or
a linked sender did it, as other delegated actions do.

## Out of the script's sight

- No script can list, change or count another person's schedules.
- The engine operation table gets no "every schedule" listing in this
  change. If administrators need one later, it is an `AdministerEngine`
  operation like `list_users`, not a script global.
- The script owner sees runs in the script's log, under the task's invocation
  id, as for any delegated task. They do not see the schedule rows.

## Grant expiry needs someone to say so

A grant lasts at most 90 days. A schedule that pauses on day 91 with nobody
told is the failure this design exists to remove, moved three months later.
Until the engine can send email or notifications (pilot readiness, "Sending
email or notifications"), the script is the one that can: its own run reads
`personalTasks.authorization().expiresAt` and says so in whatever channel it
already uses (a Telegram reply, a line on its page). The template for a
scheduled app does this. When the engine can push, it warns seven days
before, and that warning moves here.

## One spec, one path for script schedules too

`schedulerService` and personal schedules share their vocabulary and their
machinery, and stay two calls.

### Why not one call

The two differ in who declares them, and everything else follows from that:

|                  | Script schedule            | Personal schedule                         |
| ---------------- | -------------------------- | ----------------------------------------- |
| Declared by      | the code, in `init()`      | a person, at runtime (or a linked sender) |
| Lives as long as | the version of the code    | the person's grant                        |
| On redeploy      | wiped and declared again   | kept                                      |
| Runs as          | the script                 | the person, narrowed to the grant         |
| Interval floor   | none beyond the job budget | `personal_schedule_min_interval_ms`       |
| Who can see it   | anyone reading the source  | the person, on the account page           |

One `schedule()` with a switch for whose it is would branch every row of that
table on the switch, including cleanup — the kind of quiet mistake the
authority invariants exist to rule out. Two calls let the call itself say
whose schedule it is.

Script schedules stay declared in `init()`. Declared at runtime they would
become state rather than code: what runs could no longer be read from the
source, and a schedule whose handler was deleted would outlive it.

### What is shared

- **The spec.** `ScheduleSpec` above is the argument to both. Script
  schedules gain `cron`, `timeZone`, `payload` and `firstRunAfterMs`, which
  `registerRecurring` lacks: a nightly job wants "03:00 local" as much as a
  person wants "07:00".
- **The firing path.** A due script schedule inserts a script-context task
  into `script_tasks`, exactly as a personal one inserts a delegated task.
  The execution is already the same — `execute_scheduled_handler` and
  `execute_task_handler` in `js_engine/handlers.rs` share the job budget and
  the script context and differ only in metadata and in what happens after
  the run. What `scheduler/mod.rs` does beyond computing the next slot
  (claiming, renewing the claim, retry backoff, fencing) is what `tasks.rs`
  already does, so it goes. Script schedules also gain lanes and the
  skip-while-unfinished rule.
- **The handler's metadata.** Both get `context.meta.task` and
  `context.meta.schedule = { key, scheduledFor }`.

### The names

Both move under the task objects, which is where their work ends up:

```ts
scriptTasks.schedule(spec); // init() only; declared with the code
scriptTasks.unschedule(key); // init() only
personalTasks.schedule(spec); // anywhere; the person's own
```

`schedulerService` is removed. The `init()`-only rule is the one asymmetry,
and an honest one; outside `init()` the script calls answer `{ ok: false,
reason }` as `schedulerService` does now, rather than throwing.

`registerOnce` is removed too. A plain one-off is `scriptTasks.enqueue({
runAt })`. Its other use — keyed one-offs re-declared on every `init()` to
warm caches soon after a deployment, spaced so that no two upstream fetches
share a budget — is `firstRunAfterMs` on the recurring schedule that does the
same work afterwards.

### Where the rows live

Either one `schedules` table with `user_id NULL` for a script's own (one
index, one firing loop; a redeploy deletes `WHERE user_id IS NULL`, a
revocation deletes by user; the unique key needs `NULLS NOT DISTINCT`,
Postgres 15+), or `personal_schedules` beside `scheduler_jobs` with one loop
reading both. The API is the same either way; decide when writing the
migration.

### Order

Two changes, not one:

1. Personal schedules, as above, on `ScheduleSpec` and the task-insert path.
2. Script schedules onto the same path: `scriptTasks.schedule`, removal of
   `schedulerService` and most of `scheduler/mod.rs`. Mostly deletion.

The second breaks scripts, so it moves `SCRIPT_API` again and changes the
script repositories in the same step: the virtual-world example's
`registerRecurring`, the editor's API help and the two `aiwebengine-dev`
pages that describe `schedulerService`.

## Changes, by repository

**Engine**

- Migration: `personal_schedules`, `script_tasks.schedule_id`.
- `personal_schedules.rs`: firing, the next slot, limits; called from the task
  worker.
- `tasks.rs`: write the outcome back to the schedule.
- `delegation.rs`: `revoke`, `revoke_all`, and `grant` (resume
  paused-expired).
- `security/secure_globals/tasks.rs` + the tasks prelude: `schedule`,
  `scheduleFrom`, `schedules`, `unschedule`.
- `auth/routes/account.rs` and `auth/routes/delegation.rs`: list, pause,
  resume, delete; the consent page note.
- `assets/aiwebengine.d.ts`, `assets/script-primer.md`, `docs/DELEGATION.md`,
  `docs/SCRIPT_TASKS.md`.
- `config.toml`: the three limits.
- `SCRIPT_API` in `lib.rs` moves to the day it lands.

**Script repositories**

- `aiwebengine-template`: a scheduled personal app (consent, `schedule`, the
  expiry notice).
- `aiwebengine-agent`: the authoring primer covers schedules; later, standing
  instructions built on them.
- `aiwebengine-examples`, `aiwebengine-dev`: a short example and the guide.

**The second change (script schedules)**

- Engine: `scriptTasks.schedule`/`unschedule` in
  `security/secure_globals/tasks.rs`; `jobs.rs` and the `schedulerService`
  prelude removed; `scheduler/mod.rs` reduced to the next slot and the
  `init()` declaration; the `.d.ts`, the primer and `SCRIPT_TASKS.md`;
  `SCRIPT_API` moves.
- Script repositories: every `schedulerService` call and the `.d.ts` copies.

## Tests

Integration tests in `tests/personal_schedules.rs`:

- declaring twice under one key leaves one row with the second spec;
- a due row creates exactly one task, as the person, in their lane — with two
  workers firing at once;
- a slot whose previous task is unfinished is skipped, not stacked;
- after a gap of several slots, one task, and the next slot is in the future;
- revoke deletes; `revoke_all` deletes; expiry pauses; granting again resumes;
- deleting the script and deleting the account remove the rows;
- three given-up runs pause as `failing`; a success resets the count;
- the limits refuse the eleventh, the 51st, and `* * * * *`;
- `schedule` refused without a grant, and from a read-only delegated run;
- `scheduleFrom` refused for an unlinked sender;
- DST: `0 2 * * *` in `Europe/Helsinki` across both changes;
- a schedule set before a redeployment still fires after it.

Unit tests beside the next-slot code for cron parsing and the interval floor.

For the second change, the existing scheduler tests move to the task path,
plus:

- a script schedule fires a script-context task, never a delegated one;
- redeploying replaces the script's schedules and leaves personal ones;
- `scriptTasks.schedule` outside `init()` answers `{ ok: false }` and
  registers nothing;
- `firstRunAfterMs` fires once soon after `init()`, then on the interval;
- a sub-minute `intervalMilliseconds` is accepted for a script schedule and
  refused for a personal one.

## Open questions

1. **Its own scope?** This design reuses the existing grant: any live grant
   allows a schedule set during the person's own visit, as `enqueue` does
   today. The alternative is a seventh scope, "run on a schedule while you are
   away", so consent says so explicitly. Reusing keeps consent short; the
   account page is where schedules become visible either way.
2. **Should the owner see a count?** "37 people have a schedule" helps the
   owner size the work and reveals little. Not in this change.
3. **Jitter.** A popular `0 7 * * *` fires many tasks at once. The task queue
   absorbs them, but outbound limits per script may not; a few seconds of
   spread per row may be needed after the pilot.
