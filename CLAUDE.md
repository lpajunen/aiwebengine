# CLAUDE.md

**aiwebengine** is a Rust engine (Axum, Postgres) that runs JavaScript/TypeScript
"scripts" in a sandboxed QuickJS runtime (`rquickjs`). Scripts are stored in
Postgres, have no build step, and register HTTP routes, MCP tools, streams and
jobs at runtime. Why things are shaped as they are:
`docs/engine-contributors/ARCHITECTURE.md`. Index of topic docs: `docs/INDEX.md`.

## Commands

```bash
source .env-local && cargo run     # run locally (config.toml + .env-local are tracked)
make postgres-local                # Postgres only; the engine and every test need it
make test                          # cargo nextest — the only supported runner
cargo nextest run <filter>         # one test;  --test <file_stem> for one file
make lint                          # clippy -D warnings + markdownlint
make format                        # cargo fmt + prettier
make typecheck                     # tsc over scripts/** against assets/*.d.ts
make check                         # format-check + lint + typecheck + test; run before committing
cargo run -- --validate-config     # check a config without starting the server
```

- `make test-simple` (`cargo test`) does not pass and is not a gate.
- Config: `config.toml` holds defaults; each environment overrides with
  `APP_<SECTION>__<KEY>` env vars (lists as JSON). `${VAR}` in TOML is **not**
  expanded.
- Long local runs: wrap in `caffeinate -i` — a sleeping Mac looks like a hang.

## Coding rules

- No `unwrap()`/`expect()` outside tests; use `AppError`/`AppResult`.
- Zero compiler and clippy warnings.
- Conventional commits: `feat:`, `fix:`, `refactor:`, `test:`, `docs:`, `perf:`, `chore:`; `!` for breaking.
- Breaking changes are fine: there is no backwards-compatibility requirement.
  Change the engine and the script repositories (`aiwebengine-examples`,
  `aiwebengine-dev`, `aiwebengine-agent`, `aiwebengine-template`,
  `aiwebengine-private`) together.
- Treat `src/security/`, `src/auth/` and `delegation.rs` as security-critical
  (`SECURITY.md`).
- Comments and docs describe what is true now. History goes in commit messages.

## Map

| Area                                               | Where                                                                                                    |
| -------------------------------------------------- | -------------------------------------------------------------------------------------------------------- |
| Startup, dynamic routing, `handle_dynamic_request` | `lib.rs`                                                                                                 |
| Scripts, files, logs, secrets, metadata cache      | `repository/` — one file per topic; `postgres.rs` is the `Repository` trait                              |
| Script execution                                   | `js_engine/` (`handlers.rs`: every handler path); build: `module_loader.rs`, `transpiler.rs` (oxc)       |
| Every JS global, and where it is authorized        | `security/secure_globals/` (one file per global) + `assets/*_prelude.js`                                 |
| Capabilities and `UserContext`                     | `security/capabilities.rs`                                                                               |
| Engine operations (one table)                      | `engine_api/operations.rs::native_tools`; HTTP from it in `engine_http.rs`                               |
| Routes, files, streams per host                    | `route_index.rs`, `hosts.rs`, `stream_registry.rs`                                                       |
| MCP server / client / tasks / ask                  | `mcp.rs`, `mcp_client.rs`, `mcp_tasks.rs`, `mcp_elicitation.rs`                                          |
| Auth: OAuth2/OIDC, local accounts, sessions        | `auth/`, `security/session.rs` — `docs/INTERNAL_AUTH.md`                                                 |
| History, pins, read-views                          | `revisions.rs`, `deployments.rs`, `source_view.rs`                                                       |
| Background work                                    | `scheduler/`, `tasks.rs`, `lease.rs` — `docs/SCRIPT_TASKS.md`                                            |
| Narrowing authority                                | `sandbox.rs`, `delegation.rs`, `script_limits.rs`, `security/elevation.rs` — `docs/WHAT_NARROWS_WHAT.md` |
| Git sync                                           | `git_sync.rs`, `git_github.rs` — `docs/GIT_SYNC.md`                                                      |
| Outbound HTTP                                      | `http_client.rs` — `docs/FETCH_CONCURRENCY.md`                                                           |
| Cluster sync                                       | `notifications.rs` (Postgres LISTEN/NOTIFY)                                                              |
| Engine HTML pages                                  | `engine_page.rs` + `assets/engine.css`                                                                   |

## Invariants

Breaking one of these is a bug even when every test passes.

**Authority**

- Authorization is by capability (`Capability`, `UserContext`), never by role
  name. Tiers: anonymous (read only) → authenticated (use a solution) → editor
  (author, always with an ownership check) → administrator (`AdministerEngine`:
  act on what you do not own). Never test `DeleteScripts` as "is admin".
- Every narrowing — realm, token audience, elevation, delegation,
  `sandbox.run`, script limits — goes through `UserContext::attenuated`, which
  only removes, never changes who is acting, and refuses rather than silently
  narrowing.
- All scripts are equal. A call is authorized against the calling user, never
  against which script made it. Engine administration is not exposed as JS
  globals; it is the operation table, reached by `/engine/*`, `/mcp` and
  `engine.call`.
- Anything that changes what an account may do (roles, realm, password,
  deletion) calls `security::delete_sessions_for_user`, which also drops
  refresh tokens and delegations.

**A script is one tree**

- A script's files are rows of `assets` keyed `(script_uri, path)`. The
  entrypoint is the file `main.{ts,js,tsx,jsx}`. `scripts` holds identity,
  ownership and status, no content.
- Writing or deleting a `main.*` takes `WriteScripts`/`DeleteScripts` plus
  ownership, by every route in. The JS `files` global refuses it. 1MB ceiling.
- A write to a file must go through `repository::invalidate_script_asset_caches`
  (skipped for pinned scripts). Every write records a revision.
- Exposure is the directory: `public/` may be served by a file route,
  `resources/` may be an MCP resource, everything else is private. A
  registration outside its directory is refused.
- Script names are slugs (`slug.rs`), checked on create and rename only.

**Operations**

- A new engine operation is one entry in `native_tools`: name, description,
  JSON schema, `fn(&Value, &UserContext)`. Do not hand-write HTTP handlers or
  OpenAPI for it. Read-only ones go in `engine_http::READ_ONLY`.
- A refusal returns `engine_api::Refusal` with its kind; `engine_http::status_of`
  maps it to a status. `{ ok: false }` reports are answers (200).
- Keep tool descriptions short: what it does, plus the one rule that is not
  obvious. They are paid for in every agent's context.

**JavaScript API**

- Every global is wrapped by a prelude: it returns values and throws `Error`s.
  Never return JSON-in-a-string or `"Error: ..."` values.
- Types are in `assets/aiwebengine.d.ts`; `engine.call` argument types are
  generated from the operation table (`engine_types.rs`). Update the `.d.ts`
  with every API change.
- `.json`, `.md` and `.txt` imports are data modules; their content is never
  parsed as code.

**HTTP edge**

- Security headers, CSP and CORS apply to engine-owned paths
  (`RESERVED_ROUTE_PREFIXES`) only, and never overwrite a header already set.
- The client IP comes from `security/client_ip.rs` only; forwarding headers are
  trusted only from `server.trusted_proxies`.
- Session cookies are host-only, `__Host-` when `Secure`, `SameSite=Lax`. `/mcp`
  accepts only a bearer token whose audience names that host's `/mcp`.
- `server.management_hosts` gates `/engine/*` and the native MCP tools.

**Routing**

- Routes are indexed by `(host, path, method)`. The first script stored holds a
  path; later claims are refused and listed in `exposure_report`. A file route
  resolves before a stream, which resolves before a handler.

## Tests

- Integration tests: `tests/*.rs`, helpers in `tests/common/mod.rs` (sign in as
  `AdminServer`). Unit tests sit beside the code; many need Postgres.
- Each test process gets its own database cloned from a migrated template
  (`src/test_db.rs`). Reach the database only through `common::test_pool` /
  `test_db::pool`; never build your own connection string.
- No skipping on a missing database, no retries, no serial groups. A flaky test
  is a race to fix.
- A changed migration needs `DROP DATABASE aiwebengine_test_template`. Check a
  migration against a copy of real data too (`CREATE DATABASE x TEMPLATE aiwebengine`).
