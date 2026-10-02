# What the simplification did

A short record. `docs/SIMPLIFICATION.md` is the argument and stays as it was
written, with each section marked as it was settled; this says what is true of
the tree now, where it differs from the plan, and what was left on purpose. The
working notes it replaces — phase-by-phase progress, the cutover procedure, the
lists of call sites — are in the git history of this file.

## Done

| §     | Thing                                       | State                                                          |
| ----- | ------------------------------------------- | -------------------------------------------------------------- |
| §1    | Script and assets are one tree              | Done — storage, MCP surface, `files` for scripts               |
| §2    | `.md` / `.txt` as string modules            | Done                                                           |
| §2    | Exposure by directory                       | Done and enforced                                              |
| §2.1  | Deny shape; asset-route authorization hook  | Done (`resource_access.rs`)                                    |
| §2/§7 | `asset_registry`, stream routing            | Folded into `route_index`; `stream_registry` keeps connections |
| §3    | One operation table, HTTP generated from it | Done (`engine_http.rs`)                                        |
| §4    | A collision on one host is refused          | Done                                                           |
| §4    | Slug and stable identity                    | Done — flat slugs, `rename_script`, `scripts.id`               |
| §5    | Prelude every global; one `registerRoute`   | Done                                                           |
| §6    | `database` trimmed                          | Done — ten methods                                             |
| §7    | `mcp_elicitation.rs`                        | Decided: stays (reasoning in §7, and the cost it names)        |
| §8    | "What narrows what"                         | Done — `docs/WHAT_NARROWS_WHAT.md`                             |

Every breaking change was made in the engine and in the four script
repositories (`aiwebengine-examples`, `aiwebengine-dev`, `aiwebengine-agent`,
`aiwebengine-private`) together, and the seven live scripts were moved to slug
names.

## Where the tree differs from the plan

These override `docs/SIMPLIFICATION.md` where the two disagree.

- **No path-prefix mounts.** Every script publishes at `/` of the hosts it is
  bound to, and binding a script to a host stays an API call
  (`set_script_hosts`). What changed is that a collision on one host stopped
  being silent. `public/**` is therefore not served automatically — a served
  file keeps an explicit file route.
- **Flat HTTP URLs.** `POST /engine/{operation}` with a JSON body, `GET` with
  query arguments for the read-only ones. No REST-shaped paths and no per-route
  hints; a file read is JSON, with the digest in the body rather than an `ETag`.
- **Collisions are derived, not recorded.** The older script (by
  `scripts.created_at`, read from the database) keeps a `(host, path, method)`;
  the rest are listed as `collisions` in `exposure_report`, computed from the
  stored registrations, so nothing needs clearing on `init()`.
- **Slugs are checked where a script is created, not where it is stored.** Every
  script that existed kept the identifier it had, so there was no
  rewrite-every-identifier migration; renaming is something an operator does to
  one script at a time. Physical table names did not need renaming either — the
  name is stored in `script_tables` and read back — only _new_ tables hash
  `scripts.id`.
- **`git_sync` records the directory.** A pulled script is found by the
  repository directory on its sync row, not by composing its name again, so a
  rename is followed and a push no longer inverts a URL.
- **`scripts.id` had been dropped** when `uri` became the primary key, so §4's
  "has existed since 2024 and nothing references it" had stopped being true; the
  migration put it back.

## Left on purpose, in the order they matter

1. **Status codes come from the error text.** `engine_http::status_for_error`
   classifies an operation's `{ "error": "..." }` by substring, because that is
   all an operation returns. It is one tested place, but it is a classifier over
   prose: a new message containing `not found` becomes a 404. The fix is a typed
   refusal on the operation's side (`AppError`, or an optional `status` in the
   result), touching the ~100 `json!({ "error": ... })` sites.
2. **No capability column on the operation table.** Each operation still
   authorizes itself inside its core function. Listing the capability in the
   entry would let the router refuse before running and would document it in
   OpenAPI.
3. **`engine.call`'s argument types are not generated** into
   `aiwebengine.d.ts` from the table.
4. **`uri` and `script` are both argument names.** The operations take `uri` for
   the script's name in some places and `script` in others, and the tooling's
   `--script-uri` flag keeps the older spelling. Unifying them is one table edit
   now, and a breaking change for every caller.
5. **Prose docs got a path rename, not a rewrite**, for the `POST` examples that
   send arguments in a query string (`secrets`, `limits`, `tasks`, `deploy`,
   `git/*`). They want reading, not searching.
6. **`mcp.ask` has no user.** See §7. Reconsider after the next MCP revision.
7. **Not verified in a browser:** the editor's and admin's own flows after the
   operations and slug changes, and `git-sync` run against a real repository
   (its paths and bindings are covered by tests).

## Things to be careful of

- **`cargo nextest`, not `cargo test`.** `make test-simple` does not pass and is
  not a gate. See CLAUDE.md for why.
- **Verify a migration against a clone of real data**, not only the test
  template, which is empty: `CREATE DATABASE x TEMPLATE aiwebengine`, run it,
  drop it. The tree merge and the identity migration were checked that way. A
  test template is fingerprinted by migration _names_, so editing a migration
  that has already run needs `DROP DATABASE aiwebengine_test_template`.
- **Registrations are in memory only** (the metadata cache), so extending
  `RouteMetadata` needs no migration.
- **Anything long-running locally belongs under `caffeinate -i`.** A sleeping
  Mac freezes the engine and the Postgres VM mid-request and looks like a hang.
- **Renaming a script does not update what holds its old name outside the
  engine**: a bookmark, an MCP client's configured script, a binding made under
  the old rules.
