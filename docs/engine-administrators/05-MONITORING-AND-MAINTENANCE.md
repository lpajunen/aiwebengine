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

#### Docker PostgreSQL Backup

```bash
# Create backup
docker-compose exec postgres pg_dump -U aiwebengine aiwebengine | \
  gzip > backup-$(date +%Y%m%d-%H%M%S).sql.gz

# List backups
ls -lh backup-*.sql.gz
```

#### Automated Backup Script

```bash
#!/bin/bash
# backup-database.sh

BACKUP_DIR="/var/backups/aiwebengine"
RETENTION_DAYS=30

# Create backup directory
mkdir -p "$BACKUP_DIR"

# Create backup
docker-compose exec -T postgres pg_dump -U aiwebengine aiwebengine | \
  gzip > "$BACKUP_DIR/backup-$(date +%Y%m%d-%H%M%S).sql.gz"

# Remove old backups
find "$BACKUP_DIR" -name "backup-*.sql.gz" -mtime +$RETENTION_DAYS -delete

echo "Backup completed: $(date)"
```

**Schedule with cron** (daily at 2 AM):

```bash
0 2 * * * /path/to/backup-database.sh >> /var/log/backup.log 2>&1
```

### Database Restore

```bash
# Restore from backup
gunzip < backup-20251024-120000.sql.gz | \
  docker-compose exec -T postgres psql -U aiwebengine -d aiwebengine

# Or restore to a specific point
gunzip < backup-20251024-120000.sql.gz | \
  docker-compose exec -T postgres psql -U aiwebengine -d aiwebengine_restore
```

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

### Complete System Backup

Backup all critical data:

```bash
#!/bin/bash
# backup-system.sh

BACKUP_DIR="/var/backups/aiwebengine-full"
TIMESTAMP=$(date +%Y%m%d-%H%M%S)

mkdir -p "$BACKUP_DIR/$TIMESTAMP"

# 1. Database
docker-compose exec -T postgres pg_dump -U aiwebengine aiwebengine | \
  gzip > "$BACKUP_DIR/$TIMESTAMP/database.sql.gz"

# 2. Configuration
cp config.toml "$BACKUP_DIR/$TIMESTAMP/"
cp .env "$BACKUP_DIR/$TIMESTAMP/" 2>/dev/null || true

# 3. Docker volumes
docker run --rm \
  -v aiwebengine_caddy-data:/data \
  -v "$BACKUP_DIR/$TIMESTAMP":/backup \
  alpine tar czf /backup/caddy-data.tar.gz /data

# 4. Logs (last 7 days)
tar czf "$BACKUP_DIR/$TIMESTAMP/logs.tar.gz" \
  --mtime=-7 logs/

# 5. Scripts
tar czf "$BACKUP_DIR/$TIMESTAMP/scripts.tar.gz" scripts/

echo "Backup completed: $BACKUP_DIR/$TIMESTAMP"
```

### Restore from Backup

```bash
#!/bin/bash
# restore-system.sh

BACKUP_PATH="/var/backups/aiwebengine-full/20251024-120000"

# 1. Stop services
docker-compose down

# 2. Restore database
gunzip < "$BACKUP_PATH/database.sql.gz" | \
  docker-compose exec -T postgres psql -U aiwebengine -d aiwebengine

# 3. Restore configurations
cp "$BACKUP_PATH/config.toml" .
cp "$BACKUP_PATH/.env" . 2>/dev/null || true

# 4. Restore Caddy certificates
docker run --rm \
  -v aiwebengine_caddy-data:/data \
  -v "$BACKUP_PATH":/backup \
  alpine tar xzf /backup/caddy-data.tar.gz -C /

# 5. Restore logs
tar xzf "$BACKUP_PATH/logs.tar.gz"

# 6. Restore scripts
tar xzf "$BACKUP_PATH/scripts.tar.gz"

# 7. Restart services
docker-compose up -d

echo "Restore completed"
```

### Backup to Cloud Storage

#### AWS S3

```bash
# Upload backup
aws s3 cp backup-20251024-120000.sql.gz \
  s3://my-bucket/aiwebengine-backups/

# Download backup
aws s3 cp s3://my-bucket/aiwebengine-backups/backup-20251024-120000.sql.gz .
```

#### Automated S3 Backup

```bash
#!/bin/bash
# backup-to-s3.sh

BUCKET="s3://my-bucket/aiwebengine-backups"
TIMESTAMP=$(date +%Y%m%d-%H%M%S)

# Create backup
docker-compose exec -T postgres pg_dump -U aiwebengine aiwebengine | \
  gzip | aws s3 cp - "$BUCKET/backup-$TIMESTAMP.sql.gz"

# Cleanup old backups (keep last 30 days)
aws s3 ls "$BUCKET/" | while read -r line; do
    file=$(echo $line | awk '{print $4}')
    file_date=$(echo $file | grep -oP '\d{8}')
    if [ $(($(date +%s) - $(date -d $file_date +%s))) -gt 2592000 ]; then
        aws s3 rm "$BUCKET/$file"
    fi
done
```

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
