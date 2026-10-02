# What Narrows What

A solution developer meets the engine's limits in five places, written at five
different times, in five documents, each with its own word for the same thing.
This is the one page that puts them side by side. It adds no mechanism; every
row below exists and is documented where it is built.

## The operation they share

Every row that narrows **authority** — what a caller may do — is the same
operation: `UserContext::attenuated`, an intersection of a capability set with a
requested one. That is the whole of it, and it has four consequences worth
holding in your head instead of re-learning per layer:

- **It can only take away.** Nothing that narrows can widen, and the order
  several of them are applied in does not change the result, because
  intersection commutes. A layer that wants to grant something is not this
  operation (the tiers and the two scopes that raise one are the exceptions, and
  they are bounded by the account's roles too — see _Delegation_).
- **It never changes who is acting.** `user_id` carries through, so ownership
  checks, `personalStorage` and `{{secret:...}}` go on resolving against the same
  account. A narrowed context that changed identity would silently move a
  script's writes into somebody else's rows.
- **Asking for more than you hold is refused at the call**, by name, rather
  than silently narrowed — `sandbox.run` throws, a delegation names the scope,
  an unknown capability is refused rather than dropped. Silent narrowing hides
  the bug; a refusal names it.
- **A refusal says what was missing.** A 403 from `/engine/*` or `/mcp` carries
  the capability it needed (`required_capabilities`), and, where an elevation
  would supply it, where to go to get that.

## The layers, outermost first

| Layer                 | Narrows                                                                          | Who decides                             | How long it lasts      | Where it is documented         |
| --------------------- | -------------------------------------------------------------------------------- | --------------------------------------- | ---------------------- | ------------------------------ |
| **Role**              | which tier an account is                                                         | an administrator                        | until changed          | `INTERNAL_AUTH.md`, CLAUDE.md  |
| **Realm**             | which hosts an account is a principal on                                         | an administrator, or bootstrap          | until changed          | CLAUDE.md (identities, realms) |
| **Token audience**    | which host and path a bearer token may be used on, and which grades it may carry | the OAuth2 client's request, at consent | the token's life       | CLAUDE.md (`/mcp`, OAuth2)     |
| **Session elevation** | which gated capabilities the session holds _now_                                 | the person, by re-authenticating        | minutes                | `SESSION_ELEVATION.md`         |
| **Delegation**        | what a script may do as a person while they are away                             | the person, at `/auth/delegate`         | days, mandatory expiry | `DELEGATION.md`                |
| **Sub-execution**     | what one piece of code, and which hosts it may reach                             | the script, `sandbox.run`               | one call               | `CAPABILITY_ATTENUATION.md`    |
| **Script limits**     | what one script may _spend_: time, memory                                        | an administrator                        | until changed          | `SCRIPT_LIMITS.md`             |

Read top to bottom it is the order a request meets them: an account has a tier
and a realm, a credential has an audience, a session has an elevation, a
background run has a delegation, and a call inside a script has whatever the
script chose to narrow it to. The last row is not authority at all; it sits in
the table because it is the one a developer reaches for in the same breath.

### Role and tier

Four tiers, fixed: anonymous, authenticated, editor, administrator. Authenticated
is someone _using_ a solution and holds what an ordinary request needs;
editor adds authoring, always paired with an ownership check; administrator adds
`AdministerEngine`, the marker for acting on what you do not own. Test that
capability and not `DeleteScripts` when the question is "is this an
administrator".

Half of `Capability` names what a person may do to a solution and half names what
a script does while it runs (`use_network`, `read_secrets`, `read_storage`,
`enqueue_tasks`, ...). Every tier that could already do the second half still
holds it, the anonymous tier included — a public script serving a signed-out
visitor needs it — so they exist to be taken away by the layers below, not to
refuse anyone a tier gave them.

### Realm

Not a capability: _where_ an account is a principal. Recorded at creation from
the sign-in host, `*` for every host. It is checked where a session is validated
or refreshed, below every middleware, because a session token is accepted from a
bearer header as readily as from a cookie and a header carries none of the
host scoping a browser applies.

### Token audience

A token the OAuth2 endpoint issues is bound to a resource — the host and path
being protected (`/mcp` on one host) — and `scope=` can name the elevation
grades it may carry. A browser login carries no audience, so a session cookie is
not an API credential. A refresh token is a separate credential from the session
it mints, and a refresh re-reads roles and realm instead of copying them
forward.

### Session elevation

A session starts at the authenticated floor whatever roles the account holds,
and the rest is switched on for minutes at a time by a deliberate act. Two
grades, because the engine asks two questions: `author` (create and change the
scripts you own — ownership is still checked at every write) and `administer`
(act on what you do not own). Configured under `[security.elevation]`; **empty
`gated` is the whole of "off"**, which is the default, so an engine that names
nothing behaves as though the layer were absent.

### Delegation

What a person has authorised a script to do as them while they are away. The
tier _defaults_ to authenticated however much the person holds, then
`context_for` attenuates it to the scopes granted. Scopes name a noun the
person's thing is reached through (`personal_storage`, `secrets`) or a verb
(`write`) — nouns are checked where the thing is reached and answer as though it
were not there, the verb is a capability and refuses at the gate every other
caller meets. **`author` and `administer` are the only scopes that raise the
tier instead of narrowing within it**, and they are bounded by the account's own
roles as well: ticking `administer` on an ordinary account grants nothing, and
does not fall back to `author`. Nothing is carried forward in the task row; it is
resolved again, expiry and account included, when the task runs. A sender
linked to a person (`personalTasks.enqueueFrom`) is the one way background work
starts with nobody signed in, and the link is made by an invitation only the
sender could have read.

### Sub-execution

`sandbox.run(source, { capabilities, hosts })` runs part of a script with fewer
capabilities than the script holds, as a **separate context passing only JSON**.
It is a sub-execution and not a mask over the running context for the reason
that decides it: the thing being restricted is a string of model-authored
source, and a restricted region sharing an object graph with its parent could be
handed a function by that parent and call it — a capability leak by reference no
mask catches. `capabilities` omitted means none; `hosts` omitted means wherever
the caller could already go (the opposite default, because it only subtracts
from an already-held set). `hosts` is the dimension a capability set cannot
express — `use_network` is a verb, so without it "may call the network" meant
"may call anything" — and it is checked on every redirect hop, not only the URL
the script wrote. There is no engine endpoint for it: it narrows a script from
inside itself.

### Script limits

The odd one out: a claim on shared resources rather than on authority, so it is
an administrator's call and not the owner's (an owner who could raise their own
ceiling would be back to one script setting the policy for all of them). Per
script, every field optional, absence meaning "follow the engine", values
clamped rather than refused. It can be set in both directions; lowering is the
one that gets used, to contain a script without a restart.

## Things that look similar and are not

- **Exposure by directory** says which of a script's _files_ the world can reach
  (`public/`, `resources/`). It is about files, not callers, and it is enforced
  by refusing a registration, not by a capability.
- **A per-resource callback** (`authorize` on a stream or a file route) answers
  "may this person read this _row_", which a capability cannot — a capability
  names a verb the engine understands. It runs under the requesting user's
  context and answers `{ deny }` with a 4xx of the script's choosing.
- **Host binding** (`script_hosts`, `management_hosts`) says where a script's
  registrations are published and where the management surface is served. It is
  the "where" that the realm and the token audience are checked against, not a
  narrowing of what anyone may do.
- **A deployment pin** says which revision runs, not what it may do.

## Which one do I want?

| I want to...                                                      | Use                                           |
| ----------------------------------------------------------------- | --------------------------------------------- |
| stop one account doing something                                  | its role                                      |
| keep an account to one host                                       | its realm                                     |
| give an API client only the engine's management tools on one host | the token's audience at consent               |
| make "administrator" something a person holds for ten minutes     | `[security.elevation] gated`                  |
| let an app act for someone while they are away, and no further    | a delegation, with only the scopes it needs   |
| run model-written code with read access and nothing else          | `sandbox.run` with `capabilities` and `hosts` |
| stop a runaway script holding execution slots                     | `set_script_limits`                           |
| serve a file to everyone / only to some people                    | `public/`, and `authorize` on its route       |
