# Editing a Script's Files Without Resending Them

A change to three lines of a module is three lines. `PATCH /engine/assets`
sends that much: the text to find and the text to put in its place, against an
asset the engine already has.

```bash
curl -X PATCH "https://your-engine/engine/assets?script=myapp&asset=server/move-player.ts" \
     -H "Content-Type: application/json" \
     -d '{
           "edits": [
             { "old_string": "const SPEED = 4;", "new_string": "const SPEED = 6;" }
           ],
           "base_sha256": "9f2…"
         }'
```

`POST /engine/assets` and `/engine/assets/batch` remain the way to write a file
whose content the caller has in hand. A patch is for the other case — where the
caller has the file only because it read it, and resending it is transfer spent
to say nothing changed.

## What replaces "here is the file"

A full write carries its own answer to "what is being stored": the bytes are in
the request. A patch does not, so it has to establish the same thing two other
ways.

**Which version is being edited.** `base_sha256` is the digest the caller
believes the asset currently has — the one every read reports. It is checked
before anything is applied, so a patch computed against a version someone has
since replaced is refused with `409` rather than merged into content it was
never written for. The refusal carries the digest that is actually stored, so
the caller can re-read, rebase, and retry without a round trip to find out what
it is working against. Omitting `base_sha256` means "edit whatever is there",
which is fine for a file only one author touches.

**Which part of the file is being edited.** An `old_string` that appears more
than once is refused: an edit meant for one of three identical lines cannot be
aimed by content alone, and picking one would be a guess. Include enough
surrounding text to make the match unique, or pass `replace_all: true` to say
every occurrence was meant.

## The request

| Field         | Default | Meaning                                                                            |
| ------------- | ------- | ---------------------------------------------------------------------------------- |
| `script`      | —       | URI of the script that owns the asset (required; may also be a query parameter)    |
| `asset`       | —       | Path of the asset within the script (required; may also be a query parameter)      |
| `edits`       | —       | The replacements to apply, in order (required, at least one, at most 128)          |
| `base_sha256` | —       | Digest the asset is expected to have right now; the patch is refused if it has not |
| `reinit`      | `after` | `after` runs the script's `init()` once the edits land; `never` leaves it alone    |

Each entry in `edits`:

| Field         | Default | Meaning                                                         |
| ------------- | ------- | --------------------------------------------------------------- |
| `old_string`  | —       | Text to find (required, non-empty, unique unless `replace_all`) |
| `new_string`  | —       | Text to put in its place (required; `""` deletes)               |
| `replace_all` | `false` | Replace every occurrence rather than requiring exactly one      |

`new_string` is required rather than defaulting to empty, so a misspelled field
name cannot quietly turn a replacement into a deletion.

Editing an asset takes the same rights as writing one: the `WriteAssets`
capability, ownership of the script, or administrator. Only UTF-8 text can be
edited as strings — a patch against a binary asset is refused, with the advice
to write it whole instead.

## All or nothing

Edits apply in order, each against what the previous one left behind, and all
of them are applied in memory before the asset is stored. A patch whose third
edit does not match writes nothing: the deployed file is exactly as it was,
rather than carrying the first two edits of a change the caller no longer knows
the state of.

When the edits land, the asset is written the way a batch is written — one
transaction, one notification to the rest of the cluster, one `init()`.

## The answer

```json
{
  "script": "myapp",
  "asset": "server/move-player.ts",
  "sha256": "c07…",
  "bytes": 1841,
  "replacements": 1,
  "status": "updated",
  "init": { "ran": true, "success": true, "durationMs": 34, "error": null }
}
```

`sha256` is the digest of the content that now stands, so a caller making a
series of edits can send it as the next patch's `base_sha256` without reading
the file again. `replacements` counts the occurrences replaced across all
edits, which is how a `replace_all` edit reports how much it touched.

`status` is `updated`, or `unchanged` when the edits cancelled each other out —
that file is not rewritten, and `init` reports `{"ran": false, "reason": "no
change"}`, since there is nothing for it to pick up.

## Reading part of a file

The counterpart to editing without sending the file is reading without
receiving it. `GET /engine/assets` takes two optional filters on a single
asset:

```bash
# lines 120 to 180, as text
curl "…/engine/assets?script=myapp&asset=server/move-player.ts&lines=120-180"

# where a pattern matches, without the file
curl "…/engine/assets?script=myapp&asset=server/move-player.ts&grep=^export%20function"
```

`lines` accepts `120-180`, `120-` (to the end of the file), or `120` (that line
alone), counting from 1. An `end` past the end of the file clamps — `120-1000`
of a 400-line file plainly means the rest of it. A `start` past the end is
refused with `400` naming the file's actual length: a range that far out is
usually one computed against a version that has since shrunk, and an empty
`200` would leave the caller to work that out for itself.

`grep` is a regular expression, matched line by line, and answers with the line
numbers and their text instead of the content:

```json
{
  "script": "myapp",
  "asset": "server/move-player.ts",
  "encoding": "utf8",
  "matches": [
    {
      "line": 137,
      "text": "export function movePlayer(id) {",
      "truncated": false
    }
  ],
  "match_count": 1,
  "truncated": false,
  "total_lines": 412,
  "sha256": "9f2…",
  "bytes": 11204
}
```

The two compose: a `grep` given with `lines` searches only that range. A
listing stops after 200 matches and says so with `truncated`; an individual
line is cut at 512 characters, with `truncated` on the match itself.

Both filters require the asset to be UTF-8 text. A read with neither is
unchanged from what it always was — the whole file, base64 in `content` — so
existing callers are unaffected.

Reading a script's assets through `/engine/*` takes an authenticated caller
with the `ReadAssets` capability, ownership of the script, or administrator —
the same rule the rest of the management surface applies. This is not the same
permission a script exercises when it reads its own assets while serving a
public request: that runs in the sandbox, with the requesting user's context,
and is unaffected.

A range is a view, not a transcript: lines come back joined with `\n`, whatever
the file uses, and without its trailing newline. To build an `old_string` for a
file with CRLF endings, take the bytes from an unscoped read — a patch matches
the stored content exactly, so the line endings in `old_string` have to be the
ones in the file.

Every read, scoped or whole, reports `sha256` and `bytes` for the **whole**
asset rather than for the part returned. That is deliberate: the digest's
purpose is to be handed back as a patch's `base_sha256`, so a caller that read
twenty lines can still edit them safely.

## Over MCP

`edit_asset` takes the same arguments as the endpoint and answers with the same
body, including the `init` block. `read_asset` takes the same `lines` and
`grep` filters.

```json
{
  "script": "myapp",
  "asset": "server/move-player.ts",
  "edits": [
    { "old_string": "const SPEED = 4;", "new_string": "const SPEED = 6;" }
  ],
  "base_sha256": "9f2…"
}
```

Together they are the loop an agent editing a solution actually runs: `grep` to
find the place, `lines` to read around it, `edit_asset` to change it, with the
digest carried through so the change lands on the version it was written for.
