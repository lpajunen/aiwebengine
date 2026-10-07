# OAuth Providers and Secrets

Signing people in through an OAuth provider, making the first administrator,
and the two kinds of secret an engine holds: its own keys, and the secrets
scripts use. Configuration keys are documented where they are defined, in
`config.toml`; local accounts and guests are in `docs/INTERNAL_AUTH.md`.

## OAuth providers

A provider is configured entirely by environment variable. It registers when
its three variables are present:

```bash
APP_AUTH__PROVIDERS__GOOGLE__CLIENT_ID=...apps.googleusercontent.com
APP_AUTH__PROVIDERS__GOOGLE__CLIENT_SECRET=...
APP_AUTH__PROVIDERS__GOOGLE__REDIRECT_URI=https://example.com/auth/callback/google
```

Microsoft additionally takes `__TENANT_ID` (`common`, or your tenant); Apple
takes `__TEAM_ID`, `__KEY_ID` and `__PRIVATE_KEY` (the `.p8` file's contents).

The callback path is `/auth/callback/{provider}`. The provider compares it
exactly — scheme, host and port — so register one for every hostname that
serves a login: `server.base_url` and each of `server.additional_base_urls`. A
local `cargo run` on `http://localhost:3000` and a containerised one on
`https://local.example.com` are two entries.

### In each provider's console

- **Google** — Cloud Console → APIs & Services → Credentials → Create
  credentials → OAuth client ID → Web application. Add the callback URLs under
  _Authorized redirect URIs_.
- **Microsoft** — Azure Portal → Microsoft Entra ID → App registrations → New
  registration. Add the callback as a _Web_ redirect URI, then create a client
  secret under _Certificates & secrets_ (it is shown once).
- **Apple** — Developer Portal → Certificates, Identifiers & Profiles. Create an
  App ID with _Sign in with Apple_, a Services ID (its identifier is the client
  ID) with your domain and callback, and a key with _Sign in with Apple_ enabled.
  Download the `.p8` and note its key ID.

## The first administrator

`auth.bootstrap_admins` lists addresses that receive the administrator role,
matched case-insensitively against an address a configured provider verified:

```bash
APP_AUTH__BOOTSTRAP_ADMINS='["you@example.com"]'
```

It is applied on every sign-in, so naming an account that already exists works.
A listed account is also a principal on every host (realm `*`), so it can reach
the management host whichever host it first signed in on. Removing an address
from the list does not take the role away.

An engine without an OAuth provider uses
`auth.internal.bootstrap_admin_usernames` instead. Without a running server:

```bash
aiwebengine --grant-role <account> administrator
```

Further administrators are granted by an administrator with `add_user_role`
(`/engine/add_user_role` or the MCP tool).

## The engine's own keys

Four values have no default, and the engine refuses to start without them when
authentication is enabled. Generate each with `openssl rand -base64 32`:

| Variable                               | What it protects                                 | Rotating it                          |
| -------------------------------------- | ------------------------------------------------ | ------------------------------------ |
| `APP_SECURITY__SESSION_ENCRYPTION_KEY` | Session payloads                                 | Signs everyone out                   |
| `APP_SECURITY__CSRF_KEY`               | CSRF tokens                                      | Fails forms already open             |
| `APP_SECURITY__SECRET_ENCRYPTION_KEY`  | Script secrets, user secrets and git credentials | Makes every stored secret unreadable |
| `APP_AUTH__JWT_SECRET`                 | Checked at startup (at least 32 characters) only | No effect                            |

Every instance of one deployment must share all four. Back the environment file
up with the database dumps, and not in the same place: a dump restored without
`SECRET_ENCRYPTION_KEY` holds secrets nobody can read.

## Script secrets

A script uses an API key without its JavaScript ever seeing it. The engine
stores the value encrypted and substitutes it into an outbound `fetch`.

**Setting one.** A script's owner or an administrator calls `write_secret`
(`POST /engine/write_secret`, or the MCP tool):

```bash
curl -X POST https://manage.example.com/engine/write_secret \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"script": "my-script", "key": "anthropic_api_key", "value": "sk-ant-..."}'
```

`list_secrets` names a script's keys, `delete_secret` removes one and
`clear_secrets` removes them all. A signed-in person can also store their own
with `secretStorage.setSecret(key, value)`; theirs is found before the
script's.

**Using one.** A script checks a key with `secretStorage.exists(key)` and names
it in a `{{secret:key}}` template in a request header or the URL:

```javascript
await fetch("https://api.anthropic.com/v1/messages", {
  method: "POST",
  headers: { "x-api-key": "{{secret:anthropic_api_key}}" },
  body: JSON.stringify(request),
});
```

The template is replaced inline, so `"Bearer {{secret:token}}"` works. Bodies
are not substituted. A template that names a missing secret, or opens
`{{secret:` without closing it, fails the `fetch` rather than sending the text.
A person's own secrets are found only when the call runs as them; background and
anonymous work sees the script's.
