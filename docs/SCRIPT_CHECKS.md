# Checking Solution Scripts

A script can be checked before it is deployed. The engine bundles it with its
own module resolution, runs its `init()` with every registration withheld, and
reports what it found.

This is deliberately not a type checker. `tsc` already sees the source; what it
cannot see is how the _engine_ resolves asset-backed imports, which handler
names actually resolve at dispatch, what `init()` costs against the budget a
deploy enforces, and what another script already serves. Those are the findings
here.

## Running a check

```bash
curl -X POST "https://your-engine/engine/check?uri=myapp"
```

| Parameter    | Default          | Meaning                                      |
| ------------ | ---------------- | -------------------------------------------- |
| `uri`        | —                | The script to check (required)               |
| `rollback`   | `true`           | Roll back the database writes `init()` makes |
| `timeout_ms` | 4x deploy budget | Ceiling for the `init()` run                 |

To check code that is not deployed yet — the point of the endpoint in an
editing loop — send it as the request body:

```bash
# Raw source, any content type but application/json
curl -X POST "https://your-engine/engine/check?uri=myapp" \
     --data-binary @myapp.ts

# Or as JSON, which also carries uri and rollback
curl -X POST "https://your-engine/engine/check" \
     -H "Content-Type: application/json" \
     -d '{"uri": "myapp", "content": "function init() {}"}'
```

A script is source text, so the body cannot be sniffed for structure — `{}` is a
valid program. The content type decides: `application/json` is parsed as
`{uri, content, rollback}`, anything else is taken as the source itself.

Checking runs the script's own code, so it needs the same rights as changing the
script: an administrator, or an owner who may write scripts. Sending candidate
content for a URI nothing is deployed at needs only the right to write scripts —
checking it is a preview of writing it.

## The report

Diagnostics are a report, not a failed request: a script full of errors still
comes back `200`. Callers read `ok`.

```json
{
  "scriptUri": "myapp",
  "ok": false,
  "diagnostics": [
    {
      "file": "myapp",
      "severity": "error",
      "code": "missing-handler",
      "message": "The route '/api/users' delegates to 'listUsers', but nothing named 'listUsers' is defined. …",
      "source": "engine"
    },
    {
      "file": "myapp",
      "severity": "warning",
      "code": "init-budget",
      "message": "init() took 4100ms of the 5000ms budget (82%). …",
      "source": "engine"
    }
  ],
  "init": {
    "ran": true,
    "durationMs": 4100,
    "budgetMs": 5000,
    "ceilingMs": 20000,
    "timedOut": false
  },
  "registrations": [
    {
      "kind": "route",
      "name": "/api/users",
      "method": "GET",
      "handler": "listUsers"
    }
  ],
  "timestamp": "2026-08-21T09:12:44.108Z"
}
```

`registrations` is worth reading even on a clean report: it is the deployed shape
of the script. A route the author expected and does not see there is a finding no
checker can raise for them.

`line` and `column` appear on a diagnostic when the engine can recover them from
the error — a stack frame or a transpiler message. `source` is `engine` for every
diagnostic today; the field exists so diagnostics from other checkers can be
merged into the same list without the caller guessing which layer complained.

### Diagnostic codes

| Code                 | Severity | Meaning                                                             |
| -------------------- | -------- | ------------------------------------------------------------------- |
| `circular-import`    | error    | An import chain leads back to a module already being compiled       |
| `invalid-import`     | error    | A specifier the engine's resolver does not accept                   |
| `unsupported-import` | error    | Dynamic `import()`, or `export` syntax in a root script             |
| `transpile-error`    | error    | The script or one of its modules does not compile                   |
| `init-failed`        | error    | `init()` threw                                                      |
| `init-timeout`       | error    | `init()` was still running at the check's ceiling and was stopped   |
| `init-blocked`       | error    | `init()` did not respond to being stopped — blocked in a host call  |
| `missing-handler`    | error    | A registration names a delegate the program does not define         |
| `script-not-found`   | error    | Nothing is deployed at that URI and no content was sent             |
| `no-init`            | warning  | The script defines no `init()`, so it registers nothing             |
| `no-registrations`   | warning  | `init()` ran but registered nothing                                 |
| `init-budget`        | warning  | `init()` spent 70% or more of the deploy budget                     |
| `init-budget`        | error    | `init()` ran past the deploy budget outright                        |
| `route-conflict`     | warning  | Another script already serves that path and method on a shared host |

## What the check catches that a local toolchain cannot

**Import cycles.** `tsc` resolves a cycle happily. The engine's bundler refuses
one outright, so a script with a cycle cannot deploy at all — it comes up with a
FATAL in its log and no routes. The diagnostic names the whole chain that closes
the loop:

```text
Circular asset-backed module import: server/a.ts -> server/b.ts -> server/a.ts
```

**Handler names that do not resolve.** Every entry point looks its delegate up
as a global function by name when a request arrives. A thin entrypoint that
registers `"listUsers"` while the function is exported from a module type-checks
perfectly and then 500s on the first request:

```ts
import { listUsers } from "./server/handlers.ts"; // tsc: fine

function init() {
  routeRegistry.registerRoute("/api/users", "listUsers", "GET"); // not a global
}
```

The fix is to make the delegate a global: `globalThis.listUsers = listUsers;`.

Because the check resolves delegates by _running_ the registration pass rather
than by parsing it, a handler name assembled at runtime is checked exactly like
a literal one.

**`init()` cost against the budget.** A deploy gives `init()`
`javascript.init_timeout_ms`, and a script that exceeds it comes up with only
the registrations it managed before the interrupt. The report gives the measured
cost — bundling, the program's top level, and `init()`, the same three steps a
deploy pays for — warning from 70% of the budget and erroring once past it.

The check deliberately runs `init()` with **headroom above the deploy budget**
(four times it by default). A check measures the cost; it does not enforce it.
Capping the run at the budget would make the one script that most needs an
answer — the one that is over budget — the only one that cannot get it: the run
would be interrupted at the ceiling and the report could say `interrupted` but
never `took 12.4s, 2.5x the budget`. Raise `timeout_ms` (up to 60s) for an
`init()` slower still.

**Paths another script already claims.** The route index is keyed by
`(host, path, method)`, so this is only reported where the two scripts' host
bindings overlap. Publishing the same path on two different hosts is the
multi-host feature working as intended.

## When `init()` is slow

A slow `init()` still produces a report. Which one depends on how it is slow:

- **Slower than the deploy budget, but finishing** — the usual case. Measured
  and reported as an `init-budget` error naming the real cost and the multiple
  of the budget.
- **Slower than the check's ceiling** — `init-timeout`. Nothing was measured, so
  the report says that rather than inventing a number, and lists the
  registrations `init()` made before it was stopped. Those are exactly what a
  deploy would install, so they are the useful half of the answer.
- **Not responding to being stopped** — `init-blocked`. The engine interrupts
  JavaScript between bytecode instructions, so an `init()` parked in a _host_
  call — a `fetch` with no timeout, an unbounded query — cannot be interrupted
  at all. A backstop gives up on the run and reports what it had collected, and
  the finding itself is the diagnosis: a deploy would hit the same wall and the
  script would come up with no route table.

In every case the registrations collected before the stop are reported. That is
the point of collecting them into a sink the request still holds rather than
into the abandoned run.

## What a check does and does not isolate

`init()` really runs. What the engine mediates is withheld:

- **Registrations are recorded, not applied.** Routes, streams, asset routes,
  GraphQL operations, MCP tools and prompts, scheduled jobs and message
  listeners are collected into the report and never reach the live registries.
  This is what makes it safe to check a candidate against a deployed script:
  without it, a broken candidate's `init()` would replace the running script's
  resolvers, listeners and jobs with its own, and nothing would undo that.
- **`schedulerService.clearAll()` clears nothing.**
- **`dispatcher.sendMessage()` dispatches nothing.** Dispatching runs _other_
  scripts' listeners against live data, and no rollback covers that.
- **Database writes roll back** with `rollback` (the default), including assets
  and secrets, which go through the same transaction. A script that calls
  `database.commitTransaction()` itself commits the check's transaction and
  defeats this.

What is not isolated is everything the engine does not mediate: an outbound
`fetch`, a write to a third-party system. Checking runs the script's own code —
which is also the point, since no static pass could tell you which handler names
resolve. In a cluster, the cache-invalidation notifications the repository sends
to peer instances go out on the pool rather than the transaction, so peers may
drop a cache entry for a write that then rolls back; they rebuild it on demand.

`init()` runs under the same budget a deploy enforces
(`javascript.init_timeout_ms`, falling back to `javascript.execution_timeout_ms`).

## Tool calls are bounded

Every MCP tool call has a backstop. A tool bounds its own run, but those bounds
are enforced by interrupting JavaScript between bytecode instructions — and a
handler parked in a _host_ call executes no bytecode, so nothing inside the run
can stop it. Without a backstop the request would simply never answer.

The backstop is per tool, sized to what that tool can legitimately need: a full
test run may take a minute, a check runs to its own ceiling, and a
script-registered tool is bounded by the JavaScript execution budget. One
ceiling for all of them would hold a request open far longer than a handler
with a two-second budget could ever justify.

It only fires once a tool is past every limit it sets for itself, which means
the tool is blocked somewhere no interrupt reaches — so the message says so, and
points at the usual cause: a request with no timeout, or a query without a
limit.

## Over MCP

The same check is the `check_script` tool, taking the same `uri`, `content`,
`rollback` and `timeoutMs` arguments and returning the same report — including
the partial one an `init()` that will not stop produces — so an agent can check the
code it is about to write without leaving the protocol:

```json
{
  "name": "check_script",
  "arguments": {
    "uri": "myapp",
    "content": "function init() { routeRegistry.registerRoute(\"/x\", \"h\", \"GET\"); }"
  }
}
```

Paired with [`run_tests`](SCRIPT_TESTS.md), that is the loop: check the candidate,
write it, run its tests.
