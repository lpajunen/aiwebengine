# Acting For Someone Who Is Not There

```javascript
function scheduleDigest(context) {
  const auth = personalTasks.authorization();
  if (!auth.granted) {
    return { status: 302, headers: { Location: auth.consentUrl } };
  }
  personalTasks.enqueue({ handler: "sendDigest", payload: {} });
  return { status: 202, body: "Scheduled" };
}
```

`scriptTasks` runs work in script context: it holds what the script holds. So
`personalStorage` throws, and a `{{secret:...}}` resolves the script's key
rather than anybody's. That is right for work belonging to the solution, and
useless for work belonging to a person.

Which is why an agent could previously only act while its owner's tab was open.
A scheduled handler runs as `UserContext::admin("scheduler")`, so `fetch`
looking up a user secret searched under a user literally named `scheduler`,
missed, and fell back to the script-wide one. Anything needing a person's
credential or a person's storage had to run inside that person's request.

## Why this is a grant and not a setting

The fix is not to widen the background context. Background work holding
somebody's API key is a real grant, and a grant has to answer four questions.

**What.** A fixed vocabulary of two nouns and a verb. Each names something the
engine actually gates on, at the surface it names:

| Scope              | Kind | Reaches                                                            |
| ------------------ | ---- | ------------------------------------------------------------------ |
| `personal_storage` | noun | `personalStorage` for that person                                  |
| `secrets`          | noun | their key in a `{{secret:...}}` header, and `secretStorage.exists` |
| `write`            | verb | changing anything at all                                           |

The nouns say **whose things** are in scope. The verb says **what may be done
with them**, and until capability attenuation existed there was nowhere to
enforce it, so every grant was a grant to change as well as to read.

A scope not ticked is not granted. A grant covering only `personal_storage`
reaches that person's storage and _not_ their secrets, and the refusal is the
one a script already handles — the same answer it gets when nobody is signed
in, rather than a new failure mode.

### Reading is the floor; `write` is the grant

Without `write`, a delegated run holds no write capability at all: not the
script's tables, not either storage, not the queue, not the message
dispatcher. That is the **plan approved in advance** — a person authorising an
app to go away and work out what to do, without authorising it to do the
thing.

It is enforced as a capability rather than as a scope check, which means it is
the same gate every other caller meets, underneath the JavaScript, at every
write there is. A handler cannot arrange its way past it.

What a read-only delegation keeps is everything it needs to be worth
consenting to:

- **reading** — the script, its assets, its tables, both stores;
- **the network**, because a delegated run that cannot call out cannot check a
  feed or ask a model, which is most of what people delegate;
- **streams**, because a background run that could not report back would be
  mute, and a progress message changes nothing.

And what it loses beyond the obvious writes is worth naming: **the queue**.
Work longer than one budget is a chain of tasks, so a read-only delegation
cannot be a long one. Neither queueing nor dispatching can actually escalate —
a queued personal task re-resolves this same grant, and a listener runs under
the sending context — but a person who ticked nothing except "read" would not
expect the app to have started anything, and that is the reading the checkbox
has to keep.

The two kinds of scope are enforced in two different places on purpose. A noun
is checked where the person's thing is reached, and answers as though that
thing were not there. The verb is a capability and refuses at the gate.
Refusals therefore arrive in whichever way that API already reports failure:
`scriptTasks` throws, `database` answers `{ error }`, the stores raise a
`DOMException`. Each names the capability that was missing.

### Grants made before the verb existed

A migration adds `write` to every grant that predates it, which is not a
widening. The consent page those people saw offered "Read and change the data
this app keeps for you", so changing is what they agreed to. Leaving them
alone would have narrowed live delegations to something nobody chose, and a
background job that quietly stops writing is the worst way to learn about a
vocabulary change. From here the page asks the question separately.

**Changing a credential is refused outright**, whatever was granted. The page
offers to let an app _use_ the keys you have given it, and storing, replacing
or deleting one is not using it: background work that could rotate or delete
somebody's key while they are away would be doing something nobody was asked
about. So `secretStorage.setSecret`, `removeSecret` and `clear` are inert in a
delegated execution, and there is no checkbox that turns them on.

**For which script.** Per `(person, script)`. Authorising one solution to act
for you says nothing about another, and the tests pin that.

**For how long.** There is no "forever" to choose. A delegation that outlives
the tab that created it by an unbounded amount is the thing nobody consented
to, so the expiry is mandatory and capped at 90 days.

**How they withdraw it.** On `/auth/account`, beside their sessions — because
a background job acting as you is the same question as a session acting as
you, and people should look in one place. Withdrawing cancels whatever it had
queued.

This is shaped after `oauth_client_grants`, which answers the same question for
an OAuth client.

None of this narrows an ordinary request. A person acting for themselves _is_
the person, so there is no grant to hold them to — the scopes only ever take
things away, and only from work running while they are absent.

## Re-read, never carried forward

A task records _who_ it runs as and nothing else. Every capability it gets is
worked out when it runs: the grant is looked up again, its expiry checked
again, the account read again.

That is the argument `auth::refresh_tokens` already makes for minting a fresh
session on every refresh — a revocation that happened in between has to take
effect rather than being copied forward. A task queued this morning and run
this evening must not be the one place a withdrawn grant still holds.

A task whose grant has gone is **abandoned, not retried**. Retrying would be
the engine repeatedly asking to act as somebody who has said no, and the
attempts would only delay the row reaching the state that explains itself.

## Capped, whatever the person holds

A delegated task runs with what an ordinary request of that person's has and
nothing more. An administrator's delegated task holds no `AdministerEngine`,
no `WriteScripts`, no `DeleteScripts`.

Background work has no business authoring a solution, and a delegated task
must not become the way to get one that does. Narrowing is the safe direction
and it is one sentence to state, which is why it is stated rather than
computed from roles.

## Three ways a delegation ends

- The person withdraws it on their account page.
- It expires.
- Their access changes. `security::delete_sessions_for_user` — which runs when
  roles change, a realm narrows, or an account is deleted — withdraws every
  delegation too. Background work is the third way something goes on being
  somebody after their access has changed, and it is the one a session list
  cannot show, so "signed out everywhere" has to include it.

## What the person sees

`/auth/delegate?script=<uri>` states what is being asked for in their terms,
with each scope as a checkbox and a duration to choose. Ticking nothing is a
refusal, not a grant of nothing — it withdraws any existing grant instead of
recording an empty one that would still say this script may act for them.

Consenting again **replaces** rather than merges. The page shows what is being
asked and the person agrees to that; a union with an older grant would mean
agreeing to something that was not on the page.

The form's CSRF token is bound to the user (`validate_token_for`), because an
unbound one is something anybody can fetch with no browser and no account —
and this form is a grant of authority.
