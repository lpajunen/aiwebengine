# What a Script's Files Have Been

Every write to a script records what the script became. Not a backup taken on
request, and not a file-by-file log — a numbered revision of the whole script,
recorded by the same call that stored the files.

```bash
# What has happened to this script
curl "https://your-engine/engine/revisions?script=myapp"

# What has happened to one of its files
curl "https://your-engine/engine/revisions?script=myapp&asset=server/move-player.ts"

# Check revision 41 without deploying it
curl -X POST "https://your-engine/engine/check?uri=myapp&revision=41"

# Run the tests revision 41 had, against the modules revision 41 had
curl -X POST "https://your-engine/engine/run_tests?uri=myapp&revision=41"
```

## Why the write records it, and not the caller

The obvious design is a snapshot endpoint: call it before something risky, name
the snapshot, restore it if the risk lands badly.

That design has a precondition the failure removes. Taking the snapshot
requires knowing beforehand that the change is dangerous, and the changes that
turn out to be dangerous are mostly the ones that did not look it. An agent
editing a module through `PATCH /engine/assets` has no checkout to fall back
on, and no reliable sense of which of its edits is the one worth marking.

So the recording is unconditional and naming is retrospective. Every write
produces a revision; a label is applied afterwards, to a revision that already
exists, by whoever has since decided it was worth naming. "Put it back to
twenty minutes ago" works whether or not anyone was thinking ahead.

## Why the script, and not the file

An asset has no identity apart from its script: it is keyed by
`(script_uri, uri)` and deleted with it. One write already stores several files
in a single transaction. And the changes worth undoing span modules — a schema
step plus the modules that read it — where restoring one file at a time
reintroduces exactly the inconsistent state you were escaping.

A revision is therefore the whole script: its root source and the complete list
of its files. A per-file history is a query over those manifests; the reverse
does not hold, because a manifest cannot be reassembled from independent
per-file logs without guessing which versions were current together.

This is affordable because content is addressed by digest and stored once. A
revision of a forty-file script that changed one module adds one blob and forty
narrow rows, not forty copies of the content.

## What a revision knows that a checkout does not

```json
{
  "revision": 42,
  "parent": 41,
  "origin": "patch",
  "label": null,
  "at": "2026-08-26T09:14:02Z",
  "by": "user-4c1f",
  "files": 38,
  "bytes": 214093,
  "initOk": false,
  "initError": "TypeError: rules.forEach is not a function"
}
```

`initOk` is the engine reporting on code it ran. Every write triggers the
script's `init()`, and the outcome is recorded against the revision that write
produced. The listing therefore carries `lastGood` — the newest revision whose
`init()` succeeded — which is what an operator means by "back to when it
worked", and a number they would otherwise have to find by reading the history
themselves.

`initOk` is absent, rather than `false`, when no `init()` has reported. A
revision whose outcome is unknown is not one that failed, and anyone looking
for somewhere safe to land has to be able to tell those apart.

`origin` says which act produced the revision: `post`, `batch`, `patch`,
`delete`, `script`, `revert`, or `bootstrap`. `parent` is the revision this one
was computed against — not simply `revision - 1`, so that a history reads as a
graph rather than as a line that silently doubles back.

## A write that changed nothing is not a revision

Rewriting a file with the content it already has records nothing, and the
response reports `"revision": null`. The same holds for a patch whose edits
cancel out and a batch in which every file already matched. Recording those
would push real history out of any retention window that counts rows.

## Building from a version other than the deployed one

`check`, `run_tests` and `eval` resolve a script's imports through whichever
version they are pointed at. Without `revision`, that is what is deployed —
which is what every existing caller has always meant.

| `revision=` | Reads from                                   |
| ----------- | -------------------------------------------- |
| _(omitted)_ | The deployed files                           |
| `41`        | Revision 41                                  |
| `head`      | The newest recorded revision                 |
| `last-good` | The newest revision whose `init()` succeeded |
| _a label_   | Whatever revision carries that name          |

A revision that does not exist is refused rather than quietly falling back to
what is deployed.

This matters most when head is broken. A root that imports a module someone has
since deleted cannot be bundled at all, and until now that made every earlier
version equally unreachable: there was nothing else to build from. Pointed at
the revision that still held the module, the same check succeeds, which is how
you find out that the deletion — rather than anything else in the change — is
what broke it.

Combining `revision` with candidate `content` checks a candidate root against
that revision's modules: the tree the change was written for, rather than
whatever head has since become.

## Putting it back

```bash
# What would change, without changing it
curl -X POST "…/engine/revisions/revert?script=myapp&revision=41&dry_run=true"

# Do it
curl -X POST "…/engine/revisions/revert?script=myapp&revision=last-good"
```

A revert is a **forward write**. The restored content becomes a new revision
whose parent is the one it came from, so the history reads as a graph — "48 is
41 again" — rather than as a line that silently doubles back. Nothing is
rewritten, and the cluster notification, the cache invalidation and the
`init()` that follows are the same ones any other write goes through, rather
than a second path into the same caches with its own way of going wrong.

It restores files whose content differs, and **removes files the target
revision did not contain**. Leaving those behind would produce a tree that is
neither version — a module deleted in the change stays around, imported by
nobody, until the next person assumes it is current. The whole thing is one
transaction, so a revert never lands halfway.

Files the target holds that the deployment already has are not rewritten, so
reverting to a nearby revision is as small as the change that caused it.

### It will not deploy a revision that does not build

Because the engine can bundle a revision without deploying it, it checks first:

```json
{
  "error": "Revision 37 does not bundle: Module './server/rules.ts' imported
            from 'main.ts' was not found in assets for 'myapp'.
            Pass force to restore it anyway."
}
```

That is a `409`, before anything is written. `force=true` says the caller meant
it.

Reverting to what is already deployed changes nothing and records no revision —
`"revision": null`, and `init()` is not run, because there is nothing for it to
pick up.

## How long history is kept

```toml
[revisions]
prune_enabled = true
retention_days = 30
keep_per_script = 50
prune_interval_secs = 3600
```

Recording a revision on every write is what makes the history worth having and
also what makes it grow — an agent editing through `PATCH /engine/assets`
writes far more often than a person does. A pass runs on the interval above,
on one instance at a time.

A revision is removed only when it is older than `retention_days` **and**
outside the newest `keep_per_script` of its script. Either clause on its own
deletes history someone is plausibly still using.

Three things are never removed, whatever the numbers say:

- a **labelled** revision — a label is someone having said out loud that this
  one is worth returning to, and retention does not get to disagree;
- the newest revision that **initialised cleanly**, because it is the floor a
  rollback lands on, and collecting it would take away the answer exactly when
  the question gets asked;
- the **newest** revision of every script, which is what the script currently
  is.

Content is collected separately and only once no revision cites it, since blobs
are shared across revisions and across scripts — one revision going away says
nothing about whether its bytes are still someone else's. A blob is also left
alone for an hour after the last write that cited it, which is the window a
write needs between claiming content and recording the manifest that points at
it.

## Reading a file's history

```bash
curl "…/engine/revisions?script=myapp&asset=server/move-player.ts"
```

Only the revisions in which that file actually changed. Consecutive revisions
that left it alone report the same digest, and listing each of them answers a
question nobody asked.

## Scripts that predate their own history

At startup, a script with no revisions gets one recording its current state.
It is a backfill, not a per-boot snapshot: a script that already has history is
left alone. Every script therefore has somewhere to return to from the first
boot after this ships, rather than acquiring its first revision only once
someone changes it — by which point the state they might have wanted back is
the one that was just overwritten.

## What is not versioned

Code, and only code. A script's tables, its properties, personal storage and
its secrets are outside every revision.

This is a real boundary, not an oversight to be worked around. Reverting the
modules that read a table does not un-add the column, and a revert that dropped
the column to match would destroy data in order to restore code. Treat a
revision as an answer to "what was the code", and treat the data it ran against
as a separate question with a separate answer.
