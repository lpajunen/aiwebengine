# Testing Solution Scripts

A script can carry its own tests. They live in its assets, import the code they
cover, and run on request inside the same sandbox that serves the script — so a
test exercises the engine APIs the real handlers use, not a mock of them.

## Writing a test

A test module is an asset named `*.test.ts` (or `.js`, `.jsx`, `.tsx`). Put it
anywhere in the script's asset tree — a `tests/` folder, or beside the code it
covers.

```ts
// tests/basket.test.ts
import { totalCents } from "../server/basket.ts";

describe("basket", () => {
  test("an empty basket totals zero", () => {
    expect(totalCents([])).toBe(0);
  });

  test("rejects a negative quantity", () => {
    expect(() => totalCents([{ cents: -1 }])).toThrow("negative");
  });
});
```

Imports name the asset path exactly, extension included — the same rule the
engine uses everywhere else, since there is no extension resolution.

Available in a test module: `test` (and its alias `it`), `describe`,
`beforeEach`, `afterEach`, `expect`, and `assert`. `describe` composes into the
reported name, so the first case above is reported as
`basket > an empty basket totals zero`. Full signatures are in the type
declarations served at `/engine/types/v1/aiwebengine.d.ts`.

Every engine global the script normally has — `database`, `assetStorage`,
`fetch`, and the rest — is available inside a test.

## Running them

```bash
curl -X POST "https://your-engine/engine/run_tests?uri=myapp"
```

| Parameter  | Default | Meaning                                      |
| ---------- | ------- | -------------------------------------------- |
| `uri`      | —       | The script whose tests to run (required)     |
| `filter`   | none    | Run only cases whose name contains this text |
| `rollback` | `true`  | Roll back the database writes the tests make |

The response is a report, and a failing test is still a `200` — the request
succeeded, the tests did not:

```json
{
  "scriptUri": "myapp",
  "success": false,
  "total": 3,
  "passed": 2,
  "failed": 1,
  "durationMs": 41,
  "timedOut": false,
  "cases": [
    {
      "name": "basket > an empty basket totals zero",
      "file": "tests/basket.test.ts",
      "status": "passed",
      "durationMs": 3
    },
    {
      "name": "basket > rejects a negative quantity",
      "file": "tests/basket.test.ts",
      "status": "failed",
      "durationMs": 5,
      "error": "Expected [Function] to throw an error matching \"negative\""
    }
  ]
}
```

`success` is false for a script with no test modules too: nothing ran, so
nothing passed.

Running tests executes the script's code, so it requires the same rights as
changing the script — an administrator, or an owner who may write scripts.

## What a run does and does not isolate

Each test module gets its own runtime and context. One file cannot see globals
another file set, and a case is always attributed to the file it came from.

With `rollback` (the default), the run holds a transaction it never commits, so
**database writes disappear** when it ends. Nothing else does: assets written,
secrets written, and outbound `fetch` calls are real and survive the run. A test
that calls `database.commitTransaction()` itself commits the run's transaction
and defeats the rollback.

Registrations are switched off during a run — `routeRegistry`, `graphQLRegistry`,
and `schedulerService` calls do nothing, because a route or job registered by a
test would outlive it and no rollback undoes that.

## Budgets

Two limits bound a run, both configurable under `[javascript]`:

- `test_timeout_ms` — budget for one test module. Each module runs in its own
  runtime and gets this budget of its own, so one slow file does not spend the
  time the rest need.
- `test_run_timeout_ms` — ceiling on the whole run, so a script with many test
  files cannot hold a request open for modules × `test_timeout_ms`.

Either limit reached, the report comes back with `timedOut: true` and the
verdicts already reached. A case the interrupt stopped gets no verdict at all
rather than a failure it did not earn.

## Tests run synchronously

Scripts execute without a job queue: host calls like `fetch()` and
`database.query()` block rather than yielding. A test body that returns a
promise would never settle, so it fails with a message saying as much instead of
reporting a pass it never earned. Write test bodies without `async`/`await`.
