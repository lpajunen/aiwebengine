# One Tree, One Name, One Mount

The engine grew by answering each question where it was asked. HTTP got an
endpoint, MCP got a tool, JavaScript got a global, and each of them was right
on the day it landed. What accumulated is three vocabularies for one set of
operations, two storage shapes for one kind of file, and an identifier that
looks like a URL while naming neither a host nor a path.

This document says what to collapse and why, in the order the pieces depend on
each other. Nothing here needs backwards compatibility: scripts and data can be
migrated, so the only question is whether the end state is simpler than the one
it replaces.

`src/**/*.rs` is 105,628 lines, about 81,000 of them outside `#[cfg(test)]`.
The changes below remove an estimated 15–20k of that, and — more to the point —
remove three places where the same fact is written down.

## 1. A script and its assets are one tree

`scripts.code` is a **column**. Assets are rows in a separate table keyed
`(script_uri, uri)`. Even the history carries the split:
`script_revisions.root_sha256` sits beside `script_revision_files` as a
distinct thing rather than as one entry in the manifest.

That one schema decision forces two of everything above it:

| concept    | script half                                                                                                       | asset half                                                                                               |
| ---------- | ----------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------- |
| MCP        | `read_file`, `write_file`, `edit_file`, `create_file`, `delete_file`, `list_files`                                | `read_asset`, `write_asset`, `edit_asset`, `create_asset`, `delete_asset`, `list_assets`, `write_assets` |
| HTTP       | `/engine/read_script`, `/engine/upsert_script`, `/engine/edit_script`, `/engine/delete_script`, `/engine/scripts` | `GET`/`POST`/`PATCH`/`DELETE /engine/assets`, `/engine/assets/batch`                                     |
| JavaScript | —                                                                                                                 | `assetStorage.listAssets`, `fetchAsset`, `upsertAsset`, `deleteAsset`                                    |

And it leaks. `write_assets` has to carry "the script's root source and its
removals as well as its assets" as a special case, because the atomic unit is
the tree while the storage is not. `git_sync.rs` flattens the two into a
directory on the way out and re-splits them on the way in.

**The change.** The root source becomes the file named `main.*` in the tree.
`scripts` keeps identity, ownership, host bindings and init status; content
moves wholly into the files table. Thirteen tool names collapse to about six,
`write_assets` becomes an ordinary multi-file write, and `revisions.rs`,
`git_sync.rs`, `source_view.rs` and `module_loader.rs` each lose their
root-versus-asset branch.

This goes first because everything below it gets cheaper once it lands.

## 2. Exposure belongs to the tree, not to `init()`

Assets are read four ways, and all four already key off `(script_uri, path)`
and go through the same `repository::fetch_asset`. What differs between them is
not storage. It is **who may read the file**.

| case                 | reader                    | resolved by                       | authorization today                                          |
| -------------------- | ------------------------- | --------------------------------- | ------------------------------------------------------------ |
| module               | the linker, at build time | relative specifier → logical path | none — it is the script's own program                        |
| client file          | any browser               | `registerAssetRoute(path, name)`  | **none**; `try_serve_asset` checks the host and nothing else |
| MCP resource         | an MCP client             | `registerResource(uri, name)`     | bearer token, host-filtered                                  |
| script data (skills) | the script itself         | `assetStorage.fetchAsset(name)`   | script-internal                                              |

So the four cases are not an argument against merging the tree. They are an
argument for it. But they expose a real problem the merge should fix, because
today exposure is a **side effect of an `init()` call**:

- You cannot look at a file and know whether it is public. You have to read
  `init()` and mentally execute it.
- The default is private and the failure mode is public. `try_serve_asset`
  (`lib.rs:4344`) applies no authorization at all, so a mistyped
  `registerAssetRoute("/config", "credentials.json")` publishes a file to the
  world, with nothing in the write path, the revision manifest or a git diff to
  show it.
- Script data is defined by the **absence** of a registration. The most
  security-relevant category — system prompts, skill definitions, few-shot data
  — is the one with no positive marker. It is private because nobody happened
  to name it.

### Exposure from the directory

```
main.ts                 root module
lib/parse.ts            module, imported
skills/refund.md        script data, private
public/app.js           served, world-readable
resources/schema.json   MCP resource
```

Anything not under `public/` or `resources/` is private and reachable only to
the linker and to the script itself.

**Why a convention and not an `exposure` column.** `git_sync.rs` already made
this argument and refused a manifest: the directory structure already _is_ the
mapping. A column would be invisible in a repository, unmappable in both
directions, and would reintroduce exactly the manifest that reasoning rejected.
A directory is visible in `ls`, in a pull request, and in a revision manifest,
and it survives the round trip for free.

Registration does not disappear. It stops carrying the security decision and
carries only what is cosmetic: an HTTP path that should not mirror the file
path, OpenAPI `summary`/`tags`, an MCP resource's `name`/`description`/
`mimeType`. Publishing a file then means **moving** it, which is a reviewable
act.

### 2.1 Who may read this, when the engine answers without a handler

Exposure (§2) says whether a file is reachable at all. It cannot say _which
person_ may reach it, and that is a separate question with a separate answer.

A capability names a verb the engine understands — `ReadScriptData`,
`UseNetwork`, `ManageStreams`. "This person may subscribe to
`/orders/1234/events`" is a fact about the **script's own data model**: whose
order that is. The engine cannot know it. `script_owners` answers who may edit
a script, not who may read a row. So per-resource authorization must be script
code. It is not a mechanism competing with capabilities; it is the layer
capabilities structurally cannot reach.

Three surfaces answer this question three different ways, and one of them does
not answer it at all:

| surface     | authorization hook                                                               | personal content                         |
| ----------- | -------------------------------------------------------------------------------- | ---------------------------------------- |
| route       | the handler **is** the hook — it runs under the requesting user's context        | yes, reads `context.auth`                |
| stream      | the customization callback — the engine opens the connection, so no handler runs | yes, filter criteria from `context.auth` |
| asset route | **none**                                                                         | **impossible**                           |

`try_serve_asset` (`lib.rs:4344`) checks the host and serves the bytes. There
is no way to say "signed-in users only", let alone "the person this file
belongs to". And `asset_registry::get_asset_registration` is an exact-match
`HashMap` lookup — no `:param`, no wildcard — so `/files/:id` is not
expressible either. Folding it into `route_index` (§2) fixes the second half
of that; the first half needs the hook.

So the stream callback is not the odd one out. It is the only surface that gets
this right, and the asset route is the one with the hole.

**The principle.** Where the engine serves data _without running a handler_ —
streams, asset routes — the script supplies an authorization function that
decides, and the engine moves the bytes. Where a handler already runs, the
handler is the hook.

One shape, three surfaces, and it explains why routes need nothing extra. It
also preserves what makes an asset route worth having: **the decision is
JavaScript, the data path is not.** An asset behind an authorization callback
is still streamed directly rather than marshalled through JavaScript as a
string, which matters while `FetchResponse.body` is a `String`.

A two-valued `access: "public" | "authenticated"` flag was considered and is
not enough — being authenticated is frequently not the question. Asset routes
need the same callback shape streams have.

### What is wrong with the callback today

Three fixable defects, none of them a reason to remove it:

- **It cannot refuse cleanly.** It returns a `HashMap<String, String>` of filter
  criteria, so the only way to deny is to **throw** — which lands in
  `build_stream_error_response` as an **HTTP 500** with the throw message in the
  body. A denied subscription should be 401 or 403; a 500 tells the client to
  retry and leaks the message. It needs a real deny: a `{ deny: 403 }` return,
  or `false`, mapped to a status.
- **Its authorization role is undocumented.** `aiwebengine.d.ts` calls it
  "Optional name of a function that returns connection filter criteria", and
  `tests/stream_customization_authority.rs` reinforces the filter framing.
  Nobody writing a stream will discover that throwing is how you deny.
- **Deny-by-omission is the default.** `registerStreamRoute("/events/x")` with no
  callback means anyone who can reach the host gets the stream — the same
  open-by-default shape as assets.

### Per case

**Modules** — unchanged. This is the case the merge is built for.

**Client files** — `public/**`, served under the script's mount (§4), and
**genuinely public**: no check, no script execution. Stylesheets, bundles,
logos. Anything that needs a decision about _who_ is asking is not a `public/`
file; it is an authorized asset route (§2.1).

**MCP resources** — `resources/**`. The host filtering and the "read at
`resources/read` rather than copy at registration" behaviour (`mcp.rs:290`) are
right and do not change. What changes is that a resource is discoverable by
looking at the tree, and that registering one can no longer reach a file the
author did not mean to expose.

**Script data** — the decision here is not about exposure, it is about **build
time versus request time**:

- `assetStorage.fetchAsset(name)` is a database read on _every call_, returning
  a string through the JSON-in-a-string idiom (§5).
- `import skills from "./skills.json"` resolves at link time, is cached in the
  prepared program, is invalidated on write, is part of the revision's pinned
  content, and is visible to `tsc`.

For static content the import is strictly better, and people reach for
`fetchAsset` only because it is the one that is obviously _for_ assets.
`fetchAsset` should mean "content that changes without a redeploy", and nothing
else.

`module_loader.rs:549` already handles `.json` this way.
**Add `.md` and `.txt` as string modules** — about ten lines beside
`transform_json_module`, plus two entries in `is_supported_module_asset`
(`module_loader.rs:1119`) — and the whole skills case becomes build-time, with
no per-request cost and correct revision pinning:

```ts
import refundPolicy from "./skills/refund.md";
```

That is the highest-value small change in this area.

### Consequences

`assetStorage`'s four methods fold into the unified file API from §1.
`asset_registry.rs` folds into `route_index`: a `public/` file is a route, and
there is no reason for it to have a second global registry with a second
invalidation path.

## 3. Three hand-written descriptions of one operation set

`engine_api.rs` is 11,957 lines holding **57 `_route` functions** and
**52 `tool_` functions** over the same shared cores. `tool_read_file` and
`read_script_route` both call `read_script_authorized`, and each then does its
own argument parsing, its own error mapping and its own response shape — which
is why one answers `size` and the other answers `bytes`. On top of that sit
**57 `#[utoipa::path]` annotations** and a 1,650-line tool-schema table
describing the same operations a third and fourth time, and `aiwebengine.d.ts`
describing them a fifth.

The right mechanism already exists. `engine.call(name, args)` made the tool
table canonical and JavaScript a consumer of it. Finish the inversion:

- one operation table: `(name, json_schema, capability, fn(&Value, &UserContext) -> Value)`
- `/mcp` `tools/call` reads it — it already does
- HTTP is **generated** from it, with a uniform envelope
- `/engine/openapi.json` is generated from the same schemas; all 57 utoipa
  annotations go
- `engine.call` reads it — it already does

This deletes the route-handler half of `engine_api.rs`, an estimated 4–5k
lines, and the HTTP/MCP naming divergence (`upsert_script` versus `write_file`)
disappears as a side effect rather than as a renaming exercise.

**The cost, stated honestly.** Hand-tuned HTTP semantics are lost.
`read_script_route` returns `content-type: application/javascript` with the
digest in an `ETag`, and distinguishes 403 from 404. A generated envelope gives
`200 {"error": ...}` unless the table carries a status. Add one optional
`http:` hint per entry for the handful that need it, rather than keeping two
code paths for all of them.

## 4. The script URI is two things and is neither

### What the prefix does today: nothing

The host in a script URI is parsed by nothing. `hosts.rs` never reads it;
`script_hosts` decides where a script publishes and `route_index` keys on
`(host, pattern, method)` with the host taken from that table. There is no
validation that a script URI is a URL at all — `upsert_script_authorized`
accepts any non-empty string.

Exactly one part is load-bearing: `module_loader::root_module_path` takes the
last segment after `/` as the root module's filename, and its extension drives
`transpiler::needs_transpilation`.

So `https://example.com/shop.ts` means "a script called shop whose root module
is TypeScript", and the first nineteen characters are a fiction that
`git_sync.rs` already had to work around — its own reasoning notes that a URI
cannot travel between engines because one composes `https://her-host/shop/main.ts`
while another has a different host entirely.

### The conflation

Two independent things wear one string:

- **Identity** — what names this script in `script_owners`, `script_revisions`,
  `script_secrets`, `script_storage`, `script_tables`, `script_tasks`,
  `script_limits`, `script_deployments`, `script_git_sync` and `assets`. Must be
  unique, stable, opaque, and **not** host-scoped, because none of those are.
- **Location** — where its routes and files appear on the web. That is
  `(host, path_prefix)`, and it is many-to-many with identity, since
  `script_hosts` already lets one script publish on several hosts or on `*`.

The URL shape looks like it answers both. It answers neither: the host is
ignored, and there is no path prefix at all.

### A name and a mount

**Identity becomes a plain slug** — `shop`, or `acme/shop` if the namespace
needs structure. No scheme, no host, no file extension. The extension can go
because of §1: once the root source is `main.ts` in the tree, the tree carries
its own extension and the identifier does not have to. Merging the tree is what
frees the identifier from having to be a filename.

**Location becomes an explicit mount**, replacing `script_hosts`:

```sql
script_mounts (script_uri, host, path_prefix)   -- host '*' = every configured host
```

This is what does not exist today, and it is what public assets need:

- `public/**` serves at `{path_prefix}/...` automatically. A script mounted at
  `(shop.example.com, /)` serves `public/app.js` at
  `https://shop.example.com/app.js`. No registration call, no missing path.
- `registerRoute("/things/:id")` becomes **relative to the mount**. Two scripts
  can both declare `/things` on one host, one at `/shop` and one at `/admin`.
- Multi-host falls out directly: `(shop.example.com, /)` and
  `(admin.example.com, /)` are distinct keys, both serving `/settings`. That is
  already how `route_index` is keyed. What changes is that the mount makes the
  claim **declarative and refusable** instead of emergent from whichever
  `init()` happened to run last.

### This fixes a silent collision

`route_index.rs:190` inserts into a `HashMap` keyed `(host, pattern, method)`.
Two scripts claiming `/app` on one host: **the last to initialise silently
wins**, and which one that is depends on startup order. No error, no log, no
way to notice.

With mounts the collision moves up to mount registration, where it is a unique
constraint on `(host, path_prefix)` and is refused at write time with a clear
message. One script may hold `/` per host — the site-root script — and
everything else takes a prefix; longest-prefix-wins for resolution, which is
the specificity rule `find_route_handler` already applies to patterns.

### The identifier cannot currently be renamed

`db_schema_utils::generate_physical_table_name` (`db_schema_utils.rs:364`)
builds Postgres table names as `script_{sha256(uri)[..8]}_{logical}`. Renaming
a script URI therefore orphans every table it owns, silently.

That is a natural key doing surrogate-key duty. `scripts.id UUID PRIMARY KEY`
has existed since the original 2024 migration and is referenced by
**nothing** — every foreign key in the schema is on `scripts(uri)`.

So the rename migration is the moment to hash `scripts.id` for physical table
names and to point the foreign keys at `id`. After that the slug is a _mutable
name over a stable identity_, and renaming a script costs one `UPDATE`. Without
it, one unrenameable identifier is traded for another.

### Blast radius

There are 4,089 references to `script_uri` in `src`, but nearly all pass the
string through. Five places parse it:

| site                                                                        | effect                                                                                                             |
| --------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------ |
| `module_loader::root_module_path`                                           | dissolved by §1                                                                                                    |
| `git_sync.rs` (`compose_script_uri`, `resolve_uri_base`)                    | **shrinks**; most of the composition rule disappears with no host to invent. Costs a third `MAPPING_VERSION` bump. |
| `db_schema_utils::generate_physical_table_name`                             | fixed as above                                                                                                     |
| `lib.rs:4407`, `starts_with("engine://")`                                   | becomes a reserved slug                                                                                            |
| `engine_api.rs:9236`, `lib.rs:3703`, hardcoded `"https://example.com/core"` | becomes a reserved slug, more honestly than it reads now                                                           |

## 5. The JavaScript API has two idioms

Prelude-wrapped globals return real objects and throw real `Error`s:
`scriptTasks`, `fetch`, `sandbox`, `engine`, `mcp`, `crypto`, `scriptStorage`.

Raw host globals return **JSON inside a string** and `"Error: ..."` as a
_value_: `assetStorage`, `secretStorage`, `database`, `routeRegistry`,
`schedulerService`, `mcpRegistry`. That is 45
bare `: string` returns in `aiwebengine.d.ts` and 16 `"Error: ..."` formats in
`secure_globals.rs`.

So `database.query(...)` hands back a string to parse and then a field to check
that a caller may forget, while `scriptTasks.enqueue(...)` throws. Same engine,
same script, two conventions. Every global gets a prelude. Mechanical work, and
the change solution developers feel first.

## 6. `database` is the largest single API and most of it is redundant

27 methods:

- **seven** `addIntegerColumn` / `addBigintColumn` / `addFloatColumn` /
  `addTextColumn` / `addBooleanColumn` / `addTimestampColumn` /
  `addReferenceColumn` — `ensureTable(name, schema)` already expresses all of it
- **six** `beginTransaction` / `commitTransaction` / `rollbackTransaction` /
  `createSavepoint` / `rollbackToSavepoint` / `releaseSavepoint` — replace with
  one scoped `transaction(fn)` closing over the savepoint nesting that
  `Database::begin_transaction` already degrades to correctly. Note that the
  scoped helper does not exist today despite being referred to as though it
  does.
- `acquireLease` + `createLeaseTable` — `scriptTasks` lanes now do this
  properly, in the queue rather than in each caller

27 becomes about 10.

## 7. Features to drop — and one that stays

**Done: the two RPC surfaces nothing used.** A third way to reach a script and
a fourth registration kind, each with an invalidation path of its own, and
every user of either was a test fixture. Both are gone, along with the
capability that gated one of them and the queue's task-kind column that existed
only for the other. `scriptTasks` covers what the removed dispatch did and
covers it better — durable, laned, retried, with a state somebody can read.

**Not a drop: `stream_manager.rs` (690).** SSE is not in question — the streams
themselves are valuable and `stream_registry.rs` (1,408) is what implements
them. But `stream_manager.rs` is a tracking layer wrapped around that registry,
and it is dead: `StreamConnectionManager::new()` is constructed fresh at each
call site, used once and dropped, so its
`connection_metadata` and `connections_by_stream` maps are always empty. Two
things are broken as a result, both verified:

- **Connection limits never fire.** `check_connection_limits`
  (`stream_manager.rs:491`) compares the length of a freshly empty map against
  `max_total_connections` and `max_connections_per_stream`, so it evaluates
  `0 >= limit` on every connection and those settings do nothing.
- **Every disconnect leaks a connection.** `create_connection` discards the
  registry's connection id (`stream_manager.rs:236`) and returns
  `ActiveConnection`'s own separate `Uuid::new_v4()`. `lib.rs:1007` removes by
  that id, which is never a registry key, so the removal matches nothing and
  the count does not drop. Nothing ages them out either:
  `StreamRegistry::cleanup_stale_connections` is called only from its own unit
  test (`stream_registry.rs:1355`), and `StreamConnectionManager::start()`,
  which would spawn the cleanup task, is never called at all. Each leaked
  connection holds a `broadcast::channel(1000)` until restart.

This stayed invisible because it shows up only as slow memory growth under long
uptime, and `tests/streaming.rs` constructs the manager the same way production
does — reproducing the bug rather than catching it.

So the change is to **delete `stream_manager.rs` and move its two
responsibilities into `stream_registry`**, where the state lives and they would
work: limits checked against the registry's real counts, and one connection id
— the registry's — handed to the SSE handler. A bug fix that removes 690 lines,
not a feature drop.

**Done.** `StreamRegistry::open_connection` now checks the limits and inserts
under one acquisition of the registry lock, and returns the id it stored;
`ConnectionGuard` closes by that id on `Drop`, which is the only event a client
disconnect produces — axum drops the response body and nothing is polled, which
is why the old cleanup in the message closure's error arm could never have run
for a client that simply went away. The ceilings are
`server.max_connections_per_stream` and `server.max_total_stream_connections`;
a refused connection answers 503 rather than 500. Covered by
`tests/stream_connection_lifecycle.rs`.

The per-stream JavaScript customization callback is **not** part of this and is
not a candidate for removal. See §2.1: it is the only surface that gets
per-resource authorization right, and capabilities cannot replace it.

**Worth questioning, not an obvious cut:** `mcp_elicitation.rs` (782 lines).
`mcp.ask` is a third way to end an execution beside `mcp.task` and a normal
return, and it works only for clients implementing elicitation. If `mcp.task`
plus a stream covers it, that is one fewer terminal path.

## 8. Not a drop: document the four narrowings as one

`sandbox.rs` (capabilities and hosts), `delegation.rs` (scopes),
`elevation.rs` (grades) and `script_limits.rs` (budgets) all build on
`UserContext::attenuated`. Each is individually well-argued. But they are four
vocabularies a solution developer must hold at once, across four documents. Not
a deletion — one "what narrows what" document, and ideally one shared name for
the operation.

## Order of work

1. **Merge script and asset into one tree** (§1). Everything else gets cheaper.
2. ~~**Drop the two unused RPC surfaces**~~ _(done, §7)._
3. **Exposure by directory** (§2), plus `.md`/`.txt` string modules.
4. **Name and mount** (§4), including the move to `scripts.id` for physical
   table names and foreign keys.
5. **Collapse routes and tools into one operation table** (§3); generate HTTP
   and OpenAPI from it.
6. **Prelude every global** (§5); trim `database` to about ten methods (§6).
7. ~~**Fold `stream_manager` into `stream_registry`**~~ _(done)_ — the
   connection leak and the never-firing limits — **and `asset_registry` into
   `route_index`** (§7, §2).

## Open questions

- **Relative route paths are a behaviour change for every existing script.** The
  migration can derive a mount from the current URI path segment
  (`https://example.com/shop.ts` → `/shop`), but then absolute
  `registerRoute("/api/x")` calls need rewriting to relative — or every mount is
  `/` and today's collisions are preserved. There is no free version; deriving
  the mount and rewriting the registrations in one pass is the honest option.
- **Renaming a file changes its exposure.** That is the cost of convention over
  declaration, and also what makes it visible. Mitigate by having a write that
  moves a file _into_ `public/` say so in the audit line.
- **Classifying existing assets** needs the live registries at migration time, so
  a script whose `init()` fails will have registered nothing. That needs a
  second pass over stored source, or an operator-visible report of what could
  not be classified.
- **A file that is both a module and an MCP resource** — a `.json` schema that is
  imported _and_ published — must pick a directory. Importing from `resources/`
  is fine, since the directory governs exposure rather than importability, but
  say so explicitly or someone will assume `resources/` is closed to the linker.
- **What shape should a deny take** (§2.1)? `{ deny: 403 }`, a `false` return, or a
  thrown typed error all work; what matters is that it is not a bare throw
  landing as a 500. Settle it once for streams and asset routes together, since
  they should share the mechanism.
- **A slug namespace is global per engine**, so two tenants both wanting `shop`
  collide. `acme/shop` handles it, but decide whether the slug has structure
  before the foreign keys move, because it is much cheaper now than after.
