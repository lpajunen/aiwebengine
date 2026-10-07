# Operations

Running an engine that is already deployed: health, logs, accounts, backups.
Choosing and setting up a deployment is `DEPLOYMENT.md`; every configuration
key is described in `config.toml`.

## Health

`GET /health` answers 200 when the instance can reach Postgres and 503 when it
cannot, so a load balancer takes an instance out of rotation when its database
goes. Per-instance cluster diagnostics are at `/engine/health/cluster`, for
administrators on a management host.

The containerised deployments check liveness with a TCP connect rather than a
request to `/health`; the runtime image carries no HTTP client.

## Logs

The engine logs to stdout, and Docker collects it:

```bash
make docker-logs ENV=production        # follow
docker compose logs --since 2h aiwebengine-1
```

A script's own `console.*` output is stored in the database, read with
`read_logs` (or `/engine/read_logs`) by the script's owner or an administrator,
and pruned by `[logs]` retention in `config.toml`.

## Accounts and roles

An administrator manages accounts with the engine operations, over
`/engine/<name>` or the MCP tool of the same name:

| Operation          | What it does                                         |
| ------------------ | ---------------------------------------------------- |
| `list_users`       | Every account, with its roles and identity providers |
| `add_user_role`    | Grants `Editor` or `Administrator`                   |
| `remove_user_role` | Revokes one                                          |
| `set_user_realm`   | Which hosts an account is a principal on             |

```bash
curl -X POST https://manage.example.com/engine/add_user_role \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"user_id": "<id from list_users>", "role": "Editor"}'
```

`Authenticated` is the base role every account holds and cannot be revoked; the
last `Administrator` cannot be revoked either. A role change signs the account
out everywhere, so it takes effect on the next request. The first administrator
comes from configuration — see `OAUTH-AND-SECRETS.md`.

## Backup and restore

### What a backup consists of

1. **A dump of the database.** Scripts, files, users, sessions, secrets, logs,
   revisions and deployment pins all live there. There is no other storage.
2. **The environment file** (`.env-production`, `.env-staging`). It holds
   `APP_SECURITY__SECRET_ENCRYPTION_KEY`, without which every script and user
   secret in the dump is ciphertext. A dump restored without that key comes back
   working, with every secret unreadable. Keep the file with the dumps, and the
   pair somewhere other than the host they came from.
3. **Caddy's data volume**, optionally. It holds the issued certificates; losing
   it costs a re-issuance, not data.

### Taking them

Scheduled, inside the stack:

```bash
# In the env file:
export COMPOSE_PROFILES=backup          # or ha,backup for a clustered deployment
export BACKUP_INTERVAL_SECONDS=86400    # daily
export BACKUP_KEEP=14
```

The `backup` service (`scripts/pg-backup.sh`) runs `pg_dump -Fc` on that
interval into the `backup-data` volume, keeping the newest `BACKUP_KEEP`. It
runs the database's own image, so client and server versions match, and is given
the same `DATABASE_URL` as the engine containers. A dump is written under a
temporary name and renamed only once `pg_dump` succeeds.

By hand:

```bash
make docker-backup       ENV=production   # take one now
make docker-backup-list  ENV=production   # what the volume holds, newest first
make docker-backup-fetch ENV=production   # copy the newest to ./backups
```

The dumps live on the machine running the Docker daemon. They survive
`docker compose down`, and are not a backup until they have left that machine —
`docker-backup-fetch` is the step that makes them one. Store the env file
separately from the dumps: together, one leaked credential is every secret,
decrypted.

A deployment on a managed database uses the provider's backups instead, and
still keeps the env file — the key is not in the provider's snapshot.

### Restoring

```bash
make docker-backup-list ENV=production
make docker-restore ENV=production FILE=aiwebengine-20260906T020000Z.dump CONFIRM=yes
```

The target stops the engine containers, restores, and starts them again: a
restore drops and recreates every table, which must not happen under instances
running scripts against them. `pg_restore` runs with `--clean --if-exists`, so
it replaces a populated database, and `--exit-on-error`, so a partial restore
does not report success.

If the encryption key differs from the one the dump was taken under, the restore
succeeds and every stored secret is unreadable. Check one before deciding a
restore worked: no operation returns a secret's value, but `crypto.secretEquals`
decrypts it host-side and compares it with one you know, so an `eval_script`
against a script holding the secret answers the question:

```bash
curl -X POST https://manage.example.com/engine/eval_script \
  -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"script": "<a script with a secret>", "source": "crypto.secretEquals(\"KEY\", \"<the value you stored>\")"}'
```

A value of `true` means the key decrypts what the dump holds; `false` or an error means it does not.

### Rehearsing a restore

Rehearse somewhere that is not production, at least once and again after any
change to the stack:

```bash
# 1. A throwaway environment: the production env file under another project name.
cp .env-production .env-rehearsal
sed -i.bak 's/^export COMPOSE_PROJECT_NAME=.*/export COMPOSE_PROJECT_NAME=aiwebengine-rehearsal/' .env-rehearsal
sed -i.bak 's/^export ENV_FILE=.*/export ENV_FILE=.env-rehearsal/' .env-rehearsal

# 2. Bring up just its database, and restore into it.
docker compose --env-file .env-rehearsal up -d postgres
make docker-restore ENV=rehearsal FILE=<the dump> CONFIRM=yes

# 3. Check what came back.
docker compose --env-file .env-rehearsal exec -T postgres \
  psql -U aiwebengine -d aiwebengine -c \
  "SELECT (SELECT count(*) FROM scripts) AS scripts,
          (SELECT count(*) FROM users)   AS users,
          (SELECT count(*) FROM assets)  AS files;"

# 4. Start the engine against it and check a secret with crypto.secretEquals
#    (see Restoring above).

# 5. Tear it down.
docker compose --env-file .env-rehearsal down -v
rm .env-rehearsal .env-rehearsal.bak
```

Step 4 is the one not to skip: steps 1–3 pass with the wrong encryption key.

### Desktop standalone

`config.toml` and `postgres/` in the app's data directory are the whole
install. Back them up together with the app stopped — a copy of a running
PostgreSQL's data directory may not start — and never one without the other,
since `config.toml` holds the encryption key.
