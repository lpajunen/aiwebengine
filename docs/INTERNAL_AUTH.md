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

The exception is an account the operator named in
`auth.internal.bootstrap_admin_usernames`. That is the same declaration
`bootstrap_admins` makes, by the same authority — the configuration file — for
an engine whose accounts have no verified address, and it is how a personal
install gets an owner at all — there is no development mode to fall back on. It
is applied on every sign-in, so naming an account that already exists works;
and it is not a credential, since reaching the account still takes its password.

For the cases configuration cannot reach — an account under a different name, an
administrator locked out, the last administrator gone — there is
`aiwebengine --grant-role <username-or-email> administrator`, which needs the
database but no running server. Its counterpart is
`aiwebengine --set-password <username-or-email>`, for the account nobody
remembers the password to.

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
# Local usernames that hold the administrator role, whatever the database says.
bootstrap_admin_usernames = []
# Let an account be issued recovery codes, and let one be redeemed for a reset.
allow_recovery_codes = true
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

It also links to `/auth/account`, because the sign-in page is where someone
goes looking when they are thinking about their password. It is a link and not
a form: everything on that page needs a session, and changing a password needs
the current one, neither of which a person looking at a sign-in page has.

## The account page

`/auth/account` is what a signed-in person can do about their own way in. It
offers exactly one of two forms, because they are different acts:

- **Change your password**, when the account holds a credential. Posts to
  `/auth/local/password`, so it asks for the current one and ends every other
  session on success.
- **Add a username and password**, when it holds none. Posts to
  `/auth/local/claim` — the control that endpoint shipped without. A guest is
  told what it is for in the terms that matter to a guest: the account is gone
  when the browser is. A federated account is told the other thing it is for: a
  way in that does not depend on the provider being reachable.

Signed out, the page redirects to `/auth/login?redirect=/auth/account` and
comes back, so the link works for someone whose session has aged out. It is
served `Cache-Control: no-store`, since it names the account it belongs to.

Its forms carry a CSRF token **bound to the session** — `generate_token(Some(
user_id))`, validated with `validate_token_for`. The sign-in forms take an
unbound token, which is right for them: they are submitted before there is a
session to bind one to. It is wrong for a form that changes an account, because
an unbound token is one anybody can fetch from `/auth/login` with no browser and
no account, leaving `SameSite=Lax` as the only thing between the password form
and a cross-site POST — and a browser will carry a Lax cookie on a cross-site
POST for the first couple of minutes after it is set. `/auth/local/password`
demands a bound token; `/auth/local/claim` accepts either, because solutions
have been posting it a token from the sign-in page since it shipped.

A form whose token has expired — the page sat open long enough — comes back to
the page with `?error=csrf` and a fresh token, rather than the JSON an API
caller would get. A failed submission comes back to the page it was submitted
from. The endpoints
decide which page that is from the redirect the form carries: one landing on
`/auth/account` was submitted there, and its message belongs there rather than
on a sign-in page telling a signed-in person that their username and password
do not match.

## Endpoints

Each takes JSON or an HTML form, and answers in kind: a JSON body gets JSON, a
form submission gets a redirect — to the `redirect` field on success, or back
to `/auth/login?error=<code>` on failure, where the page renders a message
chosen from a fixed table. Nothing a caller supplies is echoed onto the page.

Form submissions must carry a `csrf_token` from the login page. JSON bodies
must not and need not: a cross-site page can POST a form anywhere without a
preflight, which is how login CSRF signs a victim into an attacker's account,
but it cannot POST `application/json` without one the engine does not grant.

| Endpoint                          | Requires                                          | Does                                                                                                                                                                         |
| --------------------------------- | ------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `POST /auth/guest`                | `allow_guests`                                    | Issues an identity and a session to a caller with no credential. Body: `{"name": "..."}` (optional).                                                                         |
| `POST /auth/local/register`       | `enabled` and `allow_registration`                | Creates an account and issues a session. Body: `{"username", "password", "name"}`.                                                                                           |
| `POST /auth/local/login`          | `enabled`                                         | Signs in against a stored credential. Body: `{"username", "password"}`.                                                                                                      |
| `POST /auth/local/claim`          | `enabled`, plus a session                         | Attaches a credential to the calling account, keeping its `user_id`. Body: `{"username", "password"}`.                                                                       |
| `POST /auth/local/password`       | `enabled`, plus a session                         | Replaces the calling account's password, given the one it has now. Ends every session the account held and issues a fresh one. Body: `{"current_password", "new_password"}`. |
| `POST /auth/local/recovery_codes` | `enabled`, `allow_recovery_codes`, plus a session | Issues a fresh set of recovery codes and answers with them, once. Replaces any set the account had. Body: `{"current_password"}`.                                            |
| `POST /auth/local/recover`        | `enabled` and `allow_recovery_codes`              | Spends a recovery code: sets a new password, ends every session the account held, issues one. Body: `{"username", "code", "new_password"}`.                                  |

`/auth/guest`, `/auth/local/register` and `/auth/local/login` set the session
cookie on success, and so do `/auth/local/password` and `/auth/local/recover` —
the session each of those issues replaces ones it has just ended.
`/auth/local/claim` does not: the account and its roles are unchanged, only the
way back in is new. `/auth/local/recovery_codes` is the exception in shape as
well as in kind — it answers with credentials in the body, so a form submission
gets a page listing them rather than a redirect, and none of it is ever put in a
URL.

`POST /auth/local/password` asks for the current password even though the caller
already holds a session: a session someone else got hold of must not be enough
to lock the owner out of their own account. Ending the other sessions is the
other half of it — a password is changed because the old one may be known, and a
session minted under it otherwise keeps working for up to `max_session_age`.

## Recovery codes

The answer to "I forgot my password" for someone who is not the operator.

A code is a second credential for the same account, issued ahead of time and
written down. It proves what a password proves, and the only thing it can do is
set a new password. Ten are issued at a time, each about ninety-eight bits, from
an alphabet with no `i`, `l`, `o` or `u` — a code is copied onto paper and typed
back months later, and a character somebody has to guess the identity of is a
code that fails when it is needed. What is compared is the code with its
decoration removed, so the hyphens the engine writes, the spaces a person types
and the case a phone capitalised all fall away.

Only hashes are stored: SHA-256, no salt and no work factor, the same reasoning
as `oauth_refresh_tokens.token_hash`. These are strings the engine generated
with far more entropy than a password, so there is nothing to guess and nothing
to pre-compute — and it means redeeming one is a single indexed lookup rather
than an Argon2 verification against every row the account holds. The engine
cannot show a code twice, which is why the page that issues a set says so.

Rules that make a code a credential rather than a password with extra steps:

- **Single use.** Spending one marks it used in the same transaction that writes
  the password, conditional on it still being unused, so two requests racing
  with the same code cannot both set one. The other nine survive: a set of ten
  is ten emergencies.
- **Bound to one account.** `/auth/local/recover` takes a username as well as a
  code and both must name the same account. An unknown username, a wrong code
  and a spent code are one answer — and one cost: the new password is hashed
  before the username is looked up, so a whole Argon2 hash is not the difference
  between "no such account" and "wrong code". That is the same oracle
  `verify_login` hashes against a dummy to close.
- **Issuing takes the current password**, even though the caller already holds a
  session — the same rule as changing a password, for the same reason. A set of
  codes is a way in that would otherwise outlive the owner noticing and changing
  their password.
- **Issuing replaces the previous set.** Reissuing because the old codes were
  seen by somebody has to actually take them away.
- **Redeeming ends every session the account had.** Recovery is what somebody
  does when they have lost control of an account, and the sessions alive at that
  moment may be the ones they are trying to be rid of.
- **Throttled per address and per account**, like `login_local`, because this is
  a second credential accepting guesses at the same account. Only failures spend
  the account's budget, and a password the engine would refuse is not counted as
  one — otherwise a person could lock themselves out of their own recovery by
  typing a short password.
- **Only for an account that holds a password.** Setting one is all a code can
  do, and for a guest — an account with no way in by design — a code would be
  one.

On the pages: `/auth/login` links to `?recover=1`, which swaps the sign-in form
for one taking a username, a code and the new password. Recovery is not a
sign-in, so the code is spent and the password chosen in the same act; an
account is never left reachable by a code already shown to work. `/auth/account`
reports how many codes are left — the only thing that can honestly be said about
codes stored as hashes — and issues a fresh set.

## Resetting a password from the command line

```bash
# Interactive: asks twice, because the password is echoed and a typo here locks
# the account it was meant to open.
aiwebengine --set-password alice

# Scripted:
printf '%s' "$NEW_PASSWORD" | aiwebengine --set-password alice
```

The new password is read from standard input rather than taken as an argument:
an argument is visible in `ps` and in shell history, and this is the one command
whose whole job is to write a credential. Only the line ending is stripped, so a
password may begin or end with a space.

`ACCOUNT` is a local username or an email address, resolved the way
`--grant-role` resolves one. The username is looked up in `local_credentials`
rather than on the `users` row, because a guest that later claimed a name still
carries `guest` as its provider.

Like `--grant-role`, it needs the database and no running server, and is
authorized by holding the configuration file and the database it points at —
the same authority `auth.bootstrap_admins` runs on. Two things follow from what
it is:

- **It ends every session the account held.** A password is reset because the
  old one may be known, and a session minted under it otherwise keeps working
  for up to `max_session_age` — thirty days by default.
- **It refuses an account with no credential.** Attaching a first one needs a
  username, and choosing somebody's username for them is not a thing a password
  reset should do. `/auth/account` is where an account without a credential gets
  one.

`min_password_length` still applies. Being the operator is not a reason to write
a password the sign-in page would then refuse.

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
  to every host with `/engine/user_realm` or the `set_user_realm` MCP tool. No
  internal sign-in path produces that value — the one thing that does is an
  address the operator listed in `auth.bootstrap_admins`, which these accounts
  cannot match because they carry no address at all.
- **A guest or local session is not an API credential.** These flows mint
  browser sessions, which carry no audience, and `/mcp` requires one for the
  host and path being requested. Presenting a session cookie as a Bearer token
  is refused. A client that needs to reach `/mcp` goes through the OAuth2
  authorization-code flow, which issues a token with an audience.

## Not done yet

- **Recovery has to be set up in advance.** A code only helps an account that
  asked for one before it needed it, and nothing prompts for that: the account
  page offers it, and a person who never visits the account page has no codes on
  the day they forget their password. What would close it is a solution being
  able to walk its own users through issuing a set at sign-up. For a guest there
  can be no recovery at all: no secret, nothing to recover.
- **No administrator-issued reset.** An account with no codes and no operator
  access has nowhere to go. An administrator can already reach `--set-password`
  on the machine, but there is no endpoint that lets them issue a reset for an
  account in their realm.
- **No session visibility.** A person cannot list the sessions their account
  holds or end one of them. Changing a password ends them all, and an
  administrator's role change ends them all, and those are the only two
  controls that exist. The data is there — `sessions.data` carries when each
  was created, when it was last used, and what it was minted against.
- **No passkeys.** A good fit here — no email, no password reset, no PII — and
  the credential table's shape leaves room for them.
