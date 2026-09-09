# Sharing a Solution Through Git

```bash
# Take a repository's scripts into this engine
pull_from_git(repo: "owner/solution")

# Publish a script's files back
push_to_git(script: "https://example.com/solution.js")

# Ask which of those two you want
get_git_status(script: "https://example.com/solution.js")
```

A solution lives in this engine's database, which is the right place for it
right up to the moment a second person wants it. There is nowhere to publish it
to and nothing to pull it from, and copying files by hand stops being possible
the moment a solution is more than one of them.

This is the other half. A repository holds a solution; an engine reads it, runs
it, and can put changes back. Nothing on either side needs a checkout, a git
binary, or a working copy — the engine talks to GitHub over HTTPS and keeps
everything it reads in memory.

## What the repository has to look like

Nothing in the repository says where its files land. The engine works it out
from the directory structure, so the same repository pulls onto any install.

A directory holding `main.ts` — or `main.js`, `main.tsx`, `main.jsx` — is one
script. Every other file under it becomes one of that script's assets at the
same relative path:

```
solution/
  shop/
    main.ts              → the script
    lib/cart.ts          → asset "lib/cart.ts"
    templates/page.html  → asset "templates/page.html"
  admin/
    main.ts              → a second script
```

A repository with an entry at its own root is itself one script:

```
solution/
  main.ts                → the script
  server/handler.ts      → asset "server/handler.ts"
```

That reading wins over any subdirectories, so adding a folder to a
single-script repository does not silently turn it into something else.

Asset names are relative paths and the engine's import specifiers are relative,
so **a repository subtree and what the engine stores are the same thing**. No
specifier is rewritten in either direction.

### Why there is no manifest

Two things would go in one, and neither can travel:

- **The script's URI.** One engine composes
  `https://her-host/shop/main.ts`; somebody pulling the same repository
  elsewhere has a different host entirely. A repository recording the full URI
  records a value correct on exactly one machine.
- **Ownership and host bindings.** If those arrived from a repository, push
  access to it would confer ownership inside somebody's engine.

Strip both out and there is nothing left to declare. The pulling engine
composes its own URI, and a script is named after where it came from — `shop`
becomes `.../solution/shop.ts`, and a single-script repository becomes
`.../solution.ts`. The extension follows the entry, because that is what
decides whether the file is transpiled.

## Telling it what not to take

The built-in exclusions cover what is always junk: dotfiles, `node_modules`,
and a `README.md` or `LICENSE` beside a script's entry. They cannot cover what
is junk in *your* repository, so name it in `.aiwebengineignore` at the root:

```gitignore
# kept for editors, meaningless to the engine
tsconfig.json
package-lock.json

# documentation, not content this solution serves
docs/
*.md
# ...except the one page it does serve
!shop/templates/help.md

# fixtures the test runner reads, never from an asset
**/__fixtures__/
```

The syntax is the one you already know: `#` comments, `*` and `?` within a
segment, `**` across them, a trailing `/` for directories, a leading `!` to
put something back, and last match wins. A pattern with no slash matches at any
depth; one with a slash is anchored at the repository root, so `shop/docs/`
takes shop's and leaves another script's alone.

> **`!` cannot bring back a dotfile.** Assets are servable over HTTP, so a
> re-included `.env` is a credential leak — and the mistake would not be
> deliberate, it would be one broad pattern that caught more than its author
> meant. That floor is not negotiable.

This is not `.gitignore`, and it is not read instead of one. Everything
`.gitignore` lists is uncommitted and never reaches the engine anyway; this
file answers the other question — of the files git *does* track, which are
content.

## Pulling

```
pull_from_git(repo: "owner/solution")
pull_from_git(repo: "https://github.com/owner/solution.git", branch: "next")
pull_from_git(repo: "owner/solution", prefix: "examples")
```

`repo` takes `owner/repo`, a browser URL, or a clone URL. `branch` defaults to
the repository's own default. `prefix` is where the scripts land, defaulted
from the repository name — set it when two repositories would otherwise
collide, or to put a solution somewhere specific.

A pull is a **sync, not an append**: a module deleted upstream is deleted here,
because a script that kept building against a file its source of truth no
longer has is a script nobody can reason about. Everything the pull writes for
one script — the entry, the assets, the removals — is one revision and one
`init()`.

Pulling a repository that has not moved costs two small API calls and does
nothing. Pass `force: true` to download and re-apply anyway, for the times the
engine's answer looks stale for a reason the commit cannot show. Forcing decides
whether to *look*, not what to conclude: identical content still writes nothing
and records no revision.

### Private repositories

Store a token once:

```
set_git_credential(token: "github_pat_...")
```

A fine-grained token scoped to the repositories you want is enough — it needs
no more than read access to their contents, and write access only if you intend
to push.

The token is encrypted at rest, is never returned by any endpoint or tool, and
is not reachable from JavaScript. It is checked against GitHub before being
stored, so a mistyped or revoked token is refused now rather than at the next
pull. It is always your own: no request names another account.

```
list_git_credentials()      # host, account, when added and last used
delete_git_credential()     # stop using it
```

An engine with no `security.secret_encryption_key` configured refuses to store
a token rather than writing it in the clear.

## Pushing

```
push_to_git(script: "https://example.com/solution.js",
            message: "Fix the cart total")
```

The script's root becomes `main.{ext}` and its assets keep their paths, laid
out the way a pull reads them back — so what a push writes pulls onto any other
engine. `repo` is optional once a script has been pulled, since it already
knows where it came from.

**Files the script does not own are left untouched.** The README, the CI
workflow, everything the ignore file excludes — from the engine's side those
are indistinguishable from files deleted upstream, and a push that treated its
own assets as the whole truth would remove all of them on the first round trip.
A file the repository already holds is recognised without being uploaded, so an
unchanged script makes no commit at all.

Pushing takes ownership of the script or administrator, and needs a stored
credential with write access.

## Knowing which of the two you want

```
get_git_status(script: "https://example.com/solution.js")
```

| State | Meaning | Do |
| --- | --- | --- |
| `in_sync` | neither side has moved | nothing |
| `ahead` | you changed it | push |
| `behind` | they changed it | pull |
| `diverged` | both changed it | reconcile, then push |
| `unbound` | never synced anywhere | push, naming a repository |
| `unreachable` | GitHub could not be asked | only the local half is known |

Ask this rather than attempting an operation and reading the refusal. The
answer also carries the deployment pin, because a pull into a pinned script
advances head without changing what answers requests — correct, and worth
knowing at the point you are deciding.

## What it refuses, and why

**Both sides moved.** The engine does not merge. Reconciling two versions of a
module is a judgement about code, which is what the agent asking for the push is
for — and `/engine/revisions/diff` shows what changed here. The refusal names
both sides. `force` publishes your copy over the repository's; GitHub still
refuses a non-fast-forward, so it cannot overwrite work the engine has not seen.

**The target is already occupied.** A pull will not overwrite a script that
this repository did not write. Owning a script is not the same as having agreed
that a repository may replace it, and since the prefix defaults to the
repository's name, a repository named like an existing prefix would otherwise be
enough. Pull under a different prefix, or `force` if you meant it. A second pull
of the *same* repository is unaffected — replacing what it wrote is the point.

## Where a script came from

```
list_git_bindings()                                    # what tracks what
clear_git_remote(script: "https://example.com/x.js")   # stop tracking
```

Clearing a binding leaves the script and its files exactly as they are; what
goes is the record of where they came from, so later pulls no longer treat it
as that repository's to replace.

There is deliberately no call that *sets* a binding without writing anything. A
push names its repository and records the binding when it lands; one that has
been neither pushed nor pulled would describe an agreement neither side has
made.

## Limits

- **GitHub only.** GitLab and Gitea expose equivalent APIs in different shapes,
  and picking an abstraction before a second host exists produces one fitted to
  a single example.
- **Nothing runs unattended.** No scheduled pulls, no webhooks. Every sync is
  something a person or their agent asked for, so the credential used is always
  one whose owner is present.
- **Sixty operations an hour per account**, refilling one per ten seconds. A
  pull downloads an archive and rewrites a tree; iterating on a solution never
  notices this, and a loop stops within the minute.
- **Operators can restrict or disable it** with `allowed_remotes` under
  `[git]`. Empty allows every supported host; a list that does not name
  `github.com` refuses every pull.

## Related

- [Script Revisions](SCRIPT_REVISIONS.md) — what a pull records, and how to put
  a version back
- [Atomic Multi-File Writes](ASSET_BATCH.md) — the write a pull is built on
- [Script Checks](SCRIPT_CHECKS.md) — checking a pulled revision before serving
  it
