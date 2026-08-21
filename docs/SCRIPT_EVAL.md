# Evaluating Snippets Against a Script

A snippet can be run against a deployed script's sandbox and its value read
back. The script's own program is loaded first, so the snippet sees what that
program defined.

This replaces the loop it used to take to answer a small question — author a
test file, deploy it, run the suite, decode the answer out of an assertion
message, delete the file again:

```bash
curl -X POST "https://your-engine/engine/eval?uri=myapp" \
     --data-binary 'database.query("players", "{\"world_id\":3}", 20)'
```

## Running one

| Parameter    | Default           | Meaning                                         |
| ------------ | ----------------- | ----------------------------------------------- |
| `uri`        | —                 | The script whose sandbox to run in (required)   |
| `rollback`   | `true`            | Roll back the database writes the snippet makes |
| `timeout_ms` | execution timeout | Budget, clamped to the engine's own ceiling     |

The snippet itself is the request body. Send it raw under any content type but
`application/json`, or use a JSON envelope carrying everything at once:

```bash
curl -X POST "https://your-engine/engine/eval" \
     -H "Content-Type: application/json" \
     -d '{"uri": "myapp", "source": "totalCents(basket)", "rollback": false}'
```

Evaluating runs the script's code with your capabilities, so it takes the same
rights as changing the script: an administrator, or an owner who may write
scripts. That is the same bar as `run_tests`, and for the same reason — it is
the same act.

## What the snippet can see

The script's prepared program is evaluated first, in the same context, so the
snippet reaches:

- the script's own top-level functions,
- the bindings its entrypoint imported — the bundler rewrites `import { x } from
"./m.ts"` into a top-level declaration, so `x` is simply in scope,
- `__asset_module_require__("path/to/module.ts")`, for a module the entrypoint
  imported but did not re-expose.

`__asset_module_require__` only knows modules that are actually in the bundle. A
module nothing imports is not compiled into it, and asking for it throws
`Unknown asset module`.

The last expression is the value. Write `someHelper(1)`, not
`return someHelper(1)`.

## The report

```json
{
  "scriptUri": "myapp",
  "ok": true,
  "value": [{ "id": 4, "name": "ana" }],
  "valueType": "array",
  "console": [
    { "level": "LOG", "message": "querying…", "timestampMs": 1755765164108 }
  ],
  "durationMs": 12,
  "rolledBack": true,
  "timestamp": "2026-08-21T09:12:44.108Z"
}
```

A snippet that throws is still a `200` — you asked what the code does, and "it
throws" is the answer. Read `ok`, and `error` for the message.

`valueType` is always reported: `undefined`, `null`, `boolean`, `number`,
`string`, `symbol`, `function`, `array` or `object`. It is close to `typeof`,
with `null` and `array` named rather than both collapsing to `object`. It exists
because `value` alone cannot tell `undefined` from `null` — the first is omitted
from the response, the second is a JSON `null`.

`value` is absent when the result has no JSON form: `undefined`, a function, a
symbol. When the result _should_ have had one but could not be rendered — a
circular structure, most often — `value` is absent and `valueError` explains
why, while `ok` stays true. The snippet ran fine; only its value could not be
carried back.

`rolledBack` reports what happened, not what was asked for. A snippet that calls
`database.commitTransaction()` itself commits the evaluation's transaction, and
the response says so.

## Console capture

`console.log` and friends are captured into the response as well as written to
the script's log.

That is not a convenience — it is what makes the default mode work. Console
writes go through the repository, so they join whatever transaction is open, and
an evaluation that rolls back would otherwise roll back its own output. Capture
is what keeps it.

Capture stops after 1,000 lines; anything past that is counted in
`consoleDropped` rather than silently lost.

## What an evaluation does and does not isolate

- **Database writes roll back** with `rollback` (the default), including asset
  and secret writes, which go through the same transaction.
- **Registrations do nothing.** `routeRegistry`, `graphQLRegistry`,
  `schedulerService` and `dispatcher.registerListener` stay callable and report
  that they did nothing — a route or job registered from a snippet would outlive
  the request with no rollback to undo it. This is the same rule a test run
  follows.
- **Not isolated:** anything the engine does not mediate — an outbound `fetch`,
  a write to a third-party system.

## Snippets run synchronously

Host calls like `fetch()` and `database.query()` block rather than yielding;
there is no job queue. A snippet that returns a promise would never settle, so
it is reported as an error saying exactly that rather than hanging until the
budget runs out. Write snippets without `async`/`await`.

## Over MCP

The same evaluation is the `eval_script` tool, taking `uri`, `source`,
`rollback` and `timeoutMs` and returning the same report:

```json
{
  "name": "eval_script",
  "arguments": { "uri": "myapp", "source": "Object.keys(globalThis).length" }
}
```

With [`check_script`](SCRIPT_CHECKS.md) and [`run_tests`](SCRIPT_TESTS.md), that
is the loop: check the candidate, write it, run its tests, and poke at the
result without deploying anything to do so.
