# Deployment Options

How aiwebengine is meant to be run, and what each way of running it costs and
requires. This is the map; the step-by-step instructions live in
[docs/engine-administrators/03-RUNNING-ENVIRONMENTS.md](docs/engine-administrators/03-RUNNING-ENVIRONMENTS.md),
and the settings themselves in
[02-CONFIGURATION.md](docs/engine-administrators/02-CONFIGURATION.md).

In every option below, the engine is reached the same way — over HTTP, by a
browser, an MCP client, a GraphQL client or anything else that speaks HTTP.
There is no other entry point, and no deployment mode changes the API surface.
What differs is where the database lives, what terminates TLS, and how many
engine processes there are.

## The two axes

A deployment is a point on two axes, not a name:

|                         | Database                              | TLS terminated by                | Engine processes |
| ----------------------- | ------------------------------------- | -------------------------------- | ---------------- |
| **Desktop standalone**  | embedded, in the app's data directory | nothing — plain HTTP on loopback | 1                |
| **Developer local**     | Postgres container                    | nothing, or Caddy container      | 1 or 2           |
| **Server, single node** | Postgres container, or managed        | Caddy                            | 1                |
| **Server, clustered**   | Postgres container, or managed        | Caddy                            | 2+               |

Everything else — the JavaScript API, the capability model, `/engine/*`, MCP,
GraphQL, scheduled jobs, revisions and deployments — is identical across all
four. That is the property worth protecting: a solution developed against a
desktop install must run unchanged on a cluster.

## What every deployment needs

Independent of topology:

- **A PostgreSQL database.** It is the only storage backend. Scripts, assets,
  users, sessions, secrets, logs, revisions and deployment pins all live there;
  there is no file-backed or in-memory mode.
- **Four secrets**, all of which must be generated per installation:
  `auth.jwt_secret`, `security.csrf_key`, `security.session_encryption_key`,
  `security.secret_encryption_key`. The values shipped in `config.local.toml`
  are published in this repository and are for a local install only.
  `secret_encryption_key` encrypts script and user secrets at rest, so **a
  database backup without that key is a backup you cannot fully restore.** Back
  the keys up with the dump, separately from it.
- **A route to the first administrator.** Either `auth.bootstrap_admins` (an
  address a configured OAuth provider verifies),
  `auth.internal.bootstrap_admin_usernames` (a local account), or the
  no-server-needed `aiwebengine --grant-role <account> administrator`. An engine
  with no administrator can only be fixed from the machine holding the database.
- **A reverse proxy, for anything not on loopback.** The engine serves plain
  HTTP and has no TLS listener at all (`axum_server::Server::bind`, no
  acceptor). HTTPS is Caddy's job in every server topology.
- **`server.trusted_proxies` matching reality.** Set it to the proxy's address
  or network when a proxy is in front, and leave it empty when nothing is. It
  decides whether `X-Forwarded-For` is believed, and every rate-limit bucket,
  session fingerprint and audit line reads the address it establishes.

## Desktop standalone

**One user, one machine, no external dependencies.** The engine binary, its data
directory, and a database that starts and stops with the app. Bound to
`127.0.0.1`, plain HTTP, no proxy, no certificates, no DNS.

What it looks like in configuration:

- `server.host = "127.0.0.1"` — loopback only, since nothing authenticates at
  the network edge.
- `auth.cookie.secure = false`, which the engine already handles: the `__Host-`
  prefix is dropped when the cookie is not `Secure`, so sign-in works over
  plain HTTP (`auth::host_scoped_cookie_name`).
- `server.management_hosts = []` and no `additional_base_urls` — there is one
  host, and it is the management host.
- `[auth.internal]` with `enabled`, `allow_guests` and
  `bootstrap_admin_usernames = ["<owner>"]`. This is the mode internal auth was
  built for: a desktop install has no public redirect URI, so no OAuth provider
  can be configured, and `bootstrap_admins` — which matches a
  provider-verified address — can never name anybody. See
  [docs/INTERNAL_AUTH.md](docs/INTERNAL_AUTH.md).
- Small budgets: `javascript.max_concurrent_executions` and
  `repository.max_connections` in the single digits, `max_memory_bytes` modest.
  The cluster defaults size for hundreds of concurrent callers.
- `security.cors_allowed_origins = []`.

Backup is copying the data directory while the app is stopped. Upgrade is
replacing the binary: migrations run at startup, under a lock, and are
forward-only.

### Status: not implemented yet

There is no embedded database in the code today. `Cargo.toml` has one driver
(`sqlx` with the `postgres` feature), and the storage layer is Postgres-shaped
throughout, not incidentally:

- `src/repository.rs` is ~8.5k lines of `sqlx::query!` — compile-time-checked
  against a live Postgres and cached in `.sqlx` for offline builds. A second
  backend does not reuse those macros.
- `src/notifications.rs` keeps a multi-instance cluster's caches coherent with
  `LISTEN`/`NOTIFY`.
- `src/revisions.rs` serialises revision numbering with
  `pg_advisory_xact_lock`, computes content digests in the database so blob
  bytes never cross the process, and collects orphans with
  `FOR UPDATE SKIP LOCKED`. `src/log_retention.rs` and the pruners use the same
  primitives.

**Two ways to get a standalone desktop build, and a recommendation.**

_Option A — bundle PostgreSQL with the app (recommended)._ Ship the Postgres
binaries next to the engine, `initdb` into the app data directory on first run,
listen on a loopback port or a Unix socket, and shut it down with the app.
Everything above keeps working verbatim: one storage implementation, one test
matrix, and desktop and server stay behaviourally identical — which is the whole
point of having a desktop mode. Costs: roughly 30–40 MB of platform binaries per
target, per-OS packaging, and a supervision path for "the database did not
start". The PostgreSQL licence permits redistribution.

_Option B — a SQLite backend._ Costs a parallel implementation of the
repository, a second set of queries (the `query!` macros are per-driver), and
re-homing everything listed above: advisory locks become an in-process mutex,
`NOTIFY` becomes an in-process broadcast (a single process needs no cross-
instance coherence at all), digests move into Rust, `SKIP LOCKED` disappears. It
is buildable, and it is permanently two backends to keep honest — every future
schema change and every repository test doubles. Worth it only if the desktop
build must be a single self-contained executable with no child process.

Either way, one thing has to be built that does not exist yet: **first-run
setup.** A desktop install cannot ask its user for four base64 keys. It needs to
generate `config.toml` on first launch with fresh random values, `0600`, in the
data directory — or keep them in the OS keychain — and then never regenerate
them, because regenerating `secret_encryption_key` destroys every stored secret.
`--validate-config` exists; a `--init-config`/first-run path does not.

## Developer local

**The engine on the developer's own machine, against a containerised Postgres.**
The fast loop, and what `make dev` targets.

```bash
make postgres-local          # Postgres container only, on localhost:5432
source .env && cargo run     # or: make dev (cargo-watch), ./dev-local.sh
```

`config.local.toml` is the template: debug logging, short JavaScript timeouts,
throwaway keys, internal auth fully on with `admin` as the bootstrap username,
and CORS allowing `localhost:3000` and `localhost:5173` so a separately served
front end can talk to it. The integration tests need this Postgres too — there
is no mocked repository, so `tests/*.rs` spin up real servers against a real
database.

This is also where a solution developer runs the engine while writing scripts,
so it should stay the lowest-ceremony option: no certificates, no DNS, no
containers for the engine itself.

## Developer local, containerised

**The same machine, but the full server topology.** Two engine containers behind
Caddy, plus Postgres — `docker-compose.local.yml` with `Caddyfile.local`. This
is what to use when the thing being tested is the deployment rather than the
code: TLS behaviour, the `X-Forwarded-For` chain and `trusted_proxies`, cookie
`Secure`/`__Host-` behaviour, multi-host routing and `management_hosts`, and
cross-instance cache invalidation over `LISTEN`/`NOTIFY`.

Two ways to reach it:

```bash
make docker-localhost   # https://localhost, Caddy's internal CA, no DNS needed
make docker-dns         # https://local.softagen.com, real Let's Encrypt cert
```

`make docker-dns` uses a DNS-01 challenge through DigitalOcean
(`DIGITALOCEAN_TOKEN`), which is what makes a publicly trusted certificate
possible for a name that resolves to a private address — no inbound port 80
needed. Use it when a real certificate and a real hostname matter: OAuth
redirect URIs the provider will accept, MCP clients that refuse self-signed
certificates, and anything testing the `__Host-` cookie prefix, which requires
`Secure`. `make check-dns` verifies the name resolves.

Note that the local compose file publishes Postgres on `5432` to the host, with
a known password. That is convenient — `psql` and the test suite reach the same
database — and it is a reason not to run this stack on a shared or exposed
machine.

## Server

**A cloud or on-premise host serving real traffic.** Caddy terminates TLS and
load-balances, one or more engine containers, and Postgres — containerised
alongside, or a managed instance.

The shape is `docker-compose.yml` with `Caddyfile.production` and
`config.production.toml`:

- Caddy holds `:80`/`:443` (and `:443/udp` for HTTP/3), obtains certificates
  automatically, sets `header_up X-Forwarded-For {remote_host}` so the proxy
  overwrites rather than appends the chain, and health-checks `/health` on each
  engine before sending it traffic.
- The engine containers publish nothing to the host — `expose: 3000` on the
  internal network only. A Host header arriving at the engine has therefore
  already been matched by one of Caddy's site blocks.
- Postgres publishes nothing either, and holds the only durable state.
- `/health` is the load-balancer probe: it runs a real `SELECT 1`, so an
  instance whose database is unreachable answers 503 and is pulled from
  rotation. `/engine/health/cluster` is the administrator's view of which
  instances are present.

**Single node or clustered** is a scale decision, not a different product. One
engine container is the right default for a small deployment; the second exists
for capacity and for surviving one instance restarting. Both are the same image
and the same configuration — the only thing the cluster adds is that cache
invalidation has to travel between instances, which it does over Postgres
`LISTEN`/`NOTIFY` rather than a separate message bus.

**Multi-host serving.** One engine cluster can answer for several hostnames
(`server.base_url` + `server.additional_base_urls`), with scripts bound to hosts
in the `script_hosts` table, and the management surface confined to
`server.management_hosts`. The production deployment uses this to keep
`/engine/*` on `manage.` while solution content is served from the apex — so a
script serving public content cannot drive the management API from a signed-in
administrator's browser. Every host used for sign-in needs its callback path
registered with each OAuth provider.

**Managed Postgres** is a supported variant of the same topology: drop the
`postgres` service and point `APP_REPOSITORY__DATABASE_URL` at the managed
instance with `sslmode=require`. One caveat that will bite silently — if a
connection pooler is put in front of the database, it must run in session
pooling mode. Transaction pooling breaks `LISTEN`/`NOTIFY`, and the visible
symptom is not an error but stale caches on instances that never hear about a
script write.

### Staging

**Production's topology, different names and data.** That is the whole
specification: same image, same compose shape, same Caddy configuration, same
number of instances, same `management_hosts` discipline — differing only in
hostnames, credentials, database contents, and log level. A staging environment
that differs structurally tests something other than what will be shipped.

Concretely that means staging should not be its own compose file with its own
service names and its own instance count. Prefer one compose file parameterised
by environment: `docker compose --env-file .env.staging up -d`, with the
hostname in the Caddyfile coming from a variable (`{$SITE_HOST}`) rather than
being written in. What is genuinely staging-specific — a lower log level, a
smaller instance, a permissive `bootstrap_admins` — belongs in the env file.

Image promotion should follow the same rule: staging and production run the
_same digest_, promoted, not two builds of the same commit.

## One set of files, or several?

Almost all of it can be one set of files driven by a per-environment `.env`.
The claims below were checked with `caddy adapt` and `docker compose config`
rather than assumed.

**One Caddyfile covers localhost, DNS-01 development, staging and production.**
Caddy substitutes `{$VAR}` textually before parsing, so a variable can carry
more than one token:

- `{$SITE_HOSTS}` as the site address, with `SITE_HOSTS="softagen.com, world.softagen.com"`,
  adapts to a two-host matcher.
- `reverse_proxy {$ENGINE_UPSTREAMS}` with `"aiwebengine-1:3000 aiwebengine-2:3000"`
  adapts to two upstreams — active health checks and `lb_policy` intact — and to
  one upstream when the variable names one. Instance count therefore lives in
  the env file, not in the Caddyfile.
- `import {$TLS_SNIPPET}` selects the issuer: a snippet holding `tls internal`
  for localhost, one holding the DNS-01 block for a development hostname, and an
  empty snippet for public ACME in staging and production.

**Two things a single Caddyfile cannot express**, both because every site block
in the file is always adapted:

- An _empty_ variable used as a site address is a hard error
  (`server block without any key is global configuration`). This is what makes
  a "management host block, if there is one" impossible to write.
- Two blocks resolving to the same host is `ambiguous site definition`, so a
  single-host deployment cannot simply point both at the same name.

That matters only because `Caddyfile.production` gives the management host a
different `X-Frame-Options` than the public hosts. The durable fix is to stop
expressing that in the proxy: the engine already owns the CSP on
`/engine/*` (`security/headers.rs`, `security::engine_page_policy`), and
`frame-ancestors` supersedes `X-Frame-Options` in current browsers. Move the
frame policy there and every host takes the same Caddy configuration.

**One Dockerfile, two targets.** The existing multi-stage build already has the
pieces; add a `dev` stage carrying the toolchain and `cargo-watch`, and select it
with `build: { target: "${ENGINE_BUILD_TARGET:-runtime}" }`. Use one Caddy image
too, built with the DigitalOcean DNS plugin — the plugin is inert unless a `tls`
block names it, and stock `caddy:2-alpine` fails only when that directive is
actually reached.

**One compose file, plus overlays for what is structurally different.**

- A YAML anchor (`x-engine: &engine`, then `<<: *engine`) removes the
  copy-pasted second instance; the two engine services in `docker-compose.yml`
  are duplicated env blocks today, which is how they drift.
- The second instance goes behind `profiles: ["ha"]`, so single-node and
  clustered are the same file with `COMPOSE_PROFILES` set or not.
- A bundled Postgres cannot be a profile: `depends_on` naming a service whose
  profile is inactive fails the whole project
  (`depends on undefined service "postgres"`). It has to be a small overlay file
  that adds both the service and the `depends_on` entries — which a managed-
  database deployment simply omits.
- `.env` can carry `COMPOSE_FILE` and `COMPOSE_PROFILES` themselves, so the file
  list, the instance count and every value come from one place:
  `docker compose --env-file .env.staging up -d`.

**What stays genuinely separate:** the development overlay. Source bind-mounts,
cargo cache volumes and a watch command are a different kind of container, not
the same container with different parameters — and that is exactly what a
compose overlay is for. Desktop standalone has no Caddy and no compose at all.

**Configuration templates** can collapse the same way: `config.production.toml`
is already `${...}` throughout, and an unexpanded placeholder is refused at
startup rather than used as a secret (`src/config.rs:406`), so staging and
production can share one file. Keep `config.local.toml` as its own file — its
value is being a readable set of development defaults, not a template.

## Operational essentials

Applying to every server deployment, and to the desktop build in reduced form:

- **Backups.** `pg_dump` on a schedule, restore rehearsed at least once, and the
  four secrets stored alongside — see the note under "What every deployment
  needs". Neither compose file schedules a dump today.
- **Upgrades.** Migrations run automatically at startup under a lock, so two
  instances starting together are safe. What is not automatic is compatibility:
  during a rolling restart the old and new binaries both run against the _new_
  schema, so a migration must be backward compatible for one release, or both
  instances must be stopped for the upgrade. Migrations are forward-only; there
  is no down path.
- **Monitoring.** `/health` per instance, `/engine/health/cluster` for the
  cluster, JSON-structured logs (`logging.format = "json"`) for aggregation, and
  Caddy's JSON access log. There is no metrics endpoint — the Prometheus and
  Grafana services in the compose files are commented-out placeholders.
- **Retention.** `[logs]` and `[revisions]` are enforced by background pruners
  and are what keep the database from growing without bound; leave
  `prune_enabled` on.
- **Secrets handling.** Everything sensitive comes from the environment
  (`APP_<SECTION>__<KEY>`), never from a committed config file.
  `config.production.toml` references `${...}` placeholders, and an unset one is
  dropped with a warning rather than kept as a literal.

## Related documentation

- [03 - Running Environments](docs/engine-administrators/03-RUNNING-ENVIRONMENTS.md) — step-by-step for each environment
- [02 - Configuration](docs/engine-administrators/02-CONFIGURATION.md) — every setting
- [04 - Secrets and Security](docs/engine-administrators/04-SECRETS-AND-SECURITY.md) — OAuth setup, key generation, bootstrap admins
- [05 - Monitoring and Maintenance](docs/engine-administrators/05-MONITORING-AND-MAINTENANCE.md) — health checks, backups, user management
- [Database Migrations](docs/engine-administrators/DATABASE-MIGRATIONS.md)
- [Internal Authentication](docs/INTERNAL_AUTH.md) — guests and local accounts, which is how a desktop or provider-less install signs anyone in
