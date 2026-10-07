# aiwebengine

[![License: AGPL v3](https://img.shields.io/badge/License-AGPL_v3-blue.svg)](https://www.gnu.org/licenses/agpl-3.0)
[![Rust Edition](https://img.shields.io/badge/rust-2024-orange.svg)](https://www.rust-lang.org)

An engine for JavaScript/TypeScript solutions — websites, HTTP APIs, web apps,
MCP tools and agents — built to be developed with generative AI. It is written
in Rust (Axum, Postgres) and runs each solution's scripts in a sandboxed QuickJS
runtime.

⚠️ **Work in progress.** Core functionality works and is used in production;
APIs change without notice.

## What a solution is

A **script** is a tree of files stored in the engine's database, with an
entrypoint `main.{ts,js,tsx,jsx}`. There is no build step: the engine
transpiles and bundles on load. A script's `init()` registers what it publishes
— HTTP routes, streams, MCP tools, prompts and resources, scheduled jobs — and
which of its files the world may reach is decided by directory: `public/` may be
served, `resources/` may be an MCP resource, everything else is private.

All scripts are equal: each sees the same JavaScript API, and what a call may do
depends on the calling user — their capabilities, whether they own the target
script, and whether they hold the editor or administrator role. The engine knows
three roles and a script cannot add one; whether a person may read a solution's
record is decided by that solution's handler, from `context.request.auth` and
its own tables.

## Developing against an engine

Everything that manages an engine is one table of operations, reached three
ways with the same authorization: `/engine/<operation>` over HTTP, the engine's
MCP tools at `/mcp`, and `engine.call()` from a script. Among them:

- **Write as one change.** `write_files` writes several files in one
  transaction, one revision and one `init()` — [docs/ASSET_BATCH.md](docs/ASSET_BATCH.md).
- **Edit without resending.** `edit_file`, `read_file` with line ranges and grep,
  `search_files` — [docs/ASSET_EDIT.md](docs/ASSET_EDIT.md).
- **Check before deploying.** `check_script` bundles a script and runs its
  `init()` with registrations withheld — [docs/SCRIPT_CHECKS.md](docs/SCRIPT_CHECKS.md).
- **Test.** A script carries `*.test.ts` files; `run_tests` runs them in the
  script's own sandbox — [docs/SCRIPT_TESTS.md](docs/SCRIPT_TESTS.md).
- **Inspect.** `eval_script` evaluates a snippet in a deployed script's sandbox,
  rolling back database writes — [docs/SCRIPT_EVAL.md](docs/SCRIPT_EVAL.md).
- **Choose what production serves.** Every write records a revision;
  `deploy_script` pins one, `revert_script` and `diff_revisions` work across
  them — [docs/SCRIPT_REVISIONS.md](docs/SCRIPT_REVISIONS.md).
- **Debug by what it said.** `read_logs` filters by request, revision and route,
  and `/engine/script_logs/stream` tails live — [docs/SCRIPT_LOGS.md](docs/SCRIPT_LOGS.md).
- **Share through GitHub**, in both directions — [docs/GIT_SYNC.md](docs/GIT_SYNC.md).

Solution developers' documentation, type definitions and tooling are in
`aiwebengine-dev`; worked examples in `aiwebengine-examples`; an agent that
builds scripts in `aiwebengine-agent`.

## Running it

```bash
make postgres-local                # Postgres in a container
source .env-local && cargo run     # http://localhost:3000
```

`.env-local` is tracked and holds throwaway development values, with local
accounts enabled and `admin` as the bootstrap username. The containerised,
desktop and server deployments are described in [DEPLOYMENT.md](DEPLOYMENT.md);
every setting in `config.toml`.

## Documentation

- [docs/INDEX.md](docs/INDEX.md) — every document, by topic
- [docs/engine-contributors/ARCHITECTURE.md](docs/engine-contributors/ARCHITECTURE.md) — why the engine is shaped as it is
- [ROADMAP.md](ROADMAP.md) — what is open
- [CONTRIBUTING.md](CONTRIBUTING.md) — working on the engine

## License

AGPL-3.0; see [LICENSE](LICENSE).
