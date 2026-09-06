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
  `security.secret_encryption_key`. The values in `.env-local` are published in
  this repository and are for a local install only.
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

- `repository.embedded = true` and an `embedded_data_dir` under the app's data
  directory. `database_url` is ignored; the engine starts its own PostgreSQL and
  connects to the port it came up on. Needs a build carrying the feature — see
  the status section below.
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

Running it:

```bash
make run-desktop             # builds if needed, then starts on http://localhost:3000
aiwebengine --desktop        # what that runs, once the binary exists
```

The first launch creates everything it needs. `--desktop` resolves the
platform's application-data directory — `~/Library/Application Support/aiwebengine`
on macOS, `%APPDATA%\aiwebengine` on Windows, `$XDG_DATA_HOME/aiwebengine` or
`~/.local/share/aiwebengine` elsewhere, or whatever `AIWEBENGINE_DATA_DIR`
names — writes a `config.toml` there at mode 600 with four freshly generated
keys, and starts a PostgreSQL in `postgres/` beside it. No network is needed:
the archive is in the binary.

`aiwebengine --init-config` does the creating without starting anything, and
prints where it went.

**It never regenerates.** An existing configuration is used exactly as it
stands, whatever else has changed, because `secret_encryption_key` is what
every script and user secret in the database is encrypted with — a second set
of keys would not reset the install, it would make the install unreadable while
leaving it looking healthy.

What it writes is an ordinary configuration file, loaded by the same
`AppConfig::load_from_file` every other deployment uses, and `APP_*` variables
still override it. There is no desktop-only code path downstream of that.

To claim the install: register the username `owner` at
`http://localhost:3000/auth/login`. It is named in the generated
`auth.internal.bootstrap_admin_usernames`, so signing in grants it the
administrator role; set `allow_registration = false` afterwards, since a
desktop install has one account. `Ctrl-C` or SIGTERM stops the engine and its
database together.

`config.toml` and `postgres/` in that directory are the whole install. Back
them up together, with the app stopped: a copy of a running PostgreSQL's data
directory may not start, and the directory without `config.toml` restores an
engine whose secrets are unreadable. Upgrade is replacing the binary:
migrations run at startup, under a lock, and are forward-only.

### Status: the supervisor and first run exist, the packaging does not

`repository.embedded = true` on a build carrying the `embedded-postgres`
feature starts a PostgreSQL of the engine's own, initialises it on first run,
creates the database, runs the migrations and stops it on shutdown. An engine
built with `--features embedded-postgres-bundled` (`make build-desktop`) carries
the platform's PostgreSQL archive inside the binary, so a first launch needs no
network.

**It is one storage implementation, not two.** `src/embedded_db.rs` starts a
real PostgreSQL on loopback and hands `database.rs` a connection string;
everything downstream of `RepositoryConfig::connection_string` is byte-for-byte
what a cluster runs — the `sqlx::query!` macros in `repository.rs`, the
`pg_advisory_xact_lock` in `revisions.rs`, `LISTEN`/`NOTIFY`, the
`FOR UPDATE SKIP LOCKED` in the pruners. That is what keeps the promise at the
top of this document: a solution developed against a desktop install runs
unchanged on a cluster, which it could not if the two ran different storage.

Two things are worth knowing about how it behaves:

- **The cluster persists, and so does its password.** `initdb` sets a password
  on the first run that the cluster still expects on every run after it, so the
  engine reads it back out of `<embedded_data_dir>/.pgpass` rather than
  generating a fresh one it would then fail to authenticate with. That file is
  narrowed to its owner once it exists.
- **The port moves and the address does not.** `embedded_port = 0` takes a free
  loopback port at each start, since nothing outside the process connects to it;
  `listen_addresses` is pinned to `127.0.0.1` on the command line rather than
  left to the `postgresql.conf` `initdb` wrote, because a database holding every
  secret in the install should not reach the network by inheriting a default.

**Why it is behind a compile-time feature.** A server deployment reaches a
PostgreSQL it does not own and should not pay for the code that starts one. The
cost being avoided is binary size, build time and dependency surface — not
runtime memory, which does not move for code that is compiled and never run:

| Build                                                         | What it gets                                        |
| ------------------------------------------------------------- | --------------------------------------------------- |
| `cargo build --release` (`make build`)                        | nothing — the supervisor is not compiled            |
| `--features embedded-postgres`                                | the supervisor; the archive is fetched on first run |
| `--features embedded-postgres-bundled` (`make build-desktop`) | the above, plus ~13 MB of archive in the binary     |

`make check` and `make ci` deliberately do not pass `--all-features`
(`TEST_FEATURES` in the Makefile): the integration suite claims a slot database
on the server `DATABASE_URL` names, which is the one thing an engine that starts
its own database must not do. `make check-embedded` compile-checks the
supervisor without staging an archive, and `cargo run --features
embedded-postgres --example embedded_smoke -- <dir>` exercises it for real —
install, migrate, restart, stop — which no test in the suite can.

**First-run setup exists now** (`src/desktop.rs`, `--desktop` /
`--init-config`), and it had to move into the binary for the reason above: the
Makefile's `.env-desktop` generation worked for somebody with a checkout, a
toolchain and `make`, which is exactly the population that does not need a
desktop build.

**What is still missing is packaging.** A per-OS bundle, an app icon, something
that opens a browser at the loopback port, code signing and notarization on
macOS, and a release workflow that builds
`--features embedded-postgres-bundled` for each platform. None of that is
engine work; all of it is between the binary and a user who does not have a
terminal.

**The road not taken: a SQLite backend.** It would cost a parallel
implementation of the repository, a second set of queries (the `query!` macros
are per-driver, so a `#[cfg]` cannot share them), and re-homing everything
listed above: advisory locks become an in-process mutex, `NOTIFY` an in-process
broadcast, digests move into Rust, `SKIP LOCKED` disappears. It is buildable,
and it is permanently two backends to keep honest — every future schema change
and every repository test doubles. Worth it only if the desktop build must be a
single self-contained executable with no child process.

## Developer local

**The engine on the developer's own machine, against a containerised Postgres.**
The fast loop, and what `make dev` targets.

```bash
make postgres-local          # Postgres container only, on localhost:5432
source .env && cargo run     # or: make dev (cargo-watch), ./dev-local.sh
```

`.env-local` carries the development values over `config.toml`'s defaults:
debug logging, short JavaScript timeouts, throwaway keys, internal auth fully on
with `admin` as the bootstrap username, and CORS allowing `localhost:3000` and
`localhost:5173` so a separately served front end can talk to it. The integration tests need this Postgres too — there
is no mocked repository, so `tests/*.rs` spin up real servers against a real
database.

This is also where a solution developer runs the engine while writing scripts,
so it should stay the lowest-ceremony option: no certificates, no DNS, no
containers for the engine itself.

## Developer local, containerised

**The same machine, running the server stack.** Not a stack of its own:
`docker-compose.local.yml` is an _overlay_ over `docker-compose.yml`, and the
Caddyfile is the same `Caddyfile` production runs. This is what to use when the
thing being tested is the deployment rather than the code: TLS behaviour, the
`X-Forwarded-For` chain and `trusted_proxies`, cookie `Secure`/`__Host-`
behaviour, multi-host routing and `management_hosts`, and cross-instance cache
invalidation over `LISTEN`/`NOTIFY`.

The overlay is deliberately thin, and what it contains is the one thing that
genuinely differs: these engine containers carry a toolchain and compile the
crate, which is a different kind of container rather than the same container
with different parameters. Everything else — the site block, the health checks,
the retry policy, the `ha` profile, the number of instances — comes from the
server file, and is therefore the same thing being rehearsed. It had been a
parallel stack with its own service names, its own Caddyfile and its own
hardcoded instance count, which is a rehearsal of something other than what
ships.

Two engine instances by default, since cross-instance invalidation is one of
the things only a second instance exercises. For the single-node shape, the
same shape a small deployment runs:

```bash
make docker-local DEV_PROFILES= DEV_UPSTREAMS=aiwebengine-1:3000
```

Two ways to reach it:

```bash
make docker-localhost   # https://localhost, Caddy's internal CA, no DNS needed
make docker-dns         # https://local.softagen.com, real Let's Encrypt cert
```

Which certificate a stack gets is `TLS_SNIPPET`, naming one of three issuers
defined in the shared `Caddyfile`: `tls_public` (public ACME — what staging and
production use), `tls_internal` (Caddy's own CA) and `tls_acme_dns` (DNS-01).
A snippet rather than a conditional because a Caddyfile is adapted whole and a
site block cannot be made conditional; an unimported snippet is not adapted at
all, which is what lets `tls_acme_dns` name a plugin that only the development
Caddy image carries.

`make docker-dns` uses a DNS-01 challenge through DigitalOcean
(`DIGITALOCEAN_TOKEN`), which is what makes a publicly trusted certificate
possible for a name that resolves to a private address — no inbound port 80
needed. Use it when a real certificate and a real hostname matter: OAuth
redirect URIs the provider will accept, MCP clients that refuse self-signed
certificates, and anything testing the `__Host-` cookie prefix, which requires
`Secure`. `make check-dns` verifies the name resolves.

Note that the overlay publishes Postgres on `5432` to the host, with a known
password. That is convenient — `psql` and the test suite reach the same
database — and it is a reason not to run this stack on a shared or exposed
machine.

## Server

**A cloud or on-premise host serving real traffic.** Caddy terminates TLS and
load-balances, one or more engine containers, and Postgres — containerised
alongside, or a managed instance.

The shape is `docker-compose.yml` with `Caddyfile`, the single `config.toml`,
and a `.env-production` holding everything specific to this deployment —
the same three files staging uses, with a `.env-staging` instead:

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

It is one set of files driven by a per-environment `.env`, and the claims below
were checked with `caddy adapt` and `docker compose config` rather than
assumed.

**One Caddyfile covers localhost, DNS-01 development, staging and production.**
Caddy substitutes `{$VAR}` textually before parsing, so a variable can carry
more than one token:

- `{$SITE_HOSTS}` as the site address, with `SITE_HOSTS="softagen.com, world.softagen.com"`,
  adapts to a two-host matcher.
- `reverse_proxy {$ENGINE_UPSTREAMS}` with `"aiwebengine-1:3000 aiwebengine-2:3000"`
  adapts to two upstreams — active health checks and `lb_policy` intact — and to
  one upstream when the variable names one. Instance count therefore lives in
  the env file, not in the Caddyfile.
- `import {$TLS_SNIPPET}` selects the issuer: `tls_internal` holding
  `tls internal` for localhost, `tls_acme_dns` holding the DNS-01 block for a
  development hostname, and an empty `tls_public` for public ACME in staging and
  production. It must always name one of them — `import` with an empty argument
  is a parse error that takes the whole proxy down rather than falling back —
  which is why the compose file supplies the default rather than the Caddyfile.

**Two things a single Caddyfile cannot express**, both because every site block
in the file is always adapted:

- An _empty_ variable used as a site address is a hard error
  (`server block without any key is global configuration`). This is what makes
  a "management host block, if there is one" impossible to write.
- Two blocks resolving to the same host is `ambiguous site definition`, so a
  single-host deployment cannot simply point both at the same name.

That mattered because the management host takes a different `X-Frame-Options`
than the public ones. It is expressed as a matcher rather than a second site
block — `@manage host {$MANAGE_HOST}` and `@content not host {$MANAGE_HOST}` —
which needs no conditional and works on a single-host deployment by pointing
`MANAGE_HOST` at that one name, where every response is then `DENY`.

**Two Caddy images, one Caddyfile.** The alternative — one image carrying the
DigitalOcean DNS plugin everywhere — would put an `xcaddy` build in front of
every production deployment for a feature only local development uses. What
makes two images cost nothing is that an unimported Caddyfile snippet is never
adapted: the server image is stock `caddy:2-alpine` and simply never imports
`tls_acme_dns`, while the development overlay builds the plugin in. Both copy
the same `Caddyfile` and the same `caddy-sites/`, so the configuration being
rehearsed is the configuration that ships.

**One Dockerfile per kind of container.** `Dockerfile` builds the runtime image;
`Dockerfile.local` carries the toolchain and `cargo-watch`. This could be one
multi-stage build selected with `build: { target: ... }` and is not yet.

**One compose file, plus overlays for what is structurally different.**

- A top-level YAML anchor (`x-engine: &engine`, then `<<: *engine`) defines an
  engine instance once. The two services were duplicated twenty-line blocks,
  which is how they drift: every change has to be made twice, and one made in
  only one place is invisible until the instance behaving differently is the one
  serving the request. The anchor has to be top-level rather than inside
  `services:` — compose reads `x-engine` there as a service and tries to start
  it.
- The second instance goes behind `profiles: ["ha"]`, so single-node and
  clustered are the same file with `COMPOSE_PROFILES` set or not. The scheduled
  `backup` service is behind `profiles: ["backup"]` for the same reason;
  `COMPOSE_PROFILES` is one variable, so a deployment wanting both writes
  `ha,backup`.
- A bundled Postgres cannot be a profile: `depends_on` naming a service whose
  profile is inactive fails the whole project
  (`depends on undefined service "postgres"`). It has to be a small overlay file
  that adds both the service and the `depends_on` entries — which a managed-
  database deployment simply omits.
- `.env` can carry `COMPOSE_FILE` and `COMPOSE_PROFILES` themselves, so the file
  list, the instance count and every value come from one place:
  `docker compose --env-file .env.staging up -d`.

**What stays genuinely separate:** the development overlay, and it is now an
overlay rather than a parallel stack. Source bind-mounts, cargo cache volumes
and a watch command are a different kind of container, not the same container
with different parameters — and that is exactly what a compose overlay is for.
It holds nothing else: the service names, the Caddyfile, the health checks, the
profiles and the instance count all come from the server file. Desktop
standalone has no Caddy and no compose at all.

**One trap this arrangement has, worth knowing.** Compose reads a variable from
the shell in preference to its `--env-file`. This project's documented workflow
is `source .env-local && cargo run`, so a shell that has done that used to
carry `ENV_FILE`, `SITE_HOSTS` and friends into every later compose command —
including one deploying production, which would then load `.env-local`'s
throwaway keys and serve `SITE_HOSTS=localhost`. Two things answer it: nothing
compose interpolates lives in `.env-local` any more (the Makefile supplies those
per invocation, where they cannot leak into a shell), and every server target
refuses to run while such a variable is set, naming it.

**Configuration is one file.** `config.toml` holds the defaults and the
reasoning behind them; every environment supplies its differences as `APP_`
variables in its own env file — `.env-local` (tracked, throwaway values),
`.env-staging`, `.env-production` (copied from `.env.example`). Compose loads
that same file twice over: `--env-file` supplies what the YAML interpolates, and
`env_file: ${ENV_FILE}` supplies what the containers receive, so one file per
environment drives everything.

Two details make this work rather than merely look tidy. Services declare
`env_file` instead of listing keys, because a listed `- KEY=${KEY:-}` sets the
key whether the environment defines it or not, and an empty value overrides the
config file exactly as a real one does — "unset" has to mean "absent from the
env file". And the entry is `required: false`, so `docker compose down`, `ps`
and `logs` still work in a checkout that has no env file yet.

## Operational essentials

Applying to every server deployment, and to the desktop build in reduced form:

- **Backups.** The `backup` profile runs `pg_dump -Fc` on an interval into a
  named volume, keeping the newest `BACKUP_KEEP`; turn it on with
  `COMPOSE_PROFILES=backup` (or `ha,backup`). `make docker-backup`,
  `docker-backup-list`, `docker-backup-fetch` and `docker-restore` are the
  by-hand half. Two things the mechanism cannot do for you: the dumps sit on the
  machine running the daemon until `docker-backup-fetch` takes them off it, and
  the env file has to travel with them — a dump restored without
  `secret_encryption_key` comes back with every secret unreadable. Rehearse a
  restore, including a secret read, since the first three steps of one pass with
  the wrong key. See
  [05 - Monitoring and Maintenance](docs/engine-administrators/05-MONITORING-AND-MAINTENANCE.md#backup-and-restore).
- **Upgrades.** `make docker-pull ENV=<env>` then `make docker-deploy ENV=<env>`:
  the roll replaces one instance at a time and waits on its health check before
  touching the next, so a clustered deployment is never without an instance that
  has finished starting. Three things have to hold together for that to be
  seamless, and all three are now set: `stop_grace_period: 40s` on the engine
  services, longer than the engine's own `shutdown_timeout_secs` so a drain is
  not interrupted by SIGKILL; `lb_try_duration` in the Caddyfile, which retries
  on the other upstream the dial that a stopping instance refuses, instead of
  answering 502; and the `ha` profile, since a single-instance deployment has
  nowhere to send the requests and the target says so.

  Migrations run automatically at startup under a lock, so two instances
  starting together are safe. What is not automatic is compatibility: during a
  roll the old and new binaries both run against the _new_ schema, so a
  migration must be backward compatible for one release, or both instances must
  be stopped for the upgrade. Migrations are forward-only; there is no down
  path.
- **Monitoring.** `/health` per instance, `/engine/health/cluster` for the
  cluster, JSON-structured logs (`logging.format = "json"`) for aggregation, and
  Caddy's JSON access log. There is no metrics endpoint — the Prometheus and
  Grafana services in the compose files are commented-out placeholders.
- **Retention.** `[logs]` and `[revisions]` are enforced by background pruners
  and are what keep the database from growing without bound; leave
  `prune_enabled` on.
- **Secrets handling.** Everything sensitive comes from the environment
  (`APP_<SECTION>__<KEY>`), never from a committed config file. Note that
  nothing expands `${VAR}` inside a TOML file: the loader merges the file and
  then the `APP_` environment over it, so a `${...}` left in a config file is
  not substituted, it is _used_ — a literal placeholder reaching the code as a
  redirect URI or a signing key. That is why `config.toml` contains no
  placeholders and no secrets, and why the keys with no safe default are simply
  absent from it, so the engine refuses to start rather than starting on a
  string nobody chose.

## Related documentation

- [03 - Running Environments](docs/engine-administrators/03-RUNNING-ENVIRONMENTS.md) — step-by-step for each environment
- [02 - Configuration](docs/engine-administrators/02-CONFIGURATION.md) — every setting
- [04 - Secrets and Security](docs/engine-administrators/04-SECRETS-AND-SECURITY.md) — OAuth setup, key generation, bootstrap admins
- [05 - Monitoring and Maintenance](docs/engine-administrators/05-MONITORING-AND-MAINTENANCE.md) — health checks, backups, user management
- [Database Migrations](docs/engine-administrators/DATABASE-MIGRATIONS.md)
- [Internal Authentication](docs/INTERNAL_AUTH.md) — guests and local accounts, which is how a desktop or provider-less install signs anyone in
