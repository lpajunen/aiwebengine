# Where the simplification stands

Working notes for picking `docs/SIMPLIFICATION.md` back up after a break.
The plan document says what to do and why; this says how far it got, what is
true of the tree right now, and what the next person needs in their head
before touching the next thing.

The first stretch was done on `simplify-one-tree` (nine commits, +4.5k/−1.8k
across 45 files) and is **merged into `main`**.

## What is done

| §     | Thing                                       | State                                         |
| ----- | ------------------------------------------- | --------------------------------------------- |
| §1    | Script and assets are one tree              | **Done** — storage and MCP surface            |
| §1    | `assetStorage`'s four methods               | **Done** — `files` (1b)                       |
| §2    | `.md` / `.txt` as string modules            | **Done**                                      |
| §2    | Exposure by directory                       | **Done and enforced**                         |
| §2.1  | Deny shape for per-resource authorization   | **Done** (`resource_access.rs`)               |
| §2.1  | Asset-route authorization hook              | **Done**                                      |
| §2/§7 | `asset_registry` → `route_index`            | **Done**; the module is deleted               |
| §2/§7 | Stream routing → `route_index`              | **Done**; `stream_registry` keeps connections |
| §3    | One operation table, HTTP generated from it | Not started (Phase 2)                         |
| §4    | Collision refusal (no mounts)               | Not started (Phase 3)                         |
| §4    | Slug + stable identity                      | Not started (Phase 4)                         |
| §5    | Prelude every global                        | **Done** (Phase 1, unmerged)                  |
| §6    | Trim `database` to ~10 methods              | **Done** — ten                                |

## What is true of the tree now

Things a returning reader would otherwise have to rediscover.

**A script is one tree.** `scripts` holds identity, name, ownership and init
status — no content. The entrypoint is the `assets` row named
`main.{ts,js,tsx,jsx}`, resolved from the tree (`SourceView::root_path`) and
falling back, only for a script with no files, to the URI's extension.
`script_revisions.root_sha256` is gone; a revision's manifest is the whole of
it.

**Writing or deleting a `main.*` takes `WriteScripts` / `DeleteScripts` and
ownership**, wherever it is reached — the batch, the single write, the patch,
the delete. Merging the storage must not make `WriteAssets` a way to replace
a script's program. `assetStorage` refuses a root name outright, because it
could not reach one before the merge.

**A registration is one kind of thing.** `RouteMetadata` carries a
`RouteKind` — `Handler`, `File` or `Stream` — and all three `routeRegistry`
calls record into the same sink, land in the script's registrations, and are
indexed by `route_index`. Consequences worth knowing: patterns work for all
three; a registration the script stops making actually disappears on
re-`init()`; precedence is stated in `route_index::resolve` (file, then
stream, then handler).

**Exposure is the directory.** `public/` is served, `resources/` is an MCP
resource, everything else is reachable only to the linker and the script.
A registration naming a file outside its directory is **refused** — and
refusals are recorded per script and cleared on each `init()`, which is what
`GET /engine/exposure` reports.

**Public by default is correct, not a gap.** A stream's callback and a file
route's `authorize` are the hook a route's handler is; a handler that checks
nothing is public too. `docs/SIMPLIFICATION.md` §2.1 used to list this as a
defect and no longer does.

## Decisions taken since the plan was written

These override `docs/SIMPLIFICATION.md` where the two disagree.

- **No path-prefix mounts.** §4's `script_mounts` table (host plus path
  prefix) is not being built. Every script keeps publishing at `/` of
  the host(s) it is bound to, and **binding a script to a host stays an API
  call** — today's `/engine/script_hosts` / `set_script_hosts`. That is what
  lets two scripts register the same path: they are set to different hosts.
  What changes is that a collision _on one host_ stops being silent (Phase 3).
  Because there is no prefix, `public/**` is **not** served automatically —
  every script would claim `/app.js` at the same root. A served file keeps an
  explicit file route.
- **Flat HTTP URLs.** The generated HTTP surface is `POST /engine/{tool_name}`
  with the tool's arguments as the JSON body (`GET` with query arguments for
  read-only operations). No REST-shaped paths are kept through hints.
- **`registerRoute(path, spec)` lands with §5**, not before it. `routeRegistry`
  is one of the raw globals §5 wraps, and both changes rewrite the same
  `init()` call sites — doing them apart breaks every script twice.
- **§3 before §4.** §4 renames the `uri` argument of nearly every operation;
  after §3 that is one table rather than HTTP, MCP and OpenAPI separately.

## The scripts that have to move with the engine

Every breaking change below is paid for in four repositories, and nothing
else is in scope — not the live database's copies, which are refreshed by
pulling these after each cutover.

| repository                | what is in it                                        | register\* sites | `JSON.parse(` |
| ------------------------- | ---------------------------------------------------- | ---------------- | ------------- |
| `../aiwebengine-agent`    | the agent script (heavy `database`, `secretStorage`) | ~30              | ~23           |
| `../aiwebengine-dev`      | `admin`, `docs`, `editor` + deploy tooling           | ~43              | ~38           |
| `../aiwebengine-examples` | ~27 example scripts + the same deploy tooling        | ~82              | ~132          |
| `../aiwebengine-private`  | one private solution (heavy `schedulerService`)      | ~16              | ~32           |

Counts are greps and include each repo's vendored `types/aiwebengine.d.ts`;
they are for sizing, not a checklist. Three things in them are coupled to the
engine beyond the JS API:

- **Deploy tooling.** `aiwebengine-dev/scripts/` and
  `aiwebengine-examples/scripts/` are identical copies (`upload-script.js`,
  `deploy-assets.js`, `check-script.js`, `run-tests.js`, `revisions.js`,
  `git-sync.js`, `set-script-hosts.js`, …) calling about thirty `/engine/*`
  paths. Phase 2 breaks all of them. `aiwebengine-examples` is already the
  source: fix it there and `make sync-tooling` in `aiwebengine-dev`
  (`make check-tooling` fails on drift).
- **Browser UIs calling the engine.** `aiwebengine-dev`'s `admin` and `editor`
  call `/engine/*` from the page (`script_logs`, `assets`, `users`,
  `script_hosts`, …). Also Phase 2.
- **`aiwebengine.config.json`** composes script URIs from `uriOrigin` +
  directory (`https://example.com/editor`). Phase 4 replaces that with slugs.

**The cutover procedure, per breaking phase:**

1. On an engine branch, make the change and rewrite the fixtures in `tests/`
   and `scripts/test_scripts/`.
2. In each of the four repos, on a branch of the same name: `make fetch-types`
   (or copy `assets/aiwebengine.d.ts`) from the local engine, rewrite, and
   `make typecheck` until clean — the typecheck is what finds the call sites
   the greps missed.
3. Against a local engine running a **clone of real data**
   (`CREATE DATABASE x TEMPLATE aiwebengine`): pull each repo in, run
   `check_script` and `run_tests` over every script, and read
   `/engine/exposure` and the collision report for anything refused.
4. Merge the engine, deploy, then pull the four repos straight away. Scripts
   fail `init()` in the gap; with one operator that is acceptable, and it is
   shorter than any compatibility shim would be to write and remove.

## Next, in order

### Phase 0 — housekeeping

- Refresh the stale local database. The repository side is done
  (`aiwebengine-agent` `e6395e9` moved the two files under `public/`); what is
  left is the database, whose `agent.ts` is far older than the repository — a
  single `main.ts` beside root-level `app.js` / `ui.html` — and which also
  holds ~40 fixture scripts from before tests had databases of their own.
  Worth doing before Phase 1, because "a clone of real data" in the cutover
  procedure means this database.
  **Rebuilt 2026-09-30** from `aiwebengine_test_template` (an empty database
  cannot even compile the engine, since `sqlx::query!` checks against it) and
  loaded from the four repositories with a scratchpad copy of the tooling, so
  no repository's production token was touched. 30 of 32 scripts initialise;
  the two that do not are `github_mcp_issues`, which has no `init()`, and
  `auth_roles_demo` (below). What it turned up:
  - ~~**Executing a script writes it.**~~ **Fixed:**
    `execute_script_secure` no longer stores what it runs; tests that used it
    as their deploy step store the script first. `js_engine::execute_script_secure`
    calls `repository::upsert_script(uri, content)` on every execution, which
    before the tree merge rewrote a column and now writes an entry _file_,
    named from the URI's extension. So every boot rewrites every script's
    entrypoint; a script with no entrypoint gets an empty `main.js`; and a
    TypeScript script whose URI has no `.ts` gets its source written into a
    second entry, `main.js`.
  - ~~**Writing `main.ts` stores `main.js`.**~~ **Fixed:** a write that
    names the entrypoint (`upsert_root_authorized`, reached by every
    single-file write of a `main.*`) writes that file and removes any other
    entrypoint in the same transaction, so naming the file is how its
    language changes. A write that names none (`upsert_script`, a batch's
    `content`) keeps writing whichever entrypoint the tree has. The single-file write of an entry
    delegates to `upsert_script_authorized`, which names the file from the
    URI and drops the name the caller gave. Same root cause: the URI extension
    is still load-bearing on the write path, which §1 says it no longer is.
    The four TypeScript/JSX examples (`typescript`, `tsx`, `jsx`,
    `import-example`) are loaded with it.
  - **The tooling uploads every entry through `/engine/upsert_script`**, which
    carries no file name — the same bug from the client side. Goes away in
    Phase 2 with `upsert_script`, but the tooling should write the entry as
    the file it is.
  - **A capability refusal on `/engine/upsert_script` answers 500**, not 403.
    Phase 2's single error mapping fixes this class.
  - **`upload-script.js` collects `.git/` and ignores the repository's
    `.aiwebengineignore` when `--assets-dir` is the repository root** (it
    reads the ignore file from the tooling's own root), and it chunks batches
    by bytes but not by the engine's 256-file ceiling.
  - `aiwebengine-examples/auth_roles_demo` registers `/auth/demo`, which is a
    reserved prefix. A script bug, not an engine one.
  - ~~**The connection pool ran dry after about an hour.**~~ The development
    Mac was sleeping. `pmset -g log` shows Maintenance Sleep every ~15
    minutes with brief DarkWakes, and a sleep freezes the engine and the
    Postgres VM mid-request: the "hang" lasted as long as the machine slept,
    including past a 20-second curl timeout. Kept awake (`caffeinate -i`),
    the same runs answer in well under a second. What remains is a weaker
    question — whether the engine recovers by itself after the database
    vanishes under it for a while — which matters for a laptop and not for a
    server. Anything long-running locally belongs under `caffeinate -i`.
- ~~Fix or quarantine `desktop::tests::generated_config_loads_and_validates`~~
  — it passes now; a full `cargo nextest run --all-features` on `47fb7b0` is
  1705 passed, 0 failed.
- ~~In `docs/SIMPLIFICATION.md`: close the settled deny-shape open question and
  point §4 at the decisions above.~~

### Phase 1 — one JavaScript convention (§5, §6, rest of §1)

**Progress. Phase 1 is complete** on `simplify-js-convention`, in the engine
and in each of the four repositories — unpushed and undeployed. The engine
and the four repositories have to be merged and deployed together: every
script on the branch assumes the new globals and every script on `main`
assumes the old ones.

- **1a done** (engine `d5c7dea`). `registerRoute(path, spec)` with the three
  old names deleted, `routeRegistry` preluded over `__hostRouteRegistry`,
  sends answering `{ delivered, connections, failed }` with the filter as an
  object. Two choices the plan above left open: `authorize` is refused on a
  handler spec, since a handler decides for itself; and a reserved path
  throws rather than being refused, since no script can ever hold one. The
  four repositories were rewritten by codemod (`register-route.js` and
  `filter-object.js` in the session scratchpad — mechanical, then by hand
  for prose and for the three places that parsed the old sentences). Loaded
  into the local engine, the same 30 of 32 scripts initialise as before, with
  165 routes.
- `git_push files_the_script_does_not_own_survive` timed out once under the
  full suite (180s) and passes alone in 0.3s — very likely the same sleep.
- **1b done** (engine `9f282c0`). `files.list/read/write/delete` replaced
  `assetStorage`: a read is text or `null`, binary is asked for with
  `{ encoding: "base64" }`, failures throw, the listing is sorted and keyed
  by `path`. `files` is configurable and writable so a script's own
  top-level `const files` shadows it. The agent's skill code, the dev
  repository's docs script and editor, and the dev reference and assets
  guide moved with it. Verified locally with the Mac kept awake: the same 30
  of 32 scripts initialise, the docs script serves Markdown through
  `files.read`, and the agent's 86 and the private script's 313 in-engine
  tests pass.
- **1c done** (engine `3326e09`, `6cd770c`). `database` is preluded and ten
  methods: `ensureTable` describes a table (with a new `reference` column
  type standing in for `addReferenceColumn`), `query` takes an options
  object, `transaction(fn)` replaced the six transaction and savepoint calls,
  and the lease calls went with their repository code — a row read
  `forUpdate` in a transaction does the same, which is what virtual-world now
  does. Three things it surfaced: a handler returning an array or plain
  object as its body got `"[object Object]"` (now JSON); an async-semantics
  rollback test ran anonymously and passed without ever writing; and a
  failed commit or rollback stranded the transaction state on its pooled
  thread, so that thread could never start a transaction again and schema
  setup run on it failed silently — the cause of virtual-world's migration
  "not running" locally. All four repositories pass their in-engine tests
  (agent 86, private 313, virtual-world 339) with the Mac kept awake.
- **1d done** (engine `991d2a9`). `secretStorage`, `schedulerService` and
  `mcpRegistry` are preluded: writes return nothing and throw, registrations
  answer `{ ok, ... }` or `{ ok: false, reason }` outside `init()` and throw
  on misuse (arguments now validated before the phase is checked, as
  `registerRoute`'s are), and `mcpRegistry` takes `(name, spec)` with schemas
  and prompt arguments as objects. `removeSecret` throws on a refusal where it
  used to answer `false`. Locally the same 30 of 32 scripts initialise, the
  in-engine tests pass, and the 14 MCP tools the repositories register are
  all registered.
- **1e done** (engine `bc250ab`). `convert` and `McpClient` are preluded.
  `convert`'s four functions answer their result and throw, and
  `render_handlebars_template` takes its data as an object (JSON text still
  accepted). `McpClient` is a class — `new McpClient(url, secret)`,
  `listTools()`, `callTool(name, args)` — throwing on a JSON-RPC error with
  the server's `code` on the error; the descriptor-passing `constructor` /
  `_listTools` / `_callTool` statics are gone, and so are the wrapper classes
  `github` and `github_mcp_issues` carried. `fetch`'s response lost the
  `toString()` that yielded the old JSON envelope, so `JSON.parse(fetch(…))`
  no longer works; options have been an object since before this phase, and
  the dev reference and the editor's prompt stopped saying they must be a
  string. The `__writeLog` host returns nothing. The grep the plan set as
  1e's bar is clean: what remains of `"Error: "` in `secure_globals.rs` is
  comments, and the one test matching it reads a thrown error's text.
  Verified locally under `caffeinate -i`: the same 30 of 32 scripts
  initialise (`auth_roles_demo` registers a reserved path, `github_mcp_issues`
  has no `init()`), `/docs` and `/blog` render through `convert`, and the
  agent's 86, the private script's 313 and virtual-world's 339 in-engine
  tests pass. Repository commits: examples `c2608c4`, dev `3aa283e`, agent
  `3daa009`; private needed nothing.

Breaking for every script; do it as one release with one cutover. One commit
per global, each with its `aiwebengine.d.ts` change:

- **1a `routeRegistry`.** Prelude, and the one call:

  ```js
  routeRegistry.registerRoute("/things/:id", {
    handler: "getThing",
    method: "GET",
  });
  routeRegistry.registerRoute("/things/:id/events", {
    stream: true,
    authorize: "mayWatch",
  });
  routeRegistry.registerRoute("/thing.css", { file: "public/thing.css" });
  ```

  Exactly one of `handler`, `stream`, `file`. Shared: `summary`,
  `description`, `tags`, `authorize`; handlers add `method`, `parameters`,
  `requestBody`. `registerStreamRoute` and `registerAssetRoute` are deleted,
  not aliased. Misuse (no target, two targets, unknown key) **throws**; a
  _refusal_ — a file outside `public/`, and from Phase 3 a path another
  script holds on this host — **returns** `{ ok: false, reason }`, so one bad
  registration does not cost the script its others. That keeps what exposure
  enforcement already does and states it as the rule.

- **1b `assetStorage` → a file API on the script's own tree.** Same four
  operations, preluded, returning values and throwing errors; keeps refusing
  `main.*`. Name to decide at the time (`files` is the obvious one). Document
  it as "content that changes without a redeploy"; anything static becomes an
  import.
- **1c `database`.** Prelude first (real objects, thrown errors), then trim:
  the seven `add*Column` go (`ensureTable` covers them), the six transaction
  and savepoint calls become one `transaction(fn)` over the nesting
  `Database::begin_transaction` already does, `acquireLease` /
  `createLeaseTable` go (`scriptTasks` lanes). ~28 → ~10. The agent and the
  examples' `dbtest` are the heaviest users; the lease users move to a laned
  `scriptTasks` queue.
- **1d** `secretStorage`, `schedulerService`, `mcpRegistry` preludes.
- **1e** Delete the `"Error: ..."` value formats from `secure_globals.rs` and
  the bare `: string` returns from `aiwebengine.d.ts`; a grep for either
  should come back empty.
- **Scripts:** every `JSON.parse(<global>.…)` and `startsWith("Error")` check
  goes, every registration is rewritten. Mechanical but wide — examples is
  most of it.

### Phase 2 — one operation table (§3)

1. **Inventory** the 58 `_route` functions against the 49 tools. Each route
   either has a tool, gets one, or is one of the few that stay hand-written:
   `/engine/script_updates` and `/engine/script_logs/stream` (SSE),
   `/engine/installed`, `/engine/openapi.json`, `/engine/types/...`,
   `/engine/engine.css`, `/favicon.ico`.
2. **Grow the entry** to name, description, schema, capability,
   `http: Option<HttpHint>` and fn. `HttpHint` is for the handful whose answer is
   not JSON — the raw file read's content type and `ETag` — not for paths.
3. **Generate HTTP** as `POST /engine/{name}`. `AppError` maps to 400/403/404
   in one place, which recovers most of what the hand-written routes did.
   `management_hosts` enforcement moves to the one generated router.
4. **Generate `/engine/openapi.json`** from the schemas; delete the 58
   `#[utoipa::path]` annotations and the dependency if nothing else needs it.
5. **Delete the `_route` functions.** Tests call operations through one helper
   by name. Expected: −4–5k lines of `engine_api.rs`.
6. Optionally generate the `engine.call` argument types in `aiwebengine.d.ts`
   from the table — the fifth description of the same operations.

**Scripts:** the deploy tooling in `aiwebengine-dev` / `aiwebengine-examples`
and the `admin` / `editor` UIs, which are the only callers of `/engine/*` in
scope. Scripts themselves call `engine.call`, which does not change.

### Phase 3 — a collision on one host is refused (§4, the part that survives)

- A script's hosts are still set by API: `/engine/script_hosts` /
  `set_script_hosts` (from Phase 2, `POST /engine/set_script_hosts`). Two
  scripts on different hosts may register the same path, as today.
- Two scripts claiming the same `(host, path, method)` — `*` counting as
  every host — is no longer settled by whichever `init()` ran last
  (`route_index.rs:190`). The holder keeps it; the second registration gets
  `{ ok: false, reason: "… held by <script>" }`, is recorded per script and
  cleared on `init()` exactly like exposure refusals, and shows in the same
  report (widen `GET /engine/exposure` into a general "refused registrations"
  report rather than adding a second one).
- Startup order must be deterministic for "the holder" to mean anything:
  execute startup scripts sorted by name, so a restart does not hand a path to
  the other script.
- `set_script_hosts` that would create a collision on the target host is
  refused at the call with the conflicting paths listed — the "move this
  script to that host" operation is where the operator learns it.
- **Scripts:** none, unless the report shows a collision that exists today.

### Phase 4 — name and stable identity (§4)

- **4a Stable identity first.** Physical table names hash `scripts.id`
  rather than the URI (the migration renames every table in `script_tables`),
  and every `script_uri` column gets a foreign key with `ON UPDATE CASCADE` —
  adding one where there is none today (secrets, storage, tasks, limits,
  logs, …). A rename is then one `UPDATE` and the 4,089 references to
  `script_uri` stay as they are. In-memory caches keyed by URI are dropped on
  rename via `notifications.rs`.
- **4b Slugs.** `https://example.com/editor` → `editor`. `engine://native` and
  `https://example.com/core` become reserved slugs. Decide whether a slug may
  be `acme/shop` before this migration, not after. `git_sync`'s URI
  composition mostly disappears (third `MAPPING_VERSION` bump).
- **Scripts:** `aiwebengine.config.json` in `aiwebengine-dev` /
  `aiwebengine-examples` drops `uriOrigin` for slugs, the tooling's URI
  composition in `scripts/lib/repo-config.js` goes with it, and anything that
  hard-codes a `https://example.com/...` URI (grep) is rewritten.

### Phase 5 — wrap-up

- The "what narrows what" document (§8).
- Decide `mcp_elicitation.rs` (§7).
- CLAUDE.md, README and `docs/` brought in line; this file collapses to a
  short record of what was done.
- Each of the four repos' README / CLAUDE.md updated for the API it now uses.

## Things to be careful of

- **`make test-simple` (`cargo test`) does not pass and is not a gate.** Use
  `cargo nextest`. See CLAUDE.md for why.
- **The local `aiwebengine` database is stale** relative to the live engine —
  see Phase 0. Its `agent.ts` still has `app.js` and `ui.html` at the tree
  root, so those two registrations will be refused on next boot. The live deployment is
  clean (all 32 asset registrations name a `public/...` file, no script
  registers MCP resources).
- **Verify migrations against a clone of real data**, not just the test
  template, which is empty: `CREATE DATABASE x TEMPLATE aiwebengine`, point
  `APP_REPOSITORY__DATABASE_URL` at it, boot, then drop it. The tree-merge
  migration was checked that way and it is the only thing that would have
  caught a bad `INSERT ... SELECT`.
- **Registrations are in-memory only** (the metadata cache), so extending
  `RouteMetadata` needs no migration.
