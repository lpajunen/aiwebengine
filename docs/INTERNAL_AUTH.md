# Internal authentication

Accounts this engine authenticates itself, with no third-party provider behind
them. Two kinds share the machinery:

- a **guest** has no credential. The engine mints an identity and issues a
  session, so someone using a solution gets stable storage, ownership and a
  display name while surrendering nothing. They cannot sign in again from
  another browser — the honest cost of holding no secret.
- a **local account** has a username and an Argon2id password hash stored in
  the `local_credentials` table.

A guest becomes a local account by attaching a credential to the same
`user_id`, which is why the credential lives in its own table rather than in a
column on `users`. Whatever the guest accumulated is still theirs afterwards.

## Why

Two problems, one mechanism.

**Solutions have users who are not developers.** Requiring a Google, Microsoft
or Apple account to play a game or use a tool means collecting verified
real-world identities the solution has no use for, and players say so. A guest
or a username is enough to own a character.

**A personal install cannot rely on an external IdP.** Google and Microsoft
both permit `http://localhost` redirect URIs, but each user would have to
register their own OAuth client, and Apple does not permit localhost at all. An
internal credential means a laptop instance has a real logged-in owner without
anyone creating a cloud project.

The same mechanism gives a server deployment break-glass access when the
external provider is down or misconfigured — otherwise a bad OAuth config locks
you out of your own engine.

## What these accounts can do

Nothing an ordinary user of a solution cannot do. Internal identities are
created with the `Authenticated` role and no other; reaching editor or
administrator takes an administrator granting the role. `auth.bootstrap_admins`
matches on email address, and these accounts have none, so no self-registered
account can arrive as an administrator.

See the security sandbox model in `CLAUDE.md` for what each tier holds.

## Configuration

```toml
[auth.internal]
# Accept username-and-password logins against credentials stored here.
enabled = true
# Let anyone create an account.
allow_registration = true
# Let a caller with no credential be issued an identity and a session.
allow_guests = true
# Minimum password length. The engine enforces a floor of 8 regardless.
min_password_length = 12
```

All three shipped templates — local, staging and production — turn these on,
because a solution whose users cannot sign themselves up has no use for
internal credentials.

**The code defaults are the opposite.** Every field defaults to `false`, so an
engine whose configuration says nothing about `[auth.internal]` gets nothing:
no forms on the sign-in page, and every endpoint refusing with 403. That is
deliberate — an upgrade must not switch on a password endpoint for a
deployment that never asked — but it is also the thing that surprises people.
A `config.toml` copied from a template before these fields existed has no
`[auth.internal]` section, and so behaves as though everything were off. Copy
the block above into it.

What `allow_registration` is worth is bounded by two things worth knowing
before enabling it on a public host. A self-registered account lands in the
authenticated tier, which holds nothing that touches scripts, assets or
schema. And it is recorded in the realm of the host it was created on, so an
account made through a solution's sign-up form is not a principal on a
management host.

## The sign-in page

`/auth/login` shows what is enabled: a username-and-password form when
`enabled`, a link to a sign-up form when `allow_registration`, a "Continue as
guest" button when `allow_guests`, and the configured OAuth providers below a
divider. With nothing internal enabled it is the provider list it always was.

Plain HTML forms, no script — the engine's configured
`security.content_security_policy` names `script-src 'self'` with no inline
allowance, and a sign-in page is the last place to depend on that being
relaxed.

A solution normally builds its own sign-in UI and posts JSON. This page is what
makes a personal install usable, and what an administrator uses for break-glass
access.

## Endpoints

Each takes JSON or an HTML form, and answers in kind: a JSON body gets JSON, a
form submission gets a redirect — to the `redirect` field on success, or back
to `/auth/login?error=<code>` on failure, where the page renders a message
chosen from a fixed table. Nothing a caller supplies is echoed onto the page.

Form submissions must carry a `csrf_token` from the login page. JSON bodies
must not and need not: a cross-site page can POST a form anywhere without a
preflight, which is how login CSRF signs a victim into an attacker's account,
but it cannot POST `application/json` without one the engine does not grant.

| Endpoint | Requires | Does |
| --- | --- | --- |
| `POST /auth/guest` | `allow_guests` | Issues an identity and a session to a caller with no credential. Body: `{"name": "..."}` (optional). |
| `POST /auth/local/register` | `enabled` and `allow_registration` | Creates an account and issues a session. Body: `{"username", "password", "name"}`. |
| `POST /auth/local/login` | `enabled` | Signs in against a stored credential. Body: `{"username", "password"}`. |
| `POST /auth/local/claim` | `enabled`, plus a session | Attaches a credential to the calling account, keeping its `user_id`. Body: `{"username", "password"}`. |

Each of the first three sets the session cookie on success.

## Rules worth knowing

- **Usernames are case-insensitive** and stored folded to lower case. Two
  accounts differing only in capitalisation are two accounts a person cannot
  tell apart, which is a phishing tool rather than a feature.
- **Usernames are 3–32 characters**, must start with a letter or digit, and may
  contain only ASCII letters, digits, `_`, `.` and `-`. The narrow set is
  deliberate: a username is displayed next to other people's names.
- **One credential per account.** Claiming cannot overwrite an existing
  credential — otherwise the claim path is a password reset that needs no old
  password.
- **A wrong password and an unknown username answer identically**, and the
  unknown-username branch still hashes, so the endpoint is not a way to
  enumerate accounts by response or by timing.
- **`/auth/local/claim` is POST-only** and relies on the session cookie's
  `SameSite=Lax` to refuse cross-site requests. Keep it a POST.
- **Passwords are Argon2id PHC strings.** Parameters and salt travel inside the
  hash, so raising the cost later is a code change, not a migration.
- **An account belongs to one host.** Guests and local accounts are recorded
  with the host they were created on, and authenticate only there — an account
  a solution's sign-up form mints is not a principal on your management host.
  Signing in elsewhere does not move it. An administrator can widen an account
  to every host with `/engine/user_realm` or the `set_user_realm` MCP tool; no
  sign-in path produces that value.
- **A guest or local session is not an API credential.** These flows mint
  browser sessions, which carry no audience, and `/mcp` requires one for the
  host and path being requested. Presenting a session cookie as a Bearer token
  is refused. A client that needs to reach `/mcp` goes through the OAuth2
  authorization-code flow, which issues a token with an audience.

## Not done yet

- **No claim form on the sign-in page.** The endpoint accepts a form, but the
  page offers no control for it, because a guest who wants to claim an account
  is mid-solution rather than at a sign-in screen. A solution prompts for it
  where it makes sense.
- **No password change or reset.** An account with a credential cannot rotate
  it. There is no recovery path for a forgotten password, and for a guest there
  cannot be one.
- **No passkeys.** A good fit here — no email, no password reset, no PII — and
  the credential table's shape leaves room for them.
