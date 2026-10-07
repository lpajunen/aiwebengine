# Holding Your Roles Only While You Are Using Them

```
POST /engine/write_file          →  403
{
  "error": { "code": "Forbidden",
             "message": "Insufficient capabilities: required write_scripts",
             "context": {
               "required_capabilities": ["write_scripts"],
               "elevate": "/auth/elevate?need=write_scripts&redirect=%2Feditor"
             } }
}
```

A role answers _may this person ever do this_. Elevation answers _is this person
doing it right now, on purpose_. With elevation on, a session starts at the
`UserContext::authenticated` floor whatever roles the account holds, and the
rest is switched on for minutes at a time, by a deliberate act, by a session
that has just re-authenticated. A stolen cookie, a stray tab or an agent turn
running as you then holds what an ordinary user holds.

It is off by default: `security.elevation.gated` is empty, and an engine gating
nothing behaves as though elevation did not exist.

## Where it sits

Elevation is one of the narrowings in `docs/WHAT_NARROWS_WHAT.md`, and like
the others it is `UserContext::attenuated` — an intersection that only takes
away. It differs in sitting on the _credential_ rather than on an execution, so
it applies to everything: JavaScript, `/engine/*`, `/mcp` and script routes all
build their context through `UserContext::for_session`, where the account's
tier is the ceiling and the live elevation is the grant.

Two consequences:

- **Nothing downstream can raise it.** `sandbox.run`, `delegation::context_for`
  and every other layer below only intersect.
- **A delegated run never sees an elevation.** `delegation::context_for` builds
  from the grant, not the caller's session, so an agent working while you are
  away cannot inherit the half hour you spent elevated.

## The vocabulary

Only the half of `Capability` that changes a solution is elevatable. The half
naming what a script does while it runs — `use_network`, `read_secrets`,
`read_storage`, `enqueue_tasks` — is held by the anonymous tier, because a
public script serving a signed-out visitor needs it. So the floor is
`authenticated_capabilities()`: read scripts and files, view logs, manage
streams, read and write a script's own rows. Nothing an ordinary request does
requires elevating.

What is left is two bundles:

| Bundle       | What the page says                               | Capabilities                                                                                                              | Role required |
| ------------ | ------------------------------------------------ | ------------------------------------------------------------------------------------------------------------------------- | ------------- |
| `author`     | Create and change the scripts you own            | `write_scripts`, `delete_scripts`, `write_assets`, `delete_assets`, `delete_logs`, `manage_mcp`, `manage_script_database` | editor        |
| `administer` | Act on scripts, users and secrets you do not own | `administer_engine`                                                                                                       | administrator |

`author` alone still cannot touch somebody else's script, because every write
pairs the capability with an ownership check. The bundles are a presentation:
what a session stores is capability names, so a renamed or removed capability
drops out of an existing session rather than corrupting it.

## Configuration

```toml
[security.elevation]
gated = ["administer"]   # bundles that need elevating; empty means off
max_minutes = 60         # the longest elevation the page offers
reauth_window_secs = 120 # how recent a sign-in must be to elevate
```

`gated` makes it gradual: `administer` alone moves only the dangerous half;
add `author` once the clients in use handle the challenge.

## What is stored

The elevation lives in the encrypted `SessionData` (`security::elevation::Elevation`):
capability names, when it was granted and expires, and how the person proved
they were there. There is no elevations table, so
`security::delete_sessions_for_user` — run whenever roles, realm, password or
the account change — ends every elevation too.

`validate_session` and `refresh_session` drop an expired elevation, the same two
places the realm is checked. Dropping an elevation does not end the session:
the person stays signed in at the floor.

**The window is absolute, not sliding.** A session's own expiry slides on use
so somebody working is not signed out; sliding the elevation would let an agent
looping every thirty seconds hold an administrator's authority indefinitely.

## The refusal is a challenge

A capability refusal carries `required_capabilities`, and — when the engine
gates something and a gated bundle supplies a missing capability — an
`elevate` URL. Otherwise it names what is missing and stops, because pointing at
a page that cannot change the answer is worse than not pointing anywhere. A
script or client propagates the link; it implements nothing.

## Proving the person is there

Clicking a button with the cookie you already have stops a confused script and
a CSRF, not a stolen session. So `POST /auth/elevate` requires
`reauthenticated_at` (set at sign-in and by re-authentication) within
`reauth_window_secs`:

- **A local account** types its password on the elevation form. A wrong guess
  spends `RateLimitKey::LoginFailure`, the same budget as a sign-in.
- **A federated account** is sent to its provider with `prompt=login`, which
  mints a fresh session and lands back on the elevation page. Sign in with Apple
  documents no `prompt`, so an Apple account cannot step up and the page says
  so.

## The page

`GET /auth/elevate?need=<names>&redirect=<path>` shows the bundles as
checkboxes pre-ticked from `need`, a bundle the account cannot reach shown
disabled with the reason, a duration up to `max_minutes`, and the password
field or re-authenticate button. `redirect` goes through `safe_redirect_target`,
and the form's CSRF token is bound to the user, because the form grants
authority. `POST /auth/elevate/drop` ends an elevation early.

`/auth/account` shows what is elevated and for how long, beside sessions and
delegations — the same question, _what can act as me right now_, at three
lifetimes.

## Tokens

An OAuth token's elevation comes from the scope its consent named:
`/auth/oauth2/authorize` reads `scope=author` (or `administer`), the consent
page names it in the elevation page's words, and the consent is the
re-authentication. A token's elevation lasts as long as the token, bounded by
the access token's short life and rotating refresh — and a refresh re-reads
the grant, so a withdrawn one stops elevating. A person who consents to
`author` gives an agent a token that no arrangement can turn into
`administer`.

## What this does not fix

**It bounds the window, not the act.** An agent running forty turns inside a
thirty-minute elevation gets forty chances to act on injected text. Per-action
approval — the agent's plan mode — is the complementary half.

**It does not help against a compromised live page.** An elevated session in a
tab with XSS is an elevated session. What it removes is the long tail: a cookie
stolen on Monday is worth little on Tuesday.

**It says nothing about which scripts.** An elevation carries verbs, not
targets; ownership already narrows `author` to the person's own scripts.
