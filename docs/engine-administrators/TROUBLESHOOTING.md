# Troubleshooting

Problems specific to this engine. Configuration keys are described in
`config.toml`; deployment shapes in `DEPLOYMENT.md`.

## The engine will not start

Check a configuration without starting the server:

```bash
aiwebengine --validate-config        # or: cargo run -- --validate-config
```

- **A required key is missing.** With authentication enabled the engine refuses
  to start without `APP_AUTH__JWT_SECRET` (at least 32 characters),
  `APP_SECURITY__CSRF_KEY`, `APP_SECURITY__SESSION_ENCRYPTION_KEY` and
  `APP_SECURITY__SECRET_ENCRYPTION_KEY`. Generate each with
  `openssl rand -base64 32`.
- **A literal `${...}` reached the engine.** `config.toml` does not expand
  placeholders. Set the value as an `APP_` environment variable instead.
- **A list did not parse.** A list in an environment variable is a JSON array in
  one variable: `APP_SERVER__MANAGEMENT_HOSTS='["manage.example.com"]'`.
- **A setting is empty when it should be the default.** A variable that is set,
  even to an empty string, replaces the file's value. Leave it out of the env
  file instead.

## Sign-in

- **`redirect_uri_mismatch`.** The provider has no entry for the callback this
  host sent. Register `https://<host>/auth/callback/<provider>` for every host
  that serves a login (`server.base_url` and `server.additional_base_urls`),
  matching scheme and port exactly.
- **Signed in, but the session does not stick over plain HTTP.**
  `auth.cookie.secure` defaults to true, and a browser drops a `Secure` cookie
  over HTTP. A local plain-HTTP install sets `APP_AUTH__COOKIE__SECURE=false`.
- **Signed in, but not an administrator.** The verified address must appear in
  `auth.bootstrap_admins` (case-insensitive). It is applied at sign-in, so sign
  out and in again after changing it. Without a running server:
  `aiwebengine --grant-role <account> administrator`.
- **Signed in on one host, refused on another.** An account is a principal on
  the host it signed up on (its realm). An administrator widens it with
  `set_user_realm`.
- **`/engine/*` answers 404.** The host is not in `server.management_hosts`.

## Behind a proxy

Rate limits and session fingerprints key on the client address, which comes
from `X-Forwarded-For` only when the connection arrives from
`server.trusted_proxies`. If every request appears to come from one address,
add the proxy's network there. The containerised deployments set the Docker
bridge range.

## Browser CORS errors

CORS applies to engine-owned paths only (`/engine`, `/auth`, `/mcp`). List the
calling origin in `security.cors_allowed_origins`
(`APP_SECURITY__CORS_ALLOWED_ORIGINS='["https://app.example.com"]'`). A `"*"`
entry allows unauthenticated reads only. A script's own routes set their own
headers.

## Scripts

- **A handler times out.** `javascript.execution_timeout_ms` bounds a request,
  `init_timeout_ms` an `init()`, and `job_timeout_ms` scheduled and background
  work. One script can be given more or less than those with
  `set_script_limits` — see `docs/SCRIPT_LIMITS.md`.
- **A script's log is empty.** Lines are pruned by count and age under `[logs]`.
  Read them with `read_logs` as the script's owner or an administrator.

## Debug logging

```bash
APP_LOGGING__LEVEL=debug RUST_BACKTRACE=1 aiwebengine
```
