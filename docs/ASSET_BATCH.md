# Writing a Script's Files as One Change

A script's files are one unit of change. `POST /engine/assets/batch` writes
them that way: one request, one transaction, one revision, one `init()` at the
end — the modules, the root source that imports them, and whatever the change
removes.

```bash
curl -X POST "https://your-engine/engine/assets/batch?script=myapp" \
     -H "Content-Type: application/json" \
     -d '{
           "content": "import { handle } from \"./server/handlers.ts\"; …",
           "files": [
             { "name": "server/handlers.ts", "content_base64": "…" },
             { "name": "server/model.ts",    "content_base64": "…" }
           ],
           "remove": ["server/legacy.ts"]
         }'
```

## Why not a loop over `/engine/assets`

Every single-asset write is a deploy in miniature. It invalidates the script's
prepared program, and it notifies the rest of the cluster, where each instance
answers by reinitializing the script from scratch. Pushing a twelve-file change
one request at a time therefore makes every instance reinitialize twelve times,
eleven of them from a tree that is still being uploaded — with routes
registered, and requests served, from each half-written state along the way.

A batch collapses that into a single announcement made once the whole set has
landed.

## The whole of a change

A batch could carry a script's assets and not the file that imports them, so a
change touching the root and its modules was two writes — two revisions, two
cluster notifications, two `init()` runs, and a window in which the deployment
is half-changed. `/engine/check` would check exactly that change in one
request, which is the shape this now matches: the request that _describes_ a
change is the request that _applies_ it, `content` for the root and `files` for
the modules in both.

A deletion is the same argument. Removing a module is as much a change as
rewriting one, and belongs in the same request as the change to the code that
stopped importing it — so `remove` names paths that must not survive this
write. Naming a file the script no longer has is not an error; it is a change
that has already happened.

## The request

| Field     | Default | Meaning                                                                                    |
| --------- | ------- | ------------------------------------------------------------------------------------------ |
| `script`  | —       | URI of the script whose files these are (required; may also be a query parameter)          |
| `files`   | —       | The files to write (at most 256; required unless `content` or `remove` carries the change) |
| `content` | —       | The script's **root source**, as text, as this change leaves it. Omit to leave it alone.   |
| `remove`  | `[]`    | Asset paths this change deletes                                                            |
| `reinit`  | `after` | `after` runs the script's `init()` once the batch has landed; `never` leaves it alone      |

`content` at the top level is the root source and is plain text — it is the
program the engine executes. It is not the `content` accepted inside a `files`
entry, which is an alias for `content_base64`.

Each entry in `files`:

| Field            | Default                     | Meaning                                             |
| ---------------- | --------------------------- | --------------------------------------------------- |
| `name`           | —                           | Path of the asset within the script (required)      |
| `content_base64` | —                           | Base64-encoded content (required, max 10MB)         |
| `mimetype`       | inferred from the extension | MIME type stored with the asset                     |
| `sha256`         | —                           | Digest the caller expects the decoded bytes to have |

`asset` and `content` are accepted as aliases for `name` and `content_base64`,
so a caller written against the single-asset route does not have to rename its
fields. One batch may carry 10MB of content in total.

A batch carries the content it writes. To change part of a file the engine
already has, without sending the file back, see
[Editing a Script's Files Without Resending Them](ASSET_EDIT.md).

Writing assets takes the same rights as writing one at a time: the
`WriteAssets` capability, ownership of the script, or administrator. A batch
carrying `content` also writes a script, so it takes what that takes —
`WriteScripts`, with the same ownership rule. Neither capability is a way to
reach the other: a caller holding `WriteAssets` alone is refused the root, and
the whole request writes nothing.

## All or nothing

Every file is decoded and checked — path, size, and digest — before any of them
is stored. A batch with one bad entry writes nothing and answers `400` naming
the file at fault. This is the property worth having: a rejected push leaves
the deployed tree exactly as it was, rather than applying the files that
happened to come before the broken one.

Supplying `sha256` for a file makes that guarantee cover the transfer too. The
digest is checked before the write, so a corrupted upload is refused rather
than deployed.

## The answer

```json
{
  "script": "myapp",
  "results": [
    {
      "name": "server/handlers.ts",
      "sha256": "9f2…",
      "bytes": 1841,
      "status": "updated"
    },
    {
      "name": "server/model.ts",
      "sha256": "c07…",
      "bytes": 622,
      "status": "unchanged"
    }
  ],
  "written": 1,
  "root": "updated",
  "deleted": 1,
  "revision": 41,
  "init": { "ran": true, "success": true, "durationMs": 34, "error": null }
}
```

Each file's digest is echoed back, so a caller can confirm what was stored
without a read-back round trip.

`root` says what happened to the root source — `updated`, or `unchanged` when
it already held what was sent — and is present only when the request carried
`content`. `deleted` counts the removals that removed something, and is present
only when there were any. So a caller written against the asset-only batch sees
exactly the body it always saw.

`revision` is the one revision recorded for the whole change — the number a
`revert` goes back to — and is `null` when nothing reached storage.

`status` is `created`, `updated`, or `unchanged`. A file whose stored content
and type already match what was sent is reported as `unchanged` and not
rewritten — rewriting it would invalidate the script's prepared program for no
change. `written` counts the files that actually reached the database.

`init` reports the run that followed, rather than leaving it to happen in the
background: `success` is whether `init()` completed, `error` carries its message
when it did not. It reports `{"ran": false, "reason": …}` when `reinit` was
`never`, and when nothing changed — a batch that wrote nothing has no
registrations to rebuild.

Note that `reinit: "never"` is local: other instances still learn of the change
and reinitialize when they do. It is for a caller pushing one part of a larger
change that is not coherent yet, not a way to keep a cluster quiet.

## Over MCP

The `write_assets` tool takes the same arguments — including `content` and
`remove` — and answers with the same
body, including the `init` block:

```json
{
  "script": "myapp",
  "files": [{ "name": "server/handlers.ts", "content_base64": "…" }],
  "reinit": "after"
}
```
