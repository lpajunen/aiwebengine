# Writing a Script's Files as One Change

A script's modules are one unit of change. `POST /engine/assets/batch` writes
them that way: one request, one transaction, one `init()` at the end.

```bash
curl -X POST "https://your-engine/engine/assets/batch?script=myapp" \
     -H "Content-Type: application/json" \
     -d '{
           "files": [
             { "name": "server/handlers.ts", "content_base64": "…" },
             { "name": "server/model.ts",    "content_base64": "…" }
           ]
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

## The request

| Field    | Default | Meaning                                                                               |
| -------- | ------- | ------------------------------------------------------------------------------------- |
| `script` | —       | URI of the script that owns these assets (required; may also be a query parameter)    |
| `files`  | —       | The files to write (required, at least one, at most 256)                              |
| `reinit` | `after` | `after` runs the script's `init()` once the batch has landed; `never` leaves it alone |

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
`WriteAssets` capability, ownership of the script, or administrator.

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
  "init": { "ran": true, "success": true, "durationMs": 34, "error": null }
}
```

Each file's digest is echoed back, so a caller can confirm what was stored
without a read-back round trip.

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

The `write_assets` tool takes the same arguments and answers with the same
body, including the `init` block:

```json
{
  "script": "myapp",
  "files": [{ "name": "server/handlers.ts", "content_base64": "…" }],
  "reinit": "after"
}
```
