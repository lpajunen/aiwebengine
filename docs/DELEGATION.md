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

## Who may set it going: linked senders

A grant answers "may this app act for me". It does not answer "who gets to
decide when". For work the app starts itself — a schedule, a job it queued
during your own visit — those are the same question. For an **inbound
webhook** they are not: a Telegram or Slack message arrives with nobody
signed in, and whoever sent it chooses the moment and the payload.

That is the gap that kept an agent off every channel people actually use.
There is no session, so there is no person, so there was nothing to enqueue
against.

### A script names a sender, never a person

The obvious fix — let a script name the user id it wants to act as — is the
wrong one. Anyone who can reach a webhook could then try to start anybody's
agent, and one script that trusted the wrong field in a request body would be
a way to spend other people's money.

So the person also has to say **which sender** may trigger their work, and the
script names that sender rather than the account:

```javascript
// A Telegram webhook, after checking the secret token Telegram sends.
const from = String(update.message.from.id);

const who = personalTasks.sender({ channel: "telegram", identity: from });
if (!who.granted) {
  // Minted here, and replied with *into that chat* — see below.
  const invite = personalTasks.inviteLink({
    channel: "telegram",
    identity: from,
  });
  return reply(`Authorise me first: ${invite.linkUrl}`);
}

personalTasks.enqueueFrom({
  channel: "telegram",
  identity: from,
  handler: "runTurn",
  payload: { text: update.message.text },
});
```

The engine resolves `{channel, identity}` through the links people have
consented to. An unlinked sender resolves to nobody and nothing runs. There is
no user id anywhere in that code, which is the property worth having: a
handler that trusts the wrong field can be made to claim the wrong **sender**,
and not to name a different account, and not to enumerate one.

`sender()` deliberately never answers with the account's id either —
everything a script can do for that person goes through `enqueueFrom`, and an
id would end up in whatever the bot logs or echoes back into the chat.

### The link is a token, not a name

The consent page cannot take `?channel=telegram&identity=12345`. That URL is
one anybody can construct for anybody, so linking would be first come, first
served on a guessable string — and the harm there is not squatting but
**interception**. Bind a victim's chat id to your own account before they do,
and every message they send that bot is processed as _your_ turn, with their
text landing in _your_ storage.

So `inviteLink()` mints a single-use token, expiring in fifteen minutes, and
the URL carries only that. The sender is not in it. A script mints one in
reply to a message it actually received and sends it into that chat, which
means **reaching the consent page for a sender requires being able to read
that sender's messages** — the only evidence of ownership the engine can have,
and the one a query parameter could never carry.

Two consequences worth knowing:

- **Reply with the link into the sender's own chat, and nowhere else.**
  Posting it anywhere they cannot read gives away exactly the thing it
  proves.
- **Minting again invalidates the link already sent.** That is what somebody
  re-requesting one expects, and it bounds the table at one live row per
  sender. So call it where a bot decides to send a link, not in a loop that
  polls `sender()`.

Reading a token does not spend it — the sign-in redirect, the back button and
a reload all reach the page before anybody has agreed to anything. Consenting
spends it, in the statement that reads it, so two browsers racing on one link
cannot both bind.

Unlinking needs no token. The delete is scoped to the caller's own account, so
a made-up pair matches nothing — and requiring proof there would mean you
could only unlink a sender you could still receive messages from, which is
backwards: losing access to the chat is the commonest reason to want it.

### What the engine cannot do

**Verify the message really came from that sender.** Telegram and Slack sign
their webhooks; email largely does not. Checking that signature is the
script's job, and no amount of schema can do it — a handler that reads the
sender straight from an unauthenticated body is a way for anyone who can reach
the route to start that person's agent.

This is said here rather than papered over, because it is the one part of the
chain the engine does not hold.

### The rules around a link

**Both halves are required.** A link says which sender may trigger; the grant
says whether there is anything to trigger. `enqueueFrom` checks both, at the
moment of the call, exactly as the task worker re-checks the grant when it
runs.

**One sender is at most one person per script.** Two accounts claiming the
same Telegram id would be a question the engine cannot answer, so the second
is refused — and refused rather than allowed to replace the first, since
silently moving a binding is a takeover. With invitations in place this is a
backstop rather than the defence: reaching the page at all takes a token from
that sender's chat.

**The channel is folded, the sender is not.** `Telegram` and `telegram` are
the same place, because the slug is one the solution chose. A Slack user id is
case-sensitive and belongs to the far end, so folding it would make two
senders look like one.

**Consent is one decision.** The invitation resolves at `/auth/delegate`,
which shows "it will also be able to start work for you when _12345_ messages
it on _telegram_" beside the scopes. The person approves the app, the scopes
and the trigger together, rather than being asked a second question whose
stakes they have no way to judge.

**Triggers are budgeted.** `RateLimitKey::ChannelTrigger` — thirty, then one
every thirty seconds, per link. It is the only budget in the engine whose
spender is chosen by whoever sends the message, and each spend is a model call
somebody else pays for. Keying it by the link rather than by the account
does not stop an attacker who knows a linked sender from exhausting it — the
trigger is attacker-controllable by definition — but it keeps the damage to
that one channel.

**Unlinking and withdrawing are separate.** Somebody who changed phone number
wants the first. Both are buttons on `/auth/account`, under the app they
belong to, and withdrawing the grant unlinks every sender with it — a link
that outlived its grant would come back to life the next time that person
authorised the script for some unrelated reason.

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

## Capped by default, and raised only by being asked for

A delegated task runs with what an ordinary request of that person's has and
nothing more — unless they ticked one of the two scopes that say otherwise.
An administrator's delegated task holds no `AdministerEngine`, no
`WriteScripts` and no `DeleteScripts` on the strength of their roles alone.

Background work has no business authoring a solution _by default_, and a
delegated task must not become the way to **quietly** get one that does. That
is the sentence the cap was always keeping, and it is the one that still
holds.

### The two that raise it

`author` — _Create and change your scripts while you are away._
`administer` — _Administer this engine as you — including scripts, users and
secrets you do not own._

They exist because an agent that manages scripts and users is work people
actually ask for while they are away, and the alternative was a stored `/mcp`
token: the same authority reached through a credential, past every check in
this file rather than through one.

Three rules keep them honest.

**A grant is consent, not a promotion.** Each is granted only as far as the
account's own roles already reach. Ticking `administer` on an ordinary account
grants nothing at all, and the consent page shows it disabled with the reason
rather than offering a checkbox that would do nothing.

**Neither falls back to the other.** An editor ticking `administer` does not
get `author` instead. Handing over authoring nobody ticked, on the grounds
that it is less than what was asked for, is not how consent works.

**They are independent.** `administer` alone carries `AdministerEngine` and
not `WriteScripts` — enough to read the engine and list its users, not enough
to rewrite somebody's solution. Full administration is both, and a person who
wants an agent that can answer "who are the users" without also being able to
rewrite their scripts can say exactly that.

Roles are read when the task runs, so a demotion takes effect on the next run
rather than when the grant lapses. In practice it is faster than that: a role
change runs `delete_sessions_for_user`, which withdraws every delegation the
account had.

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
