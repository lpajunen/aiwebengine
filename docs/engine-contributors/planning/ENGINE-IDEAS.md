# Engine Ideas

Ideas for the engine that are not commitments. What is committed, and ordered,
is `ROADMAP.md`; an idea moves there when somebody decides to do it, and leaves
this file when it is built or rejected.

## Observability

- **Metrics and tracing.** There is no metrics endpoint. Prometheus metrics and
  OpenTelemetry traces from the engine — and from scripts, so a solution's own
  business events can be measured — would make chains of events (request →
  task → stream message) followable across instances. `audit.record` and the
  correlated script log cover part of this today.
- **Timing from a script.** `performance.now()`, `mark` and `measure`, so a
  script can measure its own work without logging timestamps.

## JavaScript runtime

- **Timers.** `setTimeout` and `setInterval` do not exist: a host call blocks
  the script and there is no event loop. A delay is a scheduled job or a task
  today. A timer would either need an event loop per execution or be sugar over
  `scriptTasks` with a `runAt`.
- **Bidirectional connections.** Streams are Server-Sent Events only. A
  WebSocket route would serve games, collaborative editing and binary
  protocols, at the cost of a long-lived connection holding an execution model
  the engine does not have.

## Solution building blocks

- **Route middleware.** Cross-cutting checks (authorization, rate limits,
  validation) are repeated in each handler. A route option naming functions
  run before the handler would keep them in one place; the file route's
  `authorize` callback is the precedent.
- **Request validation.** A declared schema for a route's body and query,
  checked before the handler runs, with a 400 naming the field — the engine
  already holds JSON schemas for every operation.
- **Cache with expiry.** `scriptStorage` has no TTL. A `cache` with
  per-key expiry, pruned by the engine, would replace hand-written timestamp
  checks.
- **Sending email.** No mail API exists; a script calls a provider's HTTP API
  with a `{{secret:...}}` key. An engine-level sender would need configuration,
  a budget, and an answer to who may send as whom.
- **Fetch conveniences.** Retries with backoff and a status predicate on
  `fetch`, which every agent loop writes by hand.

## The developer loop

- **Type checking on the server.** Edit, check, test and deploy all run
  server-side and over MCP; `tsc` is the one step that still needs a checkout,
  so an agent working over MCP can write a type error that `check_script`
  passes. The question is where the ambient types and strictness come from.
- **A test's console output.** What a test case logs is not returned with its
  verdict, so assertion messages are the only channel out of a test.
- **An inline test module** in `run_tests`, so a throwaway case needs no file.
- **`init()` diagnostics for the live deploy.** `read_init_status` reports
  whether `init()` succeeded, not how long each phase took or the stack of the
  last failure; `check_script` measures only a sandbox run.

## Naming

- **"Script" for a whole solution.** A script is a tree of files with routes,
  tools, tables and tasks; the word suggests a single file. Whether the
  documentation should call it something else ("solution", "app") is open.
