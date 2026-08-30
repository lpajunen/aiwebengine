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

Every switch is off by default. A password endpoint is the most-probed thing an
engine can expose, so each is turned on deliberately.

```toml
[auth.internal]
# Accept username-and-password logins against credentials stored here.
enabled = false
# Let anyone create an account. Off means accounts exist only because an
# administrator or a guest claim created them.
allow_registration = false
# Let a caller with no credential be issued an identity and a session.
allow_guests = false
# Minimum password length. The engine enforces a floor of 8 regardless.
min_password_length = 12
```

`config.local.toml` turns all three on, since that is the point of a personal
install. The staging and production templates leave them off.

## Endpoints

All take and return JSON.

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

## Not done yet

- **No browser UI.** `/auth/login` still lists only OAuth providers. A solution
  builds its own sign-in form and posts JSON; a personal install currently has
  no engine-provided page for local sign-in.
- **No password change or reset.** An account with a credential cannot rotate
  it. There is no recovery path for a forgotten password, and for a guest there
  cannot be one.
- **No realm scoping.** An account is a principal engine-wide, so an identity
  registered by a solution on one host is authenticated on every host. What
  bounds it today is that the authenticated tier holds nothing that reaches
  another script's work, plus the session cookie being host-scoped by default
  (`auth.cookie.domain` unset). Scoping identities to a realm is the next piece.
- **No passkeys.** A good fit here — no email, no password reset, no PII — and
  the credential table's shape leaves room for them.
