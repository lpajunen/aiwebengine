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

| Parameter  | Default | Meaning                                      |
| ---------- | ------- | -------------------------------------------- |
| `uri`      | —       | The script to check (required)               |
| `rollback` | `true`  | Roll back the database writes `init()` makes |

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
  "init": { "ran": true, "durationMs": 4100, "budgetMs": 5000 },
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
| `init-failed`        | error    | `init()` threw or ran out of budget                                 |
| `missing-handler`    | error    | A registration names a delegate the program does not define         |
| `script-not-found`   | error    | Nothing is deployed at that URI and no content was sent             |
| `no-init`            | warning  | The script defines no `init()`, so it registers nothing             |
| `no-registrations`   | warning  | `init()` ran but registered nothing                                 |
| `init-budget`        | warning  | `init()` spent 70% or more of the deploy budget                     |
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
`javascript.init_timeout_ms`, and a script that exceeds it comes up with no
routes registered. The report gives the measured cost — bundling, the program's
top level, and `init()`, the same three steps a deploy pays for — and warns from
70% of the budget, while there is still margin to act on.

**Paths another script already claims.** The route index is keyed by
`(host, path, method)`, so this is only reported where the two scripts' host
bindings overlap. Publishing the same path on two different hosts is the
multi-host feature working as intended.

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

## Over MCP

The same check is the `check_script` tool, taking the same `uri`, `content` and
`rollback` arguments and returning the same report — so an agent can check the
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
