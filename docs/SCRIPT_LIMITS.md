# What One Script May Spend

```bash
# Raise it for a script that waits on a slow API
curl -X POST "https://your-engine/engine/limits" -H 'Content-Type: application/json' \
  -d '{"script":"myagent","timeoutMs":60000,"note":"calls a model API"}'

# Contain one that has started holding execution slots
curl -X POST "https://your-engine/engine/limits" -H 'Content-Type: application/json' \
  -d '{"script":"noisy","timeoutMs":1000,"note":"runaway loop, see incident 412"}'

# What is overridden anywhere in this engine
curl "https://your-engine/engine/limits"

# Put one back on the engine's own limits
curl -X DELETE "https://your-engine/engine/limits?script=noisy"
```

`javascript.execution_timeout_ms` and its neighbours are process-wide. That is
the problem: an operator hosting one agent beside twenty ordinary solutions had
to raise the ceiling for all of them, so the number that lets an agent wait out
a model call is also the number a runaway route handler now gets. Making one
solution work made every other one worse.

An override is per script, and useful in both directions. Raising is the
obvious one. **Lowering is the one that gets used** — a script that has started
holding execution slots can be contained without restarting the engine and
without touching anyone else.

## What can be overridden

| Field            | Bounds        | What it is                                   |
| ---------------- | ------------- | -------------------------------------------- |
| `timeoutMs`      | 100ms – 10min | Wall clock for one request-shaped invocation |
| `jobTimeoutMs`   | 100ms – 1h    | Wall clock for one scheduled job or task     |
| `maxMemoryBytes` | 8MB – 2GB     | Heap ceiling for this script's runtime       |

Every field is optional, and one left out follows the engine. A script with no
row behaves exactly as it did before any of this existed — which is the
property that makes the feature safe to turn on.

Values outside the bounds are **clamped rather than refused**: the caller asked
for as much as possible and gets it. The bounds exist because a stored value
takes effect without a restart, so a mistyped one would hold a slot until
somebody noticed.

Setting **replaces rather than merges**, so the row always says the whole of
what is in force. A caller that omits a field is saying that field follows the
engine — not that it keeps an earlier override they cannot see.

The request budget and the job budget stay distinct even when both are set on
the same script. A job is not answering a request.

## Who may set one

An administrator, and deliberately **not** the script's owner.

Ownership is the right test for changing what a script _does_. This is a claim
on the engine's execution slots, its threads and its memory, which are shared
with every other tenant. An owner who could raise their own ceiling would be
back to one script setting the policy for all of them, which is the thing this
exists to stop.

Reading is gated the same way: what one script is allowed to spend says
something about the engine's capacity and about other tenants' arrangements.

## Several instances

The value is read on every execution, so it cannot be a query — the same
reasoning `deployments.rs` gives for its pins, and the cache is built the same
way. It is loaded once at startup _before any script runs_, so an instance
coming up does not briefly run a contained script at the engine's own ceiling,
and `notifications.rs` carries a change to peers.

## When to reach for this, and when not to

Raising a job budget is usually the wrong answer to "my work does not fit".
Work longer than one budget wants splitting across tasks, which survives a
restart where one long execution does not — see [Script Tasks](SCRIPT_TASKS.md).

What an override is genuinely for is the case where a single unit of work
legitimately takes longer than an ordinary request: one model call, one slow
third party. And for containment, where it has no substitute.
