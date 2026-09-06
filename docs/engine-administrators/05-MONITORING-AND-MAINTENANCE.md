# 05 - Monitoring and Maintenance

Guide for keeping your aiwebengine instance healthy, monitoring operations, and performing maintenance tasks.

## Quick Navigation

- [Health Monitoring](#health-monitoring)
- [Log Management](#log-management)
- [User Management](#user-management)
- [Database Maintenance](#database-maintenance)
- [Backup and Restore](#backup-and-restore)
- [Updates and Upgrades](#updates-and-upgrades)
- [Performance Monitoring](#performance-monitoring)

---

## Health Monitoring

### Health Check Endpoint

aiwebengine provides a built-in health check endpoint.

**Check health:**

```bash
curl http://localhost:3000/health

# Expected response:
# {"status":"ok","timestamp":"2025-10-24T12:34:56Z"}
```

**In production:**

```bash
curl https://yourdomain.com/health
```

### Docker Health Checks

Docker containers include built-in health checks.

**Check container health:**

```bash
# View status
docker-compose ps

# Detailed health info
docker inspect $(docker-compose ps -q aiwebengine) | grep -A 10 Health
```

**Health check configuration** (in docker-compose.yml):

```yaml
healthcheck:
  test: ["CMD", "curl", "-f", "http://localhost:8080/health"]
  interval: 30s
  timeout: 10s
  retries: 3
  start_period: 40s
```

### Automated Monitoring

#### UptimeRobot (Free tier available)

1. Go to [uptimerobot.com](https://uptimerobot.com/)
2. Create monitor:
   - Type: HTTP(s)
   - URL: `https://yourdomain.com/health`
   - Interval: 5 minutes
3. Set up alerts (email, SMS, Slack, etc.)

#### Custom Health Check Script

```bash
#!/bin/bash
# health-check.sh

HEALTH_URL="https://yourdomain.com/health"
ALERT_EMAIL="admin@yourdomain.com"

response=$(curl -s -o /dev/null -w "%{http_code}" "$HEALTH_URL")

if [ "$response" != "200" ]; then
    echo "Health check failed! Status: $response" | \
        mail -s "aiwebengine Health Alert" "$ALERT_EMAIL"
    exit 1
fi

echo "Health check passed"
exit 0
```

**Run via cron** (every 5 minutes):

```bash
*/5 * * * * /path/to/health-check.sh >> /var/log/health-check.log 2>&1
```

---

## Log Management

### Log Locations

**Docker deployment:**

```bash
# Application logs (on host)
./logs/aiwebengine.log
./logs/aiwebengine-dev.log  # Local development
./logs/aiwebengine-staging.log  # Staging

# Container logs
docker-compose logs aiwebengine
```

**Bare metal deployment:**

```bash
# System logs
/var/log/aiwebengine/aiwebengine.log

# Systemd journal
sudo journalctl -u aiwebengine
```

### Viewing Logs

**Docker:**

```bash
# Follow logs (real-time)
docker-compose logs -f aiwebengine

# Last 100 lines
docker-compose logs --tail=100 aiwebengine

# Specific time range
docker-compose logs --since 2h aiwebengine

# Search for errors
docker-compose logs aiwebengine | grep -i error

# Save logs to file
docker-compose logs --no-color aiwebengine > aiwebengine-$(date +%Y%m%d).log
```

**Systemd:**

```bash
# Follow logs
sudo journalctl -u aiwebengine -f

# Last 100 lines
sudo journalctl -u aiwebengine -n 100

# Today's logs
sudo journalctl -u aiwebengine --since today

# Errors only
sudo journalctl -u aiwebengine -p err

# Export logs
sudo journalctl -u aiwebengine --since "2025-10-24" > logs-$(date +%Y%m%d).log
```

### Log Rotation

Logs are automatically rotated based on configuration.

**Configuration** (in config.toml):

```toml
[logging]
rotation = "daily"    # hourly, daily, weekly
retention_days = 30   # Keep logs for 30 days
```

**Manual cleanup** (if needed):

```bash
# Remove old logs (older than 30 days)
find ./logs -name "*.log.*" -mtime +30 -delete

# Compress old logs
find ./logs -name "*.log.*" -mtime +7 -exec gzip {} \;
```

### Centralized Logging

#### Using Loki (Docker)

Add to docker-compose.yml:

```yaml
services:
  loki:
    image: grafana/loki:latest
    ports:
      - "3100:3100"
    volumes:
      - loki-data:/loki

  promtail:
    image: grafana/promtail:latest
    volumes:
      - /var/log:/var/log:ro
      - ./loki-config.yml:/etc/promtail/config.yml
    command: -config.file=/etc/promtail/config.yml

volumes:
  loki-data:
```

#### Using CloudWatch (AWS)

Install CloudWatch agent and configure log streaming.

#### Using Elasticsearch/Logstash

Configure log forwarding via filebeat or similar.

---

## User Management

The engine lets administrators list users and manage their roles over HTTP
(`/engine/users`, `/engine/user_roles`) and over MCP (`list_users`,
`add_user_role`, `remove_user_role`). See [API Endpoints](#api-endpoints) below.

The engine ships no user-management web UI of its own — the `/engine` prefix is
reserved for engine routes, so a management console is a solution script served
from a path of its own, built on the endpoints documented here.

### Access Control

**Administrator Access Only:** These endpoints require an authenticated session
whose user holds the Administrator role. Every other caller — anonymous,
authenticated non-admin — receives a 403 Forbidden.

### Getting Administrator Access

You need Administrator privileges. See the [Bootstrap Admin Configuration](04-SECRETS-AND-SECURITY.md#bootstrap-admin-configuration) section for setting up your first administrator account.

Quick example:

```toml
# config.toml
[auth]
bootstrap_admins = ["your.email@company.com"]
```

After signing in with this email via OAuth, you'll automatically have Administrator access.

### Roles

| Role            | Meaning                                                          |
| --------------- | ---------------------------------------------------------------- |
| `Authenticated` | Base role held by every user. Cannot be granted away or revoked. |
| `Editor`        | May edit scripts and assets beyond the ones they own.            |
| `Administrator` | Full access, including user administration.                      |

### Managing User Roles

Find the user's id with `GET /engine/users`, then grant or revoke:

```bash
# Grant the Editor role
curl -X POST https://yourdomain.com/engine/user_roles \
  -H "Content-Type: application/json" \
  -b cookies.txt \
  -d '{"user_id": "uuid-here", "role": "Editor"}'

# Revoke it again
curl -X DELETE \
  "https://yourdomain.com/engine/user_roles?user_id=uuid-here&role=Editor" \
  -b cookies.txt
```

Both calls return the user's resulting role set, so no follow-up read is needed
to confirm the change. Role changes are recorded in the security audit log.

### API Endpoints

User administration is engine functionality, served natively over HTTP and MCP.
Every endpoint below requires an authenticated session whose user holds the
Administrator role; anything else is rejected with 403.

#### List Users

**Endpoint:** `GET /engine/users`

**Response:**

```json
{
  "users": [
    {
      "id": "uuid-here",
      "email": "user@example.com",
      "name": "John Doe",
      "roles": ["Authenticated", "Editor"],
      "providers": ["google"],
      "createdAt": 1729771200000,
      "updatedAt": 1729771200000
    }
  ],
  "count": 1,
  "timestamp": "2025-10-24T12:00:00.000Z"
}
```

#### Grant a Role

**Endpoint:** `POST /engine/user_roles`

**Request Body** (JSON or form-encoded):

```json
{
  "user_id": "uuid-here",
  "role": "Editor"
}
```

`role` is one of `Editor`, `Administrator`, or `Authenticated`. Granting a role
the user already holds is a no-op rather than an error.

**Response:**

```json
{
  "success": true,
  "userId": "uuid-here",
  "role": "Editor",
  "roles": ["Authenticated", "Editor"],
  "timestamp": "2025-10-24T12:00:00.000Z"
}
```

#### Revoke a Role

**Endpoint:** `DELETE /engine/user_roles?user_id=<id>&role=<role>`

Responds with the same body as the grant endpoint. Two revocations are refused:
the `Authenticated` role (400), because it is the base role every user holds,
and the last remaining `Administrator` (409), because that would leave the
instance with nobody able to appoint another.

**Errors:** 400 (missing parameter or unknown role), 403 (not an administrator),
404 (no such user), 409 (last administrator).

#### MCP Tools

The same three operations are available to AI clients through the `/mcp`
endpoint as the native tools `list_users`, `add_user_role`, and
`remove_user_role`, with identical authorization and response shapes.

```bash
curl -X POST https://yourdomain.com/mcp \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"jsonrpc":"2.0","id":1,"method":"tools/call",
       "params":{"name":"add_user_role",
                 "arguments":{"user_id":"uuid-here","role":"Editor"}}}'
```

### Troubleshooting

**Problem:** 403 Forbidden from `/engine/users` or `/engine/user_roles`

**Solution:** Ensure you're calling as an administrator. Check:

1. You've signed in with a bootstrap admin email
2. Another admin has granted you the Administrator role
3. Your request carries the session cookie or bearer token
4. Check server logs for authentication errors

```bash
docker-compose logs aiwebengine | grep -i "admin\|authz"
```

Note that development mode's relaxed anonymous permissions do **not** apply
here: user administration always requires a real authenticated admin session.

**Problem:** Role Changes Not Persisting

**Solution:** Check server logs for errors:

```bash
docker-compose logs aiwebengine | grep -i error
```

Ensure the database is functioning correctly and user repository is accessible.

**Problem:** 409 Conflict when revoking the Administrator role

**Solution:** You're removing the last administrator, which would leave nobody
able to appoint another. Grant `Administrator` to a second user first, then
retry the revocation.

### Security Considerations

1. **Admin-Only Access:** All endpoints check for administrator capabilities
2. **No Client-Side Bypass:** Authorization is enforced server-side
3. **Audit Trail:** All role changes are logged with admin ID and timestamp
4. **No Password Exposure:** User credentials are never transmitted or displayed
5. **HTTPS Required:** Use HTTPS in production to protect session cookies

### Best Practices

✅ **DO:**

- Use HTTPS for all user administration in production
- Review role changes regularly in the audit log
- Limit the number of administrators (principle of least privilege)
- Keep administrator accounts secure with strong passwords on OAuth providers
- Log out when finished managing users

❌ **DON'T:**

- Share administrator accounts
- Grant Administrator role unnecessarily
- Administer users over insecure networks
- Leave administrator sessions open on shared computers

---

## Database Maintenance

### Database Backups

Backups have one home in this document: [Backup and Restore](#backup-and-restore)
below. The short version is that the stack takes them for you once
`COMPOSE_PROFILES` names the `backup` profile, and `make docker-backup` takes
one now.

### Database Maintenance Tasks

#### Vacuum and Analyze

```bash
# Manual vacuum and analyze
docker-compose exec postgres psql -U aiwebengine -d aiwebengine -c "VACUUM ANALYZE;"

# Check database size
docker-compose exec postgres psql -U aiwebengine -d aiwebengine -c "\l+"

# Check table sizes
docker-compose exec postgres psql -U aiwebengine -d aiwebengine -c "\dt+"
```

#### Check Connection Count

```bash
docker-compose exec postgres psql -U aiwebengine -d aiwebengine -c \
  "SELECT count(*) FROM pg_stat_activity WHERE datname='aiwebengine';"
```

#### Check Slow Queries

Enable slow query logging in PostgreSQL:

```sql
ALTER SYSTEM SET log_min_duration_statement = 1000;  -- Log queries > 1 second
SELECT pg_reload_conf();
```

### Database Migrations

**Production: Run migrations manually**

```bash
# Check current migration status
cargo run --bin migrate -- status

# Run pending migrations
cargo run --bin migrate -- up

# Rollback last migration (if needed)
cargo run --bin migrate -- down
```

**⚠️ Always backup before migrations!**

---

## Backup and Restore

### What a backup consists of

Three things, and the first two are both required:

1. **A dump of the database.** Scripts, assets, users, sessions, secrets, logs,
   revisions and deployment pins all live there. There is no other storage.
2. **The environment file** (`.env-production`, `.env-staging`). It holds
   `APP_SECURITY__SECRET_ENCRYPTION_KEY`, and every script and user secret in
   the dump is ciphertext without it. **A dump restored without that key comes
   back with its secrets unreadable** — the rest of the engine works, and every
   `getSecret` fails. Keep the file with the dumps and store the pair somewhere
   other than the host they came from.
3. **Caddy's data volume**, optionally. It holds the issued certificates. Losing
   it costs a re-issuance rather than data, so it is worth copying only if the
   deployment is near Let's Encrypt's rate limits.

Not on the list: `logs/`, `scripts/`, `data/`. No engine deployment writes to
any of them — the repository moved into Postgres, and logs go to stdout for
Docker to collect.

### Taking them

Scheduled, inside the stack:

```bash
# In the env file:
export COMPOSE_PROFILES=backup          # or ha,backup for a clustered deployment
export BACKUP_INTERVAL_SECONDS=86400    # daily
export BACKUP_KEEP=14
```

The `backup` service runs `pg_dump -Fc` on that interval into the `backup-data`
volume, keeping the newest `BACKUP_KEEP`. It runs the same image as the database
so the client and server versions cannot drift apart, and it is handed the same
`DATABASE_URL` expression as the engine containers, so it cannot end up dumping
a database nobody is using. Each dump is written under a temporary name and
renamed only once `pg_dump` has succeeded — a half-written file carrying the
final name is the one a restore would reach for.

By hand:

```bash
make docker-backup       ENV=production   # take one now
make docker-backup-list  ENV=production   # what the volume holds, newest first
make docker-backup-fetch ENV=production   # copy the newest to ./backups
```

**The dumps live on the machine running the Docker daemon.** They survive
`docker compose down`, and they are not a backup until they have left that
machine — `docker-backup-fetch` is the step that makes them one.

A deployment on a **managed database** should use the provider's backups
instead, and still keep the env file: the key is not in the provider's snapshot
either.

### Restoring

```bash
make docker-backup-list ENV=production
make docker-restore ENV=production FILE=aiwebengine-20260906T020000Z.dump CONFIRM=yes
```

The target stops the engine containers, restores, and starts them again. The
stop is not politeness: a restore drops and recreates every table, and doing
that under instances executing scripts against them leaves neither the dump nor
what was there before.

`pg_restore` runs with `--clean --if-exists`, so restoring over a populated
database replaces it rather than failing on every unique constraint, and with
`--exit-on-error`, because a restore that reports success having skipped half
the objects is the worst outcome available.

Restore the env file with it if the key has been lost. If the key differs from
the one the dump was taken under, the restore itself succeeds and every stored
secret is unreadable — check with something small before deciding the restore
worked.

### Rehearsing a restore

A backup nobody has restored is a hypothesis. Rehearse it somewhere that is not
production, at least once, and again after any change to the stack:

```bash
# 1. A throwaway environment: a copy of the production env file with a
#    different project name, and no hostnames anyone points at.
cp .env-production .env-rehearsal
sed -i.bak 's/^export COMPOSE_PROJECT_NAME=.*/export COMPOSE_PROJECT_NAME=aiwebengine-rehearsal/' .env-rehearsal
sed -i.bak 's/^export ENV_FILE=.*/export ENV_FILE=.env-rehearsal/' .env-rehearsal

# 2. Bring up just its database, and put the dump into it.
docker compose --env-file .env-rehearsal up -d postgres
make docker-restore ENV=rehearsal FILE=<the dump> CONFIRM=yes

# 3. Check what actually came back.
docker compose --env-file .env-rehearsal exec -T postgres \
  psql -U aiwebengine -d aiwebengine -c \
  "SELECT (SELECT count(*) FROM scripts) AS scripts,
          (SELECT count(*) FROM users)   AS users,
          (SELECT count(*) FROM assets)  AS assets;"

# 4. And that the secrets decrypt, which is the half a dump does not carry.
#    Start the engine against it and read one back through /engine.

# 5. Tear it down.
docker compose --env-file .env-rehearsal down -v
rm .env-rehearsal .env-rehearsal.bak
```

Step 4 is the one worth not skipping. Steps 1–3 pass with the wrong encryption
key.

### Desktop standalone

`.env-desktop` and the embedded data directory (`data/postgres` by default) are
the whole install. Back them up together, with the app stopped — copying a
running PostgreSQL's data directory produces a copy that may not start. The same
key caveat applies: the data directory without `.env-desktop` restores an engine
whose secrets are unreadable.

### Keeping them off the host

Any object store will do; the dumps are ordinary files. With `docker-backup-fetch`
having put one in `./backups`:

```bash
aws s3 cp backups/aiwebengine-20260906T020000Z.dump \
  s3://my-bucket/aiwebengine/production/
```

Store the env file separately from the dumps, not beside them. Together in one
bucket, a single credential leak is the whole engine — every secret, decrypted.

---

## Updates and Upgrades

### Updating aiwebengine

#### Pull Latest Code

```bash
# Backup first!
./backup-system.sh

# Pull latest
cd /path/to/aiwebengine
git fetch origin
git pull origin main

# Check for config changes
git diff HEAD@{1} config.toml
```

#### Rebuild and Deploy

```bash
# Rebuild Docker image
docker-compose build --no-cache

# Stop old version
docker-compose down

# Start new version
docker-compose up -d

# Check logs
docker-compose logs -f aiwebengine

# Verify health
curl https://yourdomain.com/health
```

#### Rollback if Needed

```bash
# Go back to previous version
git reset --hard HEAD@{1}

# Rebuild and restart
docker-compose build
docker-compose up -d
```

### System Updates

```bash
# Update server OS
sudo apt update
sudo apt upgrade -y

# Update Docker
sudo apt install docker-ce docker-ce-cli containerd.io

# Update Docker Compose
sudo apt install docker-compose-plugin

# Restart services after OS updates
sudo reboot
```

### Dependency Updates

```bash
# Update Rust dependencies
cargo update

# Check for outdated dependencies
cargo outdated

# Update specific dependency
cargo update <package-name>

# Rebuild
cargo build --release
```

---

## Performance Monitoring

### Resource Usage

```bash
# Docker resource usage
docker stats

# Container-specific stats
docker stats aiwebengine

# Server resources
htop
# or
top
```

### Database Performance

```bash
# Active connections
docker-compose exec postgres psql -U aiwebengine -d aiwebengine -c \
  "SELECT count(*) FROM pg_stat_activity;"

# Slow queries
docker-compose exec postgres psql -U aiwebengine -d aiwebengine -c \
  "SELECT query, mean_exec_time, calls FROM pg_stat_statements ORDER BY mean_exec_time DESC LIMIT 10;"

# Database size growth
docker-compose exec postgres psql -U aiwebengine -d aiwebengine -c \
  "SELECT pg_size_pretty(pg_database_size('aiwebengine'));"
```

### Application Metrics

Check logs for:

- Request latency
- Error rates
- Memory usage
- Connection pool usage

### Performance Tuning

**Increase connection pool** (high traffic):

```toml
[repository]
max_connections = 100  # Increase from 50
```

**Enable compression** (bandwidth):

```toml
[performance]
enable_compression = true
```

**Increase cache** (repeated requests):

```toml
[performance]
cache_size_mb = 512
cache_ttl_seconds = 3600
```

**Add worker threads** (CPU-bound):

```toml
[performance]
worker_pool_size = 16  # Match CPU cores
```

---

## Maintenance Schedules

### Daily

- [ ] Check health endpoint
- [ ] Review error logs
- [ ] Monitor disk space
- [ ] Check backup completion

### Weekly

- [ ] Review full logs
- [ ] Check database size
- [ ] Analyze slow queries
- [ ] Review performance metrics
- [ ] Test backup restore procedure

### Monthly

- [ ] Update dependencies
- [ ] Review and rotate logs
- [ ] Database maintenance (vacuum, analyze)
- [ ] Security updates
- [ ] Review and update documentation

### Quarterly

- [ ] Rotate secrets (JWT, API keys)
- [ ] Review access controls
- [ ] Performance optimization review
- [ ] Disaster recovery drill
- [ ] Update dependencies (Rust, Docker images)

---

## Maintenance Scripts

Create a maintenance directory:

```bash
mkdir -p /opt/aiwebengine/maintenance
```

**health-check.sh** - Monitor health  
**backup-database.sh** - Database backups  
**backup-system.sh** - Full system backup  
**cleanup-logs.sh** - Remove old logs  
**check-resources.sh** - Resource monitoring

Schedule via cron:

```bash
# Edit crontab
crontab -e

# Add maintenance tasks
*/5 * * * * /opt/aiwebengine/maintenance/health-check.sh
0 2 * * * /opt/aiwebengine/maintenance/backup-database.sh
0 3 * * 0 /opt/aiwebengine/maintenance/backup-system.sh
0 4 * * * /opt/aiwebengine/maintenance/cleanup-logs.sh
```

---

## Related Documentation

- **[Getting Started](01-GETTING-STARTED.md)** - Initial setup
- **[Configuration](02-CONFIGURATION.md)** - Config options
- **[Running Environments](03-RUNNING-ENVIRONMENTS.md)** - Deployment guides
- **[Troubleshooting](06-TROUBLESHOOTING.md)** - Problem solving
- **[Quick Reference](QUICK-REFERENCE.md)** - Command cheat sheet
