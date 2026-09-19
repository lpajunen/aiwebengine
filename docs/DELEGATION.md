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

**What.** A fixed vocabulary — currently "read and change the data this app
keeps for you" (`personal_storage`) and "use the API keys you have given this
app" (`secrets`). Each names something the engine actually gates on. A scope
nothing checks would be a promise to the person that nothing keeps.

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
