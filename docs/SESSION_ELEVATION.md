# Holding Your Roles Only While You Are Using Them

> **Status: design, not built.** Nothing in this document exists yet. It is
> written in the present tense because that is how the decisions read.

```
POST /engine/write_file          →  403
{
  "error": "elevation_required",
  "capabilities": ["write_scripts"],
  "elevate": "/auth/elevate?need=write_scripts&redirect=%2Feditor"
}
```

A session carries the roles it was minted with and nothing narrows them
afterwards. Sign in as an administrator and every request for the next thirty
days — every tab, every stolen cookie, every agent turn running as you — holds
`AdministerEngine`. The role answers _may this person ever do this_. Nothing in
the engine answers _is this person doing it right now, on purpose_.

Elevation is that second answer. A session starts at the
[`UserContext::authenticated`] floor whatever roles the account holds, and the
rest is granted for minutes at a time, by a deliberate act, to a session that
has just re-authenticated.

## Why this is the missing piece and not a new idea

The engine already narrows a `UserContext` in two places and the mechanism is
the same in both:

| Layer                 | Who narrows it                       | Lifetime      | Built  |
| --------------------- | ------------------------------------ | ------------- | ------ |
| Role, in `users`      | an administrator                     | until changed | yes    |
| **Session elevation** | **the person, by re-authenticating** | **minutes**   | **no** |
| Execution attenuation | the script, `sandbox.run`            | one call      | yes    |
| Delegated run         | the person, at `/auth/delegate`      | days          | yes    |

Every one of those is [`UserContext::attenuated`], an intersection that can
only ever take away. The missing row is the one that would apply to
_everything_, because it sits on the credential rather than on an execution —
and every API in the engine starts from a credential. JavaScript, `/engine/*`,
`/mcp` and GraphQL all resolve a session into a `UserContext` before they
authorize anything. Narrow it there and "the role caps what you can do
regardless of the API" stops being a convention maintained in four places and
becomes structural.

It also closes a hole that has no other fix. A `/mcp` bearer token **is** a
session, so today the smallest credential the engine can mint carries the
holder's full authority. A script that stores one — the obvious way for an
agent to reach the engine's own management tools — is holding, through a
credential, authority that `delegation.rs` refuses to hand it directly, with
the refusal still in place and doing nothing. Once a session can hold less than
its roles, that stops being possible: there is no full-authority token to
store.

## The vocabulary

Half of [`Capability`] names what a person may do to a solution and half names
what a _script_ does while it runs. Only the first half is elevatable. The
second half — `use_network`, `read_secrets`, `read_storage`, `enqueue_tasks`,
`send_messages` — is held by the anonymous tier, because a public script
serving a signed-out visitor needs it, and gating it would break every solution
on the engine rather than protecting anything.

So the floor is exactly `authenticated_capabilities()`: read scripts and
assets, view logs, manage streams, read and write the script's own rows. An
unelevated session of an administrator holds precisely that, which is what
someone _using_ a solution needs. Nothing an ordinary request does requires
elevating, which is the property that makes this deployable at all.

What is left over is two bundles, and they are two because the engine already
asks exactly two questions:

| Bundle       | What the page says                               | Capabilities                                                                                                                  | Role required |
| ------------ | ------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------- | ------------- |
| `author`     | Create and change the scripts you own            | `write_scripts`, `delete_scripts`, `write_assets`, `delete_assets`, `delete_logs`, `manage_graphql`, `manage_script_database` | editor        |
| `administer` | Act on scripts, users and secrets you do not own | `administer_engine`                                                                                                           | administrator |

`author` on its own still cannot touch somebody else's script, because every
write pairs the capability with an ownership check (`repository::user_owns_script`).
`administer` is the marker for acting on what you do not own — which is what
`AdministerEngine` already means. The bundles are a presentation, not a type:
what is stored is a set of capability names, so a finer vocabulary later
(`manage_users` split out of `administer`, say) costs a row in this table and
nothing else.

## What is stored

`SessionData` is encrypted and kept as a JSONB blob in `sessions.data`, so
**this needs no migration**. It is the same shape `realm` arrived in: a new
field, `#[serde(default)]`, absent meaning a session that predates the feature.

```rust
/// What this session may do beyond the floor, and until when.
///
/// `None` is a session at the floor. A session that predates elevation
/// deserialises to `None`, which is the safe direction: it is refused and
/// offered the elevation page rather than silently carrying authority nobody
/// asked for.
#[serde(default)]
pub elevation: Option<Elevation>,

pub struct Elevation {
    /// Capability *names*, not the enum. A capability that is renamed or
    /// removed then drops out of an existing session instead of corrupting
    /// it — and dropping is the direction that takes authority away.
    pub capabilities: Vec<String>,
    pub granted_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    /// How the person proved they were there. For the audit line.
    pub method: ElevationMethod, // Password | Provider | Consent
}
```

`Capability::as_str` / `parse` / `all` already exist for `sandbox.run`, so the
wire format is free.

There is no elevations table. An elevation lives and dies inside the session,
which means `security::delete_sessions_for_user` — the statement that already
runs when roles change, a realm narrows, an account is deleted or a password is
changed — revokes every elevation with no new code and no second thing to
remember.

## Where it hooks

Today a session becomes a `UserContext` in four places, all spelling the same
three-armed match:

- `engine_api.rs:74` — `user_context_from`, for `/engine/*` and the native MCP tools
- `lib.rs:657` — `create_user_context_from_session`, for `/mcp`
- `lib.rs:4060` — inline, for a script serving a request
- `auth/js_api.rs:71` — `JsAuthContext::to_user_context`

All four collapse into one:

```rust
impl UserContext {
    /// The context this session is entitled to right now.
    ///
    /// The tier is the ceiling and the elevation is the grant, and the whole
    /// of the composition is an intersection — so an elevation naming a
    /// capability the account's roles do not carry adds nothing. The ceiling
    /// stays in the repository, where an administrator put it; the elevation
    /// only ever says how much of it is switched on.
    pub fn for_session(session: &SessionData, now: DateTime<Utc>) -> Self {
        let tier = match (session.is_admin, session.is_editor) {
            (true, _) => Self::admin(session.user_id.clone()),
            (_, true) => Self::editor(session.user_id.clone()),
            _ => Self::authenticated(session.user_id.clone()),
        };

        if !elevation_enabled() {
            return tier; // Today's behaviour, byte for byte.
        }

        let mut keep = Self::authenticated_capabilities();
        if let Some(live) = session.elevation.as_ref().filter(|e| e.expires_at > now) {
            keep.extend(live.capabilities.iter().filter_map(|n| Capability::parse(n)));
        }
        tier.attenuated(keep)
    }
}
```

Two consequences worth stating out loud.

**Elevation is applied once, where the credential becomes a context, and
nothing downstream can raise it.** Everything below this line — `sandbox.run`,
`delegation::context_for`, a dispatched message running under the sender's
context — only has `attenuated`, which intersects. There is no call anywhere in
the engine that widens a `UserContext`, and this design does not add one.

**A delegated run is unaffected and stays that way.** `delegation::context_for`
builds from `UserContext::authenticated` rather than from the caller's session,
so background work never sees an elevation. That is already the rule — "a
delegated task must not be the way to obtain a context that can author a
solution" — and it means an agent working while you are away cannot inherit the
half hour you spent elevated.

## Where it is checked

The same two places `realm` is checked, for the same reason:
`validate_session` and `refresh_session`. A session close enough to expiry to
be renewed would otherwise be the one path that skipped the check.

Both do the same small thing: if the elevation has expired, drop it and
re-encrypt. Dropping an elevation never expires the _session_ — the person
stays signed in, they just fall back to the floor.

**The elevation window is absolute, not sliding.** The session's own expiry
slides on use, deliberately, so that somebody working does not get signed out.
Sliding the elevation would be the same idea and the wrong one: an agent
looping every thirty seconds would hold an administrator's authority
indefinitely, which is exactly the thing this exists to stop. A person who is
still working re-elevates, and the cost of that is one click and one password.

## The refusal is a challenge

This is the half that keeps security out of scripts. A capability refusal today
is `SecurityError::InsufficientCapabilities` flattened into a message string,
and a string cannot carry the difference between _you may never do this_ and
_you may do this after re-authenticating_. That difference is the whole
feature, so it needs its own variant:

```rust
AppError::ElevationRequired {
    capabilities: Vec<Capability>,
    elevate: String,   // "/auth/elevate?need=...&redirect=..."
}
```

Three surfaces render it:

**HTTP.** A navigation (`Sec-Fetch-Mode: navigate` with `Accept: text/html`)
gets a 303 to the elevation page — a person clicking a link should land on the
thing that fixes it. Anything else gets the 403 JSON at the top of this
document, because an editor UI's `fetch` needs to decide for itself whether to
interrupt what the person was doing. Both carry
`WWW-Authenticate: Elevation capabilities="write_scripts", elevate="..."`,
which is the shape `mcp_middleware.rs` already answers a missing bearer token
with.

**MCP.** A JSON-RPC error whose `data` carries the same three fields. An MCP
client cannot browse, so what it does with the URL is show it to the person —
which is precisely how the agent already handles a lapsed delegation grant.

**JavaScript.** The thrown `Error` gains `.capabilities` and `.elevateUrl`. A
script does not implement anything; it propagates a link. That is the same
division as `personalStorage`: the engine owns the mechanism, the script owns
the wording.

## Proving the person is there

Clicking a button with the cookie you already have stops a confused script and
a CSRF. It does not stop a stolen session, which is most of what a thirty-day
window is worth to an attacker. So elevating requires a _recent_
authentication, recorded on the session:

```rust
#[serde(default)]
pub reauthenticated_at: Option<DateTime<Utc>>,
```

Set at sign-in, and again by either re-auth path. `POST /auth/elevate` refuses
unless it is within `security.elevation.reauth_window_secs` (default: 120).
Two ways to fill it:

- **A local account** types its password on the elevation form.
  `auth::local::verify_password` is already there, and a wrong guess spends
  `RateLimitKey::LoginFailure` — the same budget as a sign-in, because it is
  the same guess.
- **A federated account** has no password here, so the honest equivalent is
  bouncing to the provider with `prompt=login` (`max_age=0` for a provider that
  prefers it). `get_authorization_url` takes `state`, `nonce`, `code_challenge`
  and `resource` today; this adds a fifth argument. `extra_params` cannot do
  it — that is per-provider configuration, and this is per-request.

The bounce mints a new session, which is fine: the new one is built from the
repository's roles, carries `reauthenticated_at = now`, and the post-login
redirect lands back on `/auth/elevate?need=...`, where one button finishes the
job. Carrying the pending elevation through `oauth_state::PendingLogin` would
save that click and is not worth a new field in the cookie that binds a login
to a browser.

## The page

`GET /auth/elevate?need=<names>&redirect=<path>` is `delegate_page` with the
nouns changed, and should be written by copying it:

- the bundles as checkboxes, pre-ticked from `need`, each labelled with a
  sentence written for the person deciding rather than for the developer asking
  — the rule `Scope::describe` already follows;
- a bundle the account's roles cannot reach is shown disabled with the reason,
  not hidden, because "you are not an administrator" is the useful answer;
- a duration select (15 / 30 / 60 minutes), bounded by
  `security.elevation.max_minutes`;
- the password field, or the re-authenticate button, depending on the account;
- `redirect` through `safe_redirect_target`, which already refuses anything
  that is not a same-host absolute path;
- a CSRF token bound to the user (`CsrfProtection::validate_token_for`),
  because this form is a grant of authority and an unbound token is one
  anybody can fetch from `/auth/login` with no browser and no account.

`POST /auth/elevate/drop` ends an elevation early. It is the "exit sudo" that
makes the whole thing feel like a decision rather than a tax, and it costs one
statement.

`/auth/account` grows a third panel beside sessions and delegations, showing
what is elevated and for how much longer, with that drop button. The three
belong together: an elevated session, a delegated script and a live session are
the same question — _what is currently able to act as me_ — asked at three
lifetimes.

## Tokens, and the column that finally does something

`oauth_client_grants.scope` is recorded at consent and gates nothing. It is the
right carrier for a token's elevation:

- `/auth/oauth2/authorize` reads `scope=author`, the consent page names it in
  the same words the elevation page uses, and the minted session carries that
  elevation.
- The consent screen **is** the re-authentication: the person was there, and
  the authorization endpoint already refuses to widen a request beyond the
  stored grant.
- A token's elevation lasts as long as the token. Making an agent re-elevate
  every half hour would make this unusable, and the bound is the access
  token's own short life plus rotating refresh.
- **Refresh re-reads the grant.** `auth::refresh_tokens` already mints a fresh
  session on each refresh, re-reading roles and realm so that a revocation
  which happened in between takes effect rather than being copied forward. The
  grant is the fourth thing to re-read, and a withdrawn one stops elevating.

This is what makes an agent holding engine management tools a bounded thing
rather than an unbounded one: the person consents to `author`, the token
carries `author`, and no arrangement of that token reaches `administer`.

## Turning it on without breaking the deployment

Every existing editor session is unelevated the moment this ships, and every
`/engine/*` write from the current editor UI starts returning 403. So:

```toml
[security.elevation]
enabled = false          # engine behaves exactly as it does today
gated = ["administer"]   # which bundles require elevating, once enabled
max_minutes = 60
reauth_window_secs = 120
```

`enabled = false` by default, and the code default is `false` rather than the
template's value — the rule `[auth.internal]` follows, so an engine whose
config predates the field behaves as though the feature were absent.

`gated` is what makes the rollout gradual: start with `administer` alone, so
only the genuinely dangerous half moves, and add `author` once the editor UI
handles the challenge. The client work is one `fetch` wrapper that recognises
`elevation_required`, opens `elevate`, and retries — written once in
`aiwebengine-dev` and used everywhere.

Existing sessions self-heal: the field defaults to `None`, the first refused
write offers the page, and the person elevates.

## Tests worth writing

In the house style — one test per property, each failing if the piece is
removed:

- an unelevated editor session is refused `write_file`, and the refusal names
  `write_scripts` and carries an elevation URL;
- the ceiling holds: an account with no editor role elevating to `author` gets
  nothing, because the composition is an intersection;
- an elevation expires without expiring the session — the next request is at
  the floor and still signed in;
- an elevation does not slide: a session used continuously for twice the
  window is at the floor at the end of it;
- `refresh_session` drops an expired elevation, the way it re-checks the realm;
- `delete_sessions_for_user` — reached through a role change — leaves no live
  elevation;
- a delegated task built by `delegation::context_for` holds no elevation, even
  when the session that enqueued it was elevated;
- a token minted with `scope=author` reaches an owned script's `write_file`
  over `/mcp`, and the same token reaches nothing on a script it does not own;
- a refresh after the grant is withdrawn mints a session at the floor;
- elevating twice does not widen: `attenuated` is order-independent, and the
  test that says so is the one that keeps it that way.

## What this does not fix

**It bounds the window, not the act.** An agent that runs forty turns inside a
thirty-minute elevation gets forty chances to act on injected text. Step-up and
per-action approval are complementary: elevation decides _how long_ authority
is switched on, approval decides _which_ actions it is spent on. The agent's
plan mode is already the second half, and it runs in the person's own request —
which is exactly where an elevation lives.

**It does not help against a compromised live page.** An elevated session in a
tab with XSS is an elevated session. What it takes away is the thirty-day tail:
a cookie stolen on Monday is worth nothing on Tuesday.

**It says nothing about which scripts.** A natural extension, and deliberately
not v1: an elevation could carry targets as well as verbs —
`author` over `["shop.ts"]` rather than over everything the person owns. That
is the second dimension `NetworkScope` is to `use_network`, and the argument
for it is the same. Ownership already narrows `author` to the person's own
scripts, which is enough to ship.

## Reading order for anyone implementing this

1. `src/security/capabilities.rs` — the tiers, `attenuated`, `Capability::parse`
2. `src/security/session.rs` — `SessionData`, `validate_session`, `refresh_session`, `delete_sessions_for_user`
3. `src/auth/routes.rs` — `delegate_page` and `delegate_route`, which this is a copy of
4. `src/engine_api.rs:74` and `src/lib.rs:657` — two of the four sites that collapse
5. `docs/CAPABILITY_ATTENUATION.md` and `docs/DELEGATION.md` — the two narrowings that already exist
