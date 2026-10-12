# Roadmap

What is open, in one place. Each item says what is true today and what would
change it; finished work leaves this file, and its reasoning lives in the topic
docs (`docs/INDEX.md`) and the commit history. Ideas that are not commitments
are in `docs/engine-contributors/planning/`. What a first closed pilot with
outside users needs, as a checklist, is
[`PILOT-READINESS.md`](docs/engine-contributors/planning/PILOT-READINESS.md).

The order inside each section is the order they matter.

## Security

1. **Open client registration is still on.** Client ID Metadata Documents and
   `application_type` are in (`auth/client_metadata.rs`), and the consent page
   marks a self-registered client's name as its own claim. Turning
   `registration_endpoint` off — so every new client carries provenance — waits
   on the clients in use identifying themselves by document. Stored remote
   credentials keyed by issuer have nothing to key yet: the engine runs no
   OAuth flow as a client (Agents and MCP 2 is where that would start).
2. **Consent as a primitive.** `/auth/delegate` is a page the engine renders and
   a record it keeps. Generalised, a script needing approval for anything gets
   an auditable record instead of a boolean in its own table. Design it with the
   elevation page — the same page asked at two lifetimes.
3. **No machine-to-machine credential, on purpose.** Nothing unattended: an
   external agent completes a browser OAuth flow once and lives on refresh
   tokens, which forgive a replay for 30 seconds. If revisited, the question to
   answer first is whose roles and realm a userless token carries.
4. **The cross-origin check has no opt-out.** `security/cross_origin.rs` refuses
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
2. **No capability column on the operation table.** Each operation authorizes
   inside its core, and most checks are not one capability. Worth doing only
   together with moving the checks out of the cores.
3. **`mcp.ask` has no user.** No script in the four repositories calls it, so
   `mcp_elicitation.rs` buys conformance and its tests, not a user. It stays
   because nothing else does its job: an ask reaches the person driving the
   client, and a stream cannot carry an answer back into a tool call.
   Reconsider with the next MCP revision; removing it is one module and one
   prelude, with no storage.
4. **Not verified in a browser:** the editor's and admin's own flows after the
   operation-table and slug changes, and git sync against a real repository.

## Operations

1. **Settings in the database.** A proposal, with two decisions to take first —
   precedence with provenance, and a `--reset-settings` recovery path:
   [`docs/SETTINGS_IN_DATABASE.md`](docs/SETTINGS_IN_DATABASE.md).

## Contributor tooling

1. A shared `.claude/settings.json` allowlist in place of per-person entries,
   and project skills for the repeated workflows (run locally, run one test).
