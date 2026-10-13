# Sharing a Solution Through Git

```bash
# Take a repository's scripts into this engine
pull_from_git(repo: "owner/solution")

# Publish a script's files back
push_to_git(script: "solution")

# Ask which of those two you want
get_git_status(script: "solution")
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

- **The script's name.** One engine already has a `shop`; somebody pulling the
  same repository elsewhere calls it something else, or has renamed it since. A
  repository recording the name records a value correct on exactly one machine.
- **Ownership and host bindings.** If those arrived from a repository, push
  access to it would confer ownership inside somebody's engine.

Strip both out and there is nothing left to declare. The pulling engine
composes its own name, and a script is named after where it came from — the
directory `shop` becomes the script `shop`, and a single-script repository
takes the repository's name (`solution`). A name is a slug: no origin and no
extension, because the entry file's own name (`main.ts`) is what says whether it
is transpiled.

The name is composed **once**. The directory a script is goes on its sync row,
and every later pull and push finds the script by that, so renaming it
(`rename_script`) does not make the next pull write a second one beside it.

## Telling it what not to take

The built-in exclusions cover what is always junk: dotfiles, `node_modules`,
and a `README.md` or license file (`LICENSE`, `LICENSE-MIT`, …) beside a script's entry. They cannot cover what
is junk in _your_ repository, so name it in `.aiwebengineignore` at the root:

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

What the ignore file excludes is also **what the engine keeps for itself**. A
file a script holds at a path the repository excludes is never the repository's:
a pull leaves it where it is and does not count it as a change made here, a
push does not publish it, and status does not report it as `ahead`. The engine
keeps the rules from the last sync to tell, so a change to them counts from the
next pull or push. That is how a script writes files of its own and
still follows its repository — an agent keeps the skills it learns under
`learned/`, and its repository ignores `agent/learned/`.

This is not `.gitignore`, and it is not read instead of one. Everything
`.gitignore` lists is uncommitted and never reaches the engine anyway; this
file answers the other question — of the files git _does_ track, which are
content.

## Pulling

```
pull_from_git(repo: "owner/solution")
pull_from_git(repo: "https://github.com/owner/solution.git", ref: "next")
pull_from_git(repo: "owner/solution", ref: "v1.4.0")
pull_from_git(repo: "owner/solution", prefix: "examples")
```

`repo` takes `owner/repo`, a browser URL, or a clone URL. `prefix` is a name
put in front of each directory's (`examples-shop`) — set it when two
repositories would otherwise both want `shop`. With no prefix a directory is
simply its own name, and a repository that is one script takes the
repository's name.

### Which version

`ref` says what to read:

| `ref`                           | Reads                                                       |
| ------------------------------- | ----------------------------------------------------------- |
| `main`, `v1.4.0`                | that branch or tag; a name that is both is refused          |
| `refs/heads/v1`, `refs/tags/v1` | the branch, or the tag, when a name is both                 |
| `3f2a…` (all 40 characters)     | that one commit; a short sha is refused                     |
| _(omitted)_                     | what the repository is followed at, else its default branch |

An engine follows a repository **once**: at one ref and under one prefix. A
pull that names another ref moves it there — the scripts the old ref wrote are
the ones written, renamed or not, and later pulls naming no ref read the new
one. That is how an installation upgrades:

```
pull_from_git(repo: "owner/agent", ref: "v1.4.0")   # install
pull_from_git(repo: "owner/agent")                  # still v1.4.0, whatever main does
pull_from_git(repo: "owner/agent", ref: "v1.5.0")   # upgrade
```

A tag is what several engines should share: each one runs exactly what was
released, and moves when somebody decides it should. A branch is for the
engine you develop on. A commit is for the times nothing else will do; it is
pulled once, and cannot be followed on a schedule, since it never moves.

The stored form is git's own — `refs/heads/main`, `refs/tags/v1.4.0`, or the
sha — and that is what reports and listings show as `ref`.

Leaving `prefix` out keeps the one the repository is followed under; an empty
one is no prefix. A prefix names the scripts a pull creates, so changing it
leaves the scripts already pulled where they are.

### Taking some of the scripts

```
pull_from_git(repo: "owner/examples", scripts: ["virtual-world", "blog"])
pull_from_git(repo: "owner/examples")
```

`scripts` names the top-level directories to take, as the repository spells
them, and leaves the rest where they are. A name the repository does not hold
as a script is refused, and so is any name at all for a repository that is
itself one script.

The engine remembers what it follows from each repository (the `git_follows`
table): either the whole repository or a list of directories. A pull that names no scripts takes what is followed, so the second
line above refreshes `virtual-world` and `blog` and nothing else, and a script
added upstream later is not pulled in by a pull that did not ask for it. A
first pull naming none follows the whole repository.

- Naming another directory adds it to a partial follow.
- Naming one script of a whole-repository follow pulls that one now and keeps
  following the rest.
- A followed directory removed upstream is skipped rather than refused, so the
  others keep updating.
- Clearing a script's binding (`clear_git_remote`) stops following its
  directory; clearing the last one ends the follow, and the next pull starts
  from nothing.
- Pushing a script into a repository follows that script, not everything
  already beside it.

A pull is a **sync, not an append**: a module deleted upstream is deleted here,
because a script that kept building against a file its source of truth no
longer has is a script nobody can reason about. Everything the pull writes for
one script — the entry, the assets, the removals — is one revision and one
`init()`.

So a pull **will not replace a change made here**. When a script's files differ
from what its last sync left, the pull is refused, names the files, and writes
nothing. Push the change first, or pass `force: true` to take the repository's
copy; the replaced files stay in the script's revisions. Content is what
counts: a file written and deleted again is no change. The revision a sync last
agreed on is kept by revision pruning, since it is what the comparison reads.

Pulling a repository that has not moved costs two small API calls and does
nothing. `force: true` also downloads and re-applies anyway, for the times the
engine's answer looks stale for a reason the commit cannot show. Forcing decides
whether to _look_, not what to conclude: identical content still writes nothing
and records no revision.

### Which engine it needs

```
# .aiwebengine-requires, at the repository's root
2026-10-12
```

Scripts are written against the engine they were developed on, and the script
API changes in ways that break them. An engine reports its own script API as a
date, `script_api` in `/health`: the day of the last change that broke what
scripts see. A repository names the date it needs in `.aiwebengine-requires`,
and an engine older than that refuses to pull it, names both dates, and writes
nothing. Updating the engine is what lets the next pull through; `force` takes
it anyway. A repository without the file needs nothing, and the file never
becomes one of a script's files.

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
push_to_git(script: "solution",
            message: "Fix the cart total")
```

The script's root becomes `main.{ext}` and its assets keep their paths, laid
out the way a pull reads them back — so what a push writes pulls onto any other
engine. `repo` is optional once a script has been pulled, since it already
knows where it came from.

A push moves a branch: the one the script was pulled from, or the one named
with `branch`. A script pulled at a tag or a commit has no branch to default
to, so its push names one, and the repository is followed at that branch from
then on.

**Files the script does not own are left untouched.** The README, the CI
workflow, everything the ignore file excludes — from the engine's side those
are indistinguishable from files deleted upstream, and a push that treated its
own assets as the whole truth would remove all of them on the first round trip.
A file the repository already holds is recognised without being uploaded, so an
unchanged script makes no commit at all.

Pushing takes ownership of the script or administrator, and needs a stored
credential with write access.

## Following on a schedule

```
follow_git(repo: "owner/examples", scripts: ["virtual-world"], every_minutes: 60)
follow_git(repo: "owner/examples", every_minutes: 60, mode: "stage")
follow_git(repo: "owner/agent", ref: "v1", every_minutes: 1440)
follow_git(repo: "owner/examples", every_minutes: 0)          # stop
```

A follow can pull on its own, every 10 minutes to once a week. It is the same
pull a person makes, from the same follow, and it refuses the same things: it
never replaces a script changed here, and it never writes a script the
repository did not put there.

It runs at the ref the repository is followed at; `ref` moves it, and leaving
it out keeps it. Following a tag a project moves on release — a `v1` that
always names the newest 1.x — is how a set of engines takes releases and
nothing in between. A commit cannot be followed.

| `mode`  | Each run                                                                                                                         |
| ------- | -------------------------------------------------------------------------------------------------------------------------------- |
| `apply` | pulls; what it pulls is served, unless the script is pinned (the default)                                                        |
| `stage` | pins every followed script to what it serves now, then pulls into head; `deploy_script` serves it. A new script is live at once. |
| `check` | records whether the repository has moved, and writes nothing                                                                     |

**Whose authority.** A run acts as the account that set the schedule, with that
account's stored GitHub credential. The account is read again on every run,
capped at editor and narrowed to reading and writing scripts and their files,
so a follow only ever writes scripts that account owns, and an administrator's
follow does not act on other people's scripts. Only the person can schedule
one, in their own session or token: work running under a delegation cannot,
because a follow would outlast its grant. Once scheduled, only that account or
an administrator can change it — and that includes moving it to another ref
or prefix, by `follow_git`, a pull or a push.

**When it stops.** Anything that changes what the account may do — its roles,
its realm, its password, its deletion — pauses its follows, as it withdraws its
delegations. A run that finds the account gone or no longer an editor pauses
the follow too. A paused follow keeps what it follows; set the schedule again
to resume. There is no expiry otherwise.

**How it runs.** Every minute the engine claims up to ten due follows. Claiming
moves the next run on in the same statement, so one instance runs each, and an
engine that slept runs each once rather than once per interval missed. Each run
spends the account's git budget like a pull by hand.

**When it stops going through.** Each follow keeps how its last run went:

| `state`        | Meaning                                                                  |
| -------------- | ------------------------------------------------------------------------ |
| `ok`           | it pulled, found nothing to pull, or checked                             |
| `blocked`      | a followed script was changed here, so the pull was refused until pushed |
| `incompatible` | the repository needs a newer engine; the first run after an update pulls |
| `failing`      | anything else: GitHub did not answer, a script is another account's      |
| `paused`       | it runs again only when scheduled again                                  |

A follow that leaves `ok` writes one line to the log of each script it is
about, a warning naming why; one that comes back writes one more. A write to a
followed script (`write_file`, `write_files`, `edit_file`, `create_file`,
`delete_file`) answers with a `followed` note saying its pulls are refused
until the change is pushed, and `get_git_status` reports the script's follow.

`list_git_bindings` shows each follow you scheduled or recorded (all of them to
an administrator): its schedule, mode, next run, state, and how the last run
went.

## Knowing which of the two you want

```
get_git_status(script: "solution")
```

| State         | Meaning                   | Do                           |
| ------------- | ------------------------- | ---------------------------- |
| `in_sync`     | neither side has moved    | nothing                      |
| `ahead`       | you changed it            | push                         |
| `behind`      | they changed it           | pull                         |
| `diverged`    | both changed it           | reconcile, then push         |
| `unbound`     | never synced anywhere     | push, naming a repository    |
| `unreachable` | GitHub could not be asked | only the local half is known |

Ask this rather than attempting an operation and reading the refusal. `ref`
is what the script was last synced at, and `branch` is where a push goes by
default — empty for a tag or a commit. A script pulled at a commit is never
`behind`, since a commit does not move. The answer also carries the deployment
pin, because a pull into a pinned script
advances head without changing what answers requests — correct, and worth
knowing at the point you are deciding.

## What it refuses, and why

**Both sides moved.** The engine does not merge. Reconciling two versions of a
module is a judgement about code, which is what the agent asking for the push is
for — and `/engine/diff_revisions` shows what changed here. The refusal names
both sides. `force` publishes your copy over the repository's; GitHub still
refuses a non-fast-forward, so it cannot overwrite work the engine has not seen.

**The target is already occupied.** A pull will not overwrite a script that
this repository did not write. Owning a script is not the same as having agreed
that a repository may replace it, and a repository with a `shop` directory
would otherwise be enough to replace somebody's `shop`. Pull under a different
prefix, or `force` if you meant it. A second pull
of the _same_ repository is unaffected — replacing what it wrote is the point.

## Where a script came from

```
list_git_bindings()             # what tracks what, and what is followed
clear_git_remote(script: "x")   # stop tracking
```

Clearing a binding leaves the script and its files exactly as they are; what
goes is the record of where they came from, so later pulls do not treat it
as that repository's to replace.

There is deliberately no call that _sets_ a binding without writing anything. A
push names its repository and records the binding when it lands; one that has
been neither pushed nor pulled would describe an agreement neither side has
made.

## Limits

- **GitHub only.** GitLab and Gitea expose equivalent APIs in different shapes,
  and picking an abstraction before a second host exists produces one fitted to
  a single example.
- **Only pulls run unattended**, and only a scheduled follow's. Pushes are
  always asked for. There are no webhooks: a desktop engine cannot receive one,
  so a follow asks instead.
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
