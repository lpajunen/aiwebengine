# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this project is

**aiwebengine** is a Rust application engine that lets "solution developers" write JavaScript/TypeScript scripts which are executed inside an embedded, sandboxed QuickJS runtime (via `rquickjs`). The Rust core handles HTTP (Axum), routing, GraphQL (async-graphql), MCP (Model Context Protocol), auth (OAuth2/OIDC + sessions), a PostgreSQL-backed script/asset repository, and a capability-based security sandbox around every JS global. Scripts register HTTP route handlers, GraphQL resolvers, scheduled jobs, and message listeners at runtime — there's no separate build step for scripts, they're stored in Postgres and (re)loaded dynamically.

## Common commands

```bash
# Build & run
cargo build --release
source .env && cargo run                    # requires config.toml (copy from config.local.toml) and .env (from .env.example)
make dev                                     # cargo-watch auto-reload
make dev-local                               # run with localhost OAuth redirect (http://localhost:3000)

# Tests
make test                                    # cargo-nextest run --all-features --no-fail-fast (preferred)
make test-simple                             # plain `cargo test --all-features`
cargo nextest run <test_name_substring>      # run a single test / filter by name
cargo nextest run --test <file_stem>         # run one integration test file, e.g. `cargo nextest run --test dispatcher`
make coverage                                # cargo llvm-cov --all-features --html -> target/llvm-cov/html/index.html

# Lint / format / typecheck
make lint                                    # cargo clippy --all-targets -- -D warnings, plus markdownlint
make format                                  # cargo fmt --all + prettier for md/js/ts
make format-check                            # cargo fmt --all -- --check (CI-safe, no writes)
make typecheck                               # npm run typecheck -> tsc against scripts/**/*.{js,ts,tsx} using tsconfig.typecheck.json
make check                                   # format-check + lint + typecheck + test-simple (run before committing)
make ci                                      # check + coverage

# Docker (Postgres is required; there's no in-memory fallback)
make docker-setup && make docker-prod        # first-time production-style setup
make docker-localhost                        # https://localhost, self-signed cert, no DNS needed
make postgres-local                          # start only Postgres for local `cargo run` development
```

Config files are environment-specific templates copied to `config.toml` (`config.local.toml` / `config.staging.toml` / `config.production.toml`), and are overridden via `APP_<SECTION>__<SUBSECTION>__<KEY>` env vars (figment). `cargo run -- --validate-config` validates config without starting the server.

## Architecture

### Request lifecycle

1. `main.rs` loads config (`config::AppConfig`), sets up tracing, then calls `start_server_with_config` in `lib.rs`.
2. `lib.rs::initialize_components` brings up the DB (`database.rs`), the global `repository::PostgresRepository` (`repository.rs`, ~7k lines — the source of truth for scripts, assets, logs, secrets, and script metadata), the Postgres LISTEN/NOTIFY-based `notifications.rs` (keeps a multi-instance cluster's script caches in sync), the `scheduler` module, and bootstraps built-in scripts/assets from `scripts/feature_scripts/*.js`.
3. Every script is executed once at startup (`execute_startup_scripts`) to populate route/GraphQL/scheduler registrations, then each script's `init()` is invoked if present (`script_init.rs`).
4. Incoming HTTP requests that don't match a static Axum route are matched dynamically against script-registered routes: `find_route_handler` in `lib.rs` scans cached `registrations` on script metadata (populated by the script's `init()` call to `routeRegistry.registerRoute`), scored by specificity (exact > `:param` > `/*` wildcard).
5. Matched requests are dispatched into `js_engine::execute_script_secure` (or the streaming/GraphQL-specific variants), which spins up a fresh QuickJS `Runtime`/`Context`, injects the sandboxed globals, transpiles/bundles the script via `module_loader.rs` + `transpiler.rs` (TS/JSX → JS using `oxc`), and runs the named handler with a per-request `UserContext`.

### Security sandbox model

- Every JS global exposed to scripts goes through `security::secure_globals` (the largest file in the repo, ~6.2k lines) — it is the enforcement point, not `js_engine.rs`.
- Authorization is capability-based (`security::capabilities::Capability` + `UserContext`), not role-string checks. `UserContext::anonymous()/authenticated()/admin()` map to fixed capability sets; anonymous users get elevated capabilities only when `AIWEBENGINE_MODE=development`.
- Scripts are **privileged** or **restricted** (`repository.rs`: `privileged` column on scripts, `is_script_privileged`/`set_script_privileged`). Privileged scripts get full engine-internal API access; restricted scripts only get what's explicitly granted. `PRIVILEGED_BOOTSTRAP_SCRIPTS` in `repository.rs` hardcodes the built-in `core`/`cli`/`admin`/`auth` scripts as privileged by default.
- Other `security/` submodules: `csrf.rs`, `encryption.rs` (AES-GCM, used for sessions and at-rest secrets), `rate_limiting.rs`, `session.rs`, `audit.rs`, `threat_detection.rs`, `validation.rs`, `csp.rs`. Treat this whole directory as security-critical — changes here need extra scrutiny per `SECURITY.md`.

### JavaScript-facing API surface

- Script access is split into **script-internal** (only the script itself), **engine-internal** (any script in the same engine instance), and **external** (HTTP-exposed) — see README.md's API access model.
- Built-in engine scripts live in `scripts/feature_scripts/` (`core.js`, `auth.js`, `admin.js`, `cli.js`) and are bootstrapped into the DB at startup; example/demo scripts live in `scripts/examples/`; ad hoc test fixtures live in `scripts/test_scripts/`.
- TypeScript type declarations for the public/private JS APIs are served dynamically at `/api/types/v{version}/aiwebengine.d.ts` and `...-priv.d.ts` (generated, referenced in `lib.rs`'s OpenAPI setup). `tsconfig.typecheck.json` + `assets/**/*.d.ts` are what `make typecheck` validates scripts against.
- `module_loader.rs` implements a minimal CommonJS-like bundler for asset-backed script imports (no dynamic `import()`); `transpiler.rs` handles TS/JSX/TSX transpilation via `oxc`.

### Other core modules

- `graphql.rs` / `graphql_schema_gen.rs` / `graphql_ws.rs`: dynamic GraphQL schema built from script-registered resolvers (`async-graphql` dynamic-schema feature), rebuilt on demand; subscriptions available over WebSocket (`graphql-transport-ws`) and SSE (`/graphql/sse`).
- `mcp.rs` / `mcp_client.rs`: JSON-RPC 2.0 Model Context Protocol endpoint (`/mcp`) for AI tool integration, both as a server (exposing script-registered tools) and a client (`mcp_client.rs` for scripts calling out to other MCP servers).
- `auth/`: OAuth2/OIDC (Google, Microsoft, Apple providers under `auth/providers/`), PKCE, session management, JWT, dynamic client registration — this is the "Phase 1/2" auth system referenced throughout the code.
- `scheduler/`: cron-like job scheduling that scripts register into; runs as a background worker task started alongside the HTTP server (`scheduler::spawn_worker`) and shut down via the same graceful-shutdown channel.
- `stream_manager.rs` / `stream_registry.rs`: Server-Sent Events connection management for real-time script-driven streams, with optional per-stream customization functions (JS callbacks that filter/authorize connections).
- `dispatcher.rs`: pub/sub-style message dispatch between scripts (listeners registered by URI/handler, keyed by message type).
- `error.rs` / `error/app_error.rs`: unified `AppError`/`AppResult` used across the whole crate; `error_to_response` in `lib.rs` converts these into HTTP responses.
- Multi-instance deployments coordinate via Postgres LISTEN/NOTIFY (`notifications.rs`) rather than a separate message bus — each instance gets a generated server ID (`notifications::generate_server_id`) used for both notifications and `/health/cluster` reporting.

### Testing

- Integration tests live in `tests/*.rs` (one file per concern: `dispatcher.rs`, `streaming.rs`, `security_capabilities.rs`, `security_sessions.rs`, `secrets.rs`, `openapi_validation.rs`, `http_fetch.rs`, `cluster.rs`, etc.) with shared helpers in `tests/common/mod.rs` and `tests/test_utils.rs`.
- These are full-stack tests that spin up real server instances against Postgres — there is no in-memory/mocked repository mode, so a running Postgres (e.g. via `make postgres-local`) is generally required to run the suite.
- `cargo-nextest` is the preferred runner (faster, better output); the `[profile.test]` in `Cargo.toml` trades some optimization for faster incremental test builds.

### Coding standards (from CONTRIBUTING.md)

- No `unwrap()`/`expect()` in production code — use `Result<T, E>` and the crate's `AppError`.
- Zero compiler warnings, zero clippy warnings (`cargo clippy --all-targets -- -D warnings` must pass clean).
- Conventional commit prefixes: `feat:`, `fix:`, `refactor:`, `test:`, `docs:`, `perf:`, `chore:`.
