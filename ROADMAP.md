# Roadmap

What is open, in one place. Each item says what is true today and what would
change it; finished work leaves this file, and its reasoning lives in the topic
docs (`docs/INDEX.md`) and the commit history. Ideas that are not commitments
are in `docs/engine-contributors/planning/`. What a first closed pilot with
outside users needs, as a checklist, is
[`PILOT-READINESS.md`](docs/engine-contributors/planning/PILOT-READINESS.md).

The order inside each section is the order they matter.

## Security

1. **A stored `/mcp` token launders the delegation cap.** `delegation.rs` bounds
   a delegated task's in-process `UserContext`, and says nothing about an
   outbound `Authorization` header: a person's token in `user_secrets` makes a
   delegated turn act with whatever that token carries. Elevation-scoped tokens
   bound how much that is, not whether it happens. Decide whether a delegated
   execution may send a bearer token to this engine's own `/mcp` at all.
2. **Account erasure stops at the engine's tables.** `delete_user` takes
   sessions, grants and git credentials; a person's rows in a script's own
   tables stay. Make `personalStorage` and a per-person table namespace the
   attractive place for personal data, so erasure is an engine guarantee
   rather than each solution's promise.
3. **Native tool calls have no budget.** `GitSync` and `ChannelTrigger` are
   bounded; an agent loop calling `write_file` is not. A `RateLimitKey` per
   account for the operation table.
4. **The engine's security audit has no actor dimension.** `security/audit.rs`
   records the person, so a change an agent made as them reads like one they
   made by hand. Carry the principal kind, `UserContext.attenuated` and the
   elevation method into the audit line.
5. **The 2026-07-28 authorization hardening, the rest of it.** The `iss`
   half of RFC 9207 is done. Left: Client ID Metadata Documents in place of
   open Dynamic Client Registration (a `client_id` that carries provenance,
   where today only the per-user consent in `oauth_client_grants` does),
   `application_type` on registration, and stored remote credentials keyed by
   issuer.
6. **Consent as a primitive.** `/auth/delegate` is a page the engine renders and
   a record it keeps. Generalised, a script needing approval for anything gets
   an auditable record instead of a boolean in its own table. Design it with the
   elevation page — the same page asked at two lifetimes.
7. **No machine-to-machine credential, on purpose.** Nothing unattended: an
   external agent completes a browser OAuth flow once and lives on refresh
   tokens, which forgive a replay for 30 seconds. If revisited, the question to
   answer first is whose roles and realm a userless token carries.
8. **The cross-origin check has no opt-out.** `security/cross_origin.rs` refuses
   a signed-in state-changing request from another origin. A route that must
   accept one (a credentialed cross-origin API) would need a route option;
   add it when such a route exists.

## Agents and MCP

1. **The engine cannot call its own `/mcp` in local development.**
   `HttpClient::is_private_ip` refuses loopback, so anything built on the
   outbound path can only be developed against production.
2. **A refresh token has nowhere safe to live.** `{{secret:...}}` substitutes
   into headers and URLs, not bodies, and an OAuth refresh posts the token in a
   form body — so a script holding one keeps it in `personalStorage`, in the
   clear.
3. **MCP surface gaps.** No tool for `/engine/health/cluster`, none for the two
   streams (`/engine/script_updates`, `/engine/script_logs/stream`), and
   delegation grants are listed only on `/auth/account`.
4. **URL-mode elicitation.** Form mode works over plain POST (`mcp.ask`). URL
   mode needs server-side state, which the rest of MRTR is arranged to avoid —
   a decision before it is an implementation.
5. **`input_required` inside an MCP task.** Representable, unreachable: a queued
   run asking a person a question runs in script context or as somebody away.
   `tasks/update` acknowledges and ignores until that is designed.
6. **`subscriptions/listen`.** Not wanted yet — `ttlMs` covers list caching —
   but it is where progress notifications during a long tool call, and a true
   `listChanged`, would live.
7. **The agent's status half** (in `aiwebengine-agent`). Two meta-tools rather
   than fifty schemas, answers that link into the dev UI rather than render
   tables as prose, and long tools (`run_tests`, `pull_from_git`) run as
   enqueue-and-report: `NATIVE_TOOL_CEILING_MS` is 30 s against a 60 s job.

## The script API

1. **No event loop.** A host call blocks the script: no `setTimeout`, no
   overlap of a fetch with computation, `await` sequences finished work.
   `fetchAll` covers parallel network calls. The rest means an event loop under
   QuickJS with host calls that yield.
2. **Handlers are named by string and must be globals of `main.ts`.** The
   primer's most common failure. `check_script` reports it (`missing-handler`),
   but a deploy that skips the check still fails on the first request.
   Accepting function values would remove the failure rather than report it.
3. \*\*Schema and data access. `database` is ten methods and `ensureTable` in
   `init()`. Typed rows, declarative schema changes and aggregates are the
   gaps worth weighing; a raw-SQL escape hatch is not.
4. \*\*Web-standard globals. `URLSearchParams` and `Headers` exist; `URL`,
   `TextEncoder`/`TextDecoder` and `structuredClone` do not, and `fetch` is
   synchronous rather than WHATWG's promise-returning shape.

## Engine internals

1. **`lib.rs` is the largest file left** (4.3k lines): startup, the dynamic
   router and the `/mcp` dispatch in one module. The MCP dispatch is the seam.
2. **Some refusals are still read from their text.** The layers below the
   operations (`authorize_script_write`, the revision resolvers, git and
   storage helpers) return `Result<_, String>`, so `Refusal::from_message`
   classifies them. It shrinks as those layers get error types.
3. **No capability column on the operation table.** Each operation authorizes
   inside its core, and most checks are not one capability. Worth doing only
   together with moving the checks out of the cores.
4. **Unused security scaffolding.** `security/threat_detection.rs`,
   `security/validation.rs`'s `InputValidator`, `security/operations.rs` and
   `safe_helpers.rs` have few or no callers. Delete what nothing uses.
5. **Unit tests that skip without a database.** `js_engine.rs`'s test helpers
   return early on `should_skip_db_tests`, which the test rules forbid.
6. **`mcp.ask` has no user.** No script in the four repositories calls it, so
   `mcp_elicitation.rs` buys conformance and its tests, not a user. It stays
   because nothing else does its job: an ask reaches the person driving the
   client, and a stream cannot carry an answer back into a tool call.
   Reconsider with the next MCP revision; removing it is one module and one
   prelude, with no storage.
7. **Not verified in a browser:** the editor's and admin's own flows after the
   operation-table and slug changes, and git sync against a real repository.
8. **`run_tests` with a `filter` that matches nothing** answers "No test
   modules found", which describes a different failure and sends the caller
   looking for a discovery bug.

## Operations

1. **Settings in the database.** A proposal, with two decisions to take first —
   precedence with provenance, and a `--reset-settings` recovery path:
   [`docs/SETTINGS_IN_DATABASE.md`](docs/SETTINGS_IN_DATABASE.md).

## Contributor tooling

1. A shared `.claude/settings.json` allowlist in place of per-person entries,
   and project skills for the repeated workflows (run locally, run one test).
