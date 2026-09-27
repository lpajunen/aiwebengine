# Where the simplification stands

Working notes for picking `docs/SIMPLIFICATION.md` back up after a break.
The plan document says what to do and why; this says how far it got, what is
true of the tree right now, and what the next person needs in their head
before touching the next thing.

Branch: `simplify-one-tree`, nine commits off `main`, +4.5k/−1.8k across 45
files. Not merged.

## What is done

| §     | Thing                                       | State                                         |
| ----- | ------------------------------------------- | --------------------------------------------- |
| §1    | Script and assets are one tree              | **Done** — storage and MCP surface            |
| §1    | `assetStorage`'s four methods               | Not started; waits for §5's prelude           |
| §2    | `.md` / `.txt` as string modules            | **Done**                                      |
| §2    | Exposure by directory                       | **Done and enforced**                         |
| §2.1  | Deny shape for per-resource authorization   | **Done** (`resource_access.rs`)               |
| §2.1  | Asset-route authorization hook              | **Done**                                      |
| §2/§7 | `asset_registry` → `route_index`            | **Done**; the module is deleted               |
| §2/§7 | Stream routing → `route_index`              | **Done**; `stream_registry` keeps connections |
| §3    | One operation table, HTTP generated from it | Not started                                   |
| §4    | Name and mount                              | Not started                                   |
| §5    | Prelude every global                        | Not started                                   |
| §6    | Trim `database` to ~10 methods              | Not started                                   |

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

## Next, in the order I would do it

### 1. One `registerRoute(path, spec)` — decided, not started

The three JS calls collapse into one whose target says what it is. This was
chosen deliberately over keeping three names, accepting that every `init()`
has to be rewritten.

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

Exactly one of `handler`, `stream`, `file`; anything else refused with a
clear message. Shared: `summary`, `description`, `tags`, `authorize`, and for
handlers `method`, `parameters`, `requestBody`.

The internals are already one record, so this is surface work — but broad:
~164 call sites across `tests/`, `scripts/test_scripts/` and `assets/`, plus
the live deployment's own scripts. Worth doing in one pass with the old three
names deleted, not aliased.

### 2. §3, the operation table

`engine_api.rs` is still ~12k lines holding a `_route` function and a `tool_`
function per operation over shared cores, plus utoipa annotations and a
tool-schema table. §1 removed the _duplication between scripts and assets_;
§3 removes the duplication between HTTP, MCP and OpenAPI. Doing it before §4
means the mount work only has one surface to update.

### 3. §4, name and mount

Note the ordering constraint the plan states: move physical table names to
`scripts.id` **in the same pass** as the rename, or one unrenameable
identifier is traded for another.

## Things to be careful of

- **`make test-simple` (`cargo test`) does not pass and is not a gate.** Use
  `cargo nextest`. See CLAUDE.md for why.
- **`desktop::tests::generated_config_loads_and_validates` fails on `main`
  too.** It is pre-existing and unrelated; every run here is "1 failed" for
  that reason alone.
- **The local `aiwebengine` database is stale** relative to the live engine —
  its `agent.ts` still has `app.js` and `ui.html` at the tree root, so those
  two registrations will be refused on next boot. The live deployment is
  clean (all 32 asset registrations name a `public/...` file, no script
  registers MCP resources).
- **Verify migrations against a clone of real data**, not just the test
  template, which is empty: `CREATE DATABASE x TEMPLATE aiwebengine`, point
  `APP_REPOSITORY__DATABASE_URL` at it, boot, then drop it. The tree-merge
  migration was checked that way and it is the only thing that would have
  caught a bad `INSERT ... SELECT`.
- **Registrations are in-memory only** (the metadata cache), so extending
  `RouteMetadata` needs no migration.
