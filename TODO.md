# some ideas to reduce amout of tokens used

1. The giant files are the main cost driver. repository.rs (7,140 lines), secure_globals.rs (6,234), js_engine.rs (3,897) — reading one of these whole is ~60–80k tokens, and sessions on this repo will touch them constantly.
   Best long-term (and good for humans too): split those files into submodules. repository.rs is described as the source of truth for scripts, assets, logs, secrets, and metadata — that's 4–5 natural modules.

Quieter commands = cheaper tool results. Build/test output gets read back into context every time. Worth encoding in CLAUDE.md:

- Prefer cargo nextest run \<filter\> over the full suite when iterating (you clearly already do this — your settings.local.json is full of one-off filtered runs).
- Add --status-level fail --final-status-level fail to nextest invocations so passing tests don't emit lines.
- Use cargo check -q / cargo clippy -q for iteration instead of full builds.
- make check runs format + lint + typecheck + tests with full output — fine before committing, wasteful mid-task.

2. Consolidate permissions into a shared .claude/settings.json. Your current allowlist is 35+ hyper-specific ad-hoc entries in settings.local.json (including one for a single historical commit message). A handful of prefix rules — Bash(SQLX\*OFFLINE=true cargo nextest run \*), Bash(cargo \_), Bash(git add \*), etc. — covers all of it, avoids permission-prompt round-trips, and can be checked into the repo. The /fewer-permission-prompts skill automates exactly this by scanning your transcripts.

3. Project skills for repeated workflows. You have none. Skills only load into context when invoked, so they're the right home for anything procedural that would otherwise bloat CLAUDE.md — e.g. a test skill (Postgres-up check + SQLX_OFFLINE + nextest filter syntax) or a run-local skill (the .env + config.toml + postgres-local dance). Keep CLAUDE.md as the always-loaded map, skills as the on-demand detail.

# developer-experience improvements (JS/TS framework API)

Context: aiwebengine competes with Firebase/Supabase/Workers (a multi-tenant BaaS/PaaS), not Node/Deno/Express. The architecture (capability sandbox, built-in auth/GraphQL/MCP/scheduler, zero-build hot-reload, multi-instance leases) is a genuine strength. The weaknesses are almost all at the **JS↔Rust ergonomic boundary**. Priority order below.

1. **Kill the JSON-string boundary (biggest daily friction).** Nearly every API in aiwebengine.d.ts takes/returns JSON _strings_, forcing manual `JSON.stringify` in / `JSON.parse` out on every call, and mixes success values with error strings (`{error: string}`, or `"Error: ..."` — e.g. `personalStorage.getItem` returns "value or error message" indistinguishably). Return/accept real objects (rquickjs supports this) and `throw` real `Error`s instead of encoding errors in return values. Fixes ergonomics + error handling + unlocks generics in one move.

2. **Async support.** `fetch`, DB, and MCP calls are all synchronous/blocking — a script making 3 HTTP calls does them serially with no way to parallelize. No `setTimeout`/`setInterval`/event loop. Make I/O calls return Promises and add an event loop so `async/await` and `Promise.all` work. Matches every developer's baseline mental model.

3. **Typed DB access + real migrations.** The `database` API is imperative and low-level: runtime `createTable`/`addTextColumn`/`addUniqueIndex` as side effects in `init()`, hand-rolled upserts, a mini filter language (`$gt`/`$lte`) but no joins, no aggregations, no raw-SQL escape hatch. Add `database.query<User>(...)` generics, a declarative schema/migration mechanism, and a raw-SQL escape hatch. Removes the biggest structural weakness.

4. **Standards alignment + a std-lib.** Non-standard globals mislead: `fetch` looks like WHATWG fetch but isn't; `sharedStorage`/`personalStorage` look like Web Storage but aren't; `URL`/`crypto.subtle`/`TextEncoder`/`structuredClone` appear absent. No npm at all (only asset-backed CommonJS-ish imports, no dynamic `import()`). Align the shapes with web standards and ship a small vetted std-lib of modules to offset the missing npm ecosystem.

5. **Function-valued handlers + middleware.** Handlers are referenced by _string name_ (`registerRoute("/x", "myHandler", "GET")`) that must match a top-level function — no closures, no composition, no middleware chains. Allow passing functions and composing middleware (the Express `app.get("/x", mw, handler)` pattern). Unlocks auth guards, logging wrappers, etc.

6. **Local dev loop.** No single-script test/run loop, no source maps back to TS, limited cross-boundary stack traces. Add a CLI to run/test one script against a throwaway Postgres with fast feedback and real stack traces. "No build step" is only a win if the debug loop is also good.

7. **Consistent return contracts + docs/scaffolding.** Standardize on one result envelope across all APIs; document the script-internal / engine-internal / external access model with runnable examples; add a project scaffolding generator. Cuts the "what does this return again?" tax from today's mixed conventions.

Note: items 1 and 2 alone would jump the DX a full tier without touching the underlying architecture.
