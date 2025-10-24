# 06 - Troubleshooting

Common issues and solutions for aiwebengine deployment and operation.

## Quick Navigation

- [Installation Issues](#installation-issues)
- [Server Won't Start](#server-wont-start)
- [Authentication Problems](#authentication-problems)
- [Database Issues](#database-issues)
- [Docker Problems](#docker-problems)
- [Performance Issues](#performance-issues)
- [Network and HTTPS Issues](#network-and-https-issues)

---

## Installation Issues

### Rust Installation Fails

**Symptom:** `curl` command fails or rustup errors

**Solutions:**

```bash
# Try alternative installation
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- --default-toolchain stable

# Or use package manager (macOS)
brew install rust

# Update existing Rust
rustup update
```

### Cargo Build Fails

**Symptom:** Compilation errors or missing dependencies

**Solutions:**

```bash
# Clean and rebuild
cargo clean
cargo build --release

# Update dependencies
cargo update

# Install required system libraries (Ubuntu)
sudo apt install build-essential pkg-config libssl-dev

# Install required libraries (macOS)
brew install openssl pkg-config
```

### Docker Installation Issues

**Symptom:** Docker command not found or permission denied

**Solutions:**

```bash
# Install Docker (Ubuntu)
curl -fsSL https://get.docker.com | sh
sudo usermod -aG docker $USER
newgrp docker

# Install Docker (macOS)
brew install --cask docker

# Fix permissions
sudo chmod 666 /var/run/docker.sock

# Restart Docker daemon
sudo systemctl restart docker
```

---

## Server Won't Start

### Port Already in Use

**Symptom:** "Address already in use (os error 48/98)"

**Solutions:**

```bash
# Find process using port 3000
lsof -i :3000
# Or on Linux
sudo netstat -tulpn | grep :3000

# Kill the process
kill -9 <PID>

# Or change port in config.toml
[server]
port = 3001

# Restart with new port
docker-compose down
docker-compose up -d
```

### Configuration Validation Failed

**Symptom:** "Configuration validation failed: ..." on startup

**Solutions:**

```bash
# Check which configuration is being used
ls -la config.toml

# Validate configuration
cargo run -- --validate-config

# Common issues:
# 1. JWT secret too short
export APP_AUTH__JWT_SECRET="$(openssl rand -base64 48)"

# 2. Invalid log level
export APP_LOGGING__LEVEL="info"  # Must be: trace, debug, info, warn, error

# 3. Invalid port
export APP_SERVER__PORT="3000"  # Must be 1-65535

# Re-check
cargo run -- --validate-config
```

### Environment Variables Not Loaded

**Symptom:** Configuration defaults being used instead of environment variables

**Solutions:**

```bash
# Check if .env file exists
ls -la .env

# Load environment variables
source .env

# Verify variables are set
env | grep APP_

# For Docker, check docker-compose.yml has env_file:
cat docker-compose.yml | grep env_file

# Or pass environment inline
docker-compose up --env-file .env
```

### Permission Denied Errors

**Symptom:** Cannot write logs or access directories

**Solutions:**

```bash
# Create directories with proper permissions
mkdir -p logs data scripts
chmod 755 logs data scripts

# For Docker
sudo chown -R $USER:$USER logs data scripts

# Check user in container
docker-compose exec aiwebengine whoami
docker-compose exec aiwebengine ls -la /app/logs
```

---

## Authentication Problems

### OAuth Redirect URI Mismatch

**Symptom:** "redirect_uri_mismatch" after sign-in attempt

**Solutions:**

```bash
# 1. Check configured redirect URI
echo $APP_AUTH__PROVIDERS__GOOGLE__REDIRECT_URI

# 2. Verify it matches EXACTLY in OAuth provider console
# Local: http://localhost:3000/auth/callback/google
# Prod: https://yourdomain.com/auth/callback/google

# 3. Common mistakes:
# - http vs https
# - Wrong domain or subdomain
# - Missing or extra trailing slash
# - Wrong port number

# 4. Update if needed
export APP_AUTH__PROVIDERS__GOOGLE__REDIRECT_URI="http://localhost:3000/auth/callback/google"

# 5. Restart server
docker-compose restart aiwebengine
```

### JWT Secret Error

**Symptom:** "JWT secret must be at least 32 characters"

**Solutions:**

```bash
# Generate proper secret (48 bytes base64 = 64 characters)
export APP_AUTH__JWT_SECRET="$(openssl rand -base64 48)"

# Verify it's set
echo $APP_AUTH__JWT_SECRET | wc -c  # Should be 64+

# Restart server
docker-compose restart aiwebengine
```

### User Not Getting Admin Role

**Symptom:** User signs in but doesn't have administrator access

**Solutions:**

```bash
# 1. Check bootstrap_admins configuration
grep bootstrap_admins config.toml

# 2. Verify email matches OAuth account exactly
# Sign out and check what email is being used

# 3. Update bootstrap_admins
export APP_AUTH__BOOTSTRAP_ADMINS='["your-actual-email@gmail.com"]'

# 4. Or edit config.toml
[auth]
bootstrap_admins = ["your-actual-email@gmail.com"]

# 5. Restart and sign in again
docker-compose restart aiwebengine
```

### OAuth Provider Returns Error

**Symptom:** "invalid_client" or "unauthorized_client"

**Solutions:**

```bash
# 1. Verify Client ID and Secret are correct
echo $APP_AUTH__PROVIDERS__GOOGLE__CLIENT_ID
echo $APP_AUTH__PROVIDERS__GOOGLE__CLIENT_SECRET

# 2. Check OAuth consent screen is configured
# In Google Console: OAuth consent screen must be set up

# 3. Verify OAuth credentials are for correct environment
# Don't use production credentials in development

# 4. Check OAuth app is not suspended or restricted

# 5. Regenerate credentials if needed
```

### Session Expired Immediately

**Symptom:** User signed in but session expires right away

**Solutions:**

```bash
# 1. Check system clock is synchronized
date
sudo ntpdate time.google.com  # Linux
sudo sntp -sS time.apple.com  # macOS

# 2. Check session timeout configuration
[auth]
session_timeout = 3600  # 1 hour in seconds

# 3. Check cookies are being set (browser dev tools)
# Should see: aiwebengine_session cookie

# 4. Verify secure flag matches HTTPS usage
[auth.cookie]
secure = false  # false for http, true for https

# 5. Check SameSite attribute
same_site = "lax"  # or "strict" or "none"
```

---

## Database Issues

### Connection Refused

**Symptom:** "Connection refused" or "could not connect to server"

**Solutions:**

```bash
# Check PostgreSQL is running (Docker)
docker-compose ps postgres

# Check PostgreSQL logs
docker-compose logs postgres

# Verify connection string
echo $APP_REPOSITORY__DATABASE_URL

# Test connection manually
docker-compose exec postgres psql -U aiwebengine -d aiwebengine

# Restart PostgreSQL
docker-compose restart postgres

# Check network
docker network ls
docker network inspect aiwebengine_default
```

### Role/Database Does Not Exist

**Symptom:** "role 'aiwebengine' does not exist" or "database 'aiwebengine' does not exist"

**Solutions:**

```bash
# Create database and user
docker-compose exec postgres psql -U postgres -c \
  "CREATE USER aiwebengine WITH PASSWORD 'your-password';"

docker-compose exec postgres psql -U postgres -c \
  "CREATE DATABASE aiwebengine OWNER aiwebengine;"

# Grant privileges
docker-compose exec postgres psql -U postgres -c \
  "GRANT ALL PRIVILEGES ON DATABASE aiwebengine TO aiwebengine;"

# Verify
docker-compose exec postgres psql -U aiwebengine -d aiwebengine -c "SELECT 1;"
```

### Migration Failures

**Symptom:** "Migration failed" or schema errors

**Solutions:**

```bash
# Check migration status
cargo run --bin migrate -- status

# Backup database first!
docker-compose exec postgres pg_dump -U aiwebengine aiwebengine > backup-before-migration.sql

# Try migration again
cargo run --bin migrate -- up

# If still failing, check logs
docker-compose logs aiwebengine | grep -i migration

# Reset migrations (DESTRUCTIVE - only in development!)
cargo run --bin migrate -- reset
cargo run --bin migrate -- up
```

### Too Many Connections

**Symptom:** "sorry, too many clients already" or "remaining connection slots are reserved"

**Solutions:**

```bash
# Check current connections
docker-compose exec postgres psql -U aiwebengine -d aiwebengine -c \
  "SELECT count(*) FROM pg_stat_activity WHERE datname='aiwebengine';"

# Check max connections
docker-compose exec postgres psql -U postgres -c \
  "SHOW max_connections;"

# Kill idle connections
docker-compose exec postgres psql -U aiwebengine -d aiwebengine -c \
  "SELECT pg_terminate_backend(pid) FROM pg_stat_activity
   WHERE datname='aiwebengine' AND state='idle' AND state_change < now() - interval '5 minutes';"

# Increase connection pool in config.toml
[repository]
max_connections = 50  # Increase if needed

# Increase PostgreSQL max_connections (docker-compose.yml)
services:
  postgres:
    command: postgres -c max_connections=200
```

---

## Docker Problems

### Container Won't Start

**Symptom:** Container exits immediately or shows "unhealthy"

**Solutions:**

```bash
# Check container logs
docker-compose logs aiwebengine

# Check container status
docker-compose ps

# Inspect container
docker inspect $(docker-compose ps -q aiwebengine)

# Try starting in foreground to see errors
docker-compose up aiwebengine

# Check for port conflicts
docker-compose down
docker-compose up
```

### Container Out of Memory

**Symptom:** Container killed or OOMKilled status

**Solutions:**

```bash
# Check Docker resource limits
docker stats

# Increase memory limit in docker-compose.yml
services:
  aiwebengine:
    deploy:
      resources:
        limits:
          memory: 2G
        reservations:
          memory: 512M

# Restart with new limits
docker-compose up -d
```

### Volume Mount Issues

**Symptom:** Files not visible or permission denied in container

**Solutions:**

```bash
# Check volume mounts
docker-compose config | grep -A 5 volumes

# Verify files exist on host
ls -la logs/ data/ scripts/

# Fix permissions
chmod -R 755 logs/ data/ scripts/
chown -R $USER:$USER logs/ data/ scripts/

# Recreate volumes
docker-compose down -v
docker-compose up -d
```

### Image Build Fails

**Symptom:** Docker build errors or timeouts

**Solutions:**

```bash
# Clean Docker cache
docker system prune -a

# Build with no cache
docker-compose build --no-cache

# Check Dockerfile syntax
docker build -f Dockerfile --target builder .

# Increase Docker resources (Docker Desktop)
# Settings → Resources → Increase memory/CPU
```

---

## Performance Issues

### Slow Response Times

**Symptom:** Requests taking too long to complete

**Solutions:**

```bash
# 1. Check resource usage
docker stats aiwebengine

# 2. Check database performance
docker-compose exec postgres psql -U aiwebengine -d aiwebengine -c \
  "SELECT pid, now() - query_start AS duration, query
   FROM pg_stat_activity
   WHERE query != '<IDLE>' ORDER BY duration DESC;"

# 3. Increase worker threads
[performance]
worker_pool_size = 8  # Match CPU cores

# 4. Enable caching
[performance]
cache_size_mb = 256
enable_compression = true

# 5. Increase database connections
[repository]
max_connections = 50

# 6. Check logs for slow operations
docker-compose logs aiwebengine | grep -i "slow\|timeout"
```

### High Memory Usage

**Symptom:** Container using excessive memory

**Solutions:**

```bash
# Check memory usage
docker stats aiwebengine

# Reduce JavaScript memory limit
[javascript]
max_memory_mb = 64  # Reduce from 128

# Reduce cache size
[performance]
cache_size_mb = 128  # Reduce from 512

# Check for memory leaks in logs
docker-compose logs aiwebengine | grep -i "memory\|oom"

# Restart container to clear memory
docker-compose restart aiwebengine
```

### High CPU Usage

**Symptom:** CPU constantly at 100%

**Solutions:**

```bash
# Check what's consuming CPU
docker stats
top -p $(docker inspect -f '{{.State.Pid}}' $(docker-compose ps -q aiwebengine))

# Check for infinite loops in JavaScript
docker-compose logs aiwebengine | tail -100

# Reduce execution timeout to prevent long-running scripts
[javascript]
execution_timeout_ms = 5000  # Reduce from 10000

# Check database queries
docker-compose exec postgres psql -U aiwebengine -d aiwebengine -c \
  "SELECT query, state FROM pg_stat_activity WHERE state != 'idle';"
```

---

## Network and HTTPS Issues

### Cannot Access Server

**Symptom:** Connection refused or timeout

**Solutions:**

```bash
# 1. Check server is running
curl http://localhost:3000/health

# 2. Check firewall
sudo ufw status
sudo ufw allow 80/tcp
sudo ufw allow 443/tcp

# 3. Check port binding
docker-compose ps
netstat -tulpn | grep :3000

# 4. Check host binding in config
[server]
host = "0.0.0.0"  # Not "127.0.0.1" for external access

# 5. Restart with correct configuration
docker-compose down
docker-compose up -d
```

### HTTPS Certificate Issues

**Symptom:** SSL certificate errors or "Not Secure" warning

**Solutions:**

```bash
# 1. Check Caddy logs
docker-compose logs caddy | grep -i cert

# 2. Verify DNS is configured
dig yourdomain.com +short

# 3. Check Caddyfile configuration
cat Caddyfile.production

# 4. Force certificate renewal
docker-compose restart caddy

# 5. Check certificate details
echo | openssl s_client -connect yourdomain.com:443 -servername yourdomain.com 2>/dev/null | openssl x509 -noout -text

# 6. Verify ports are open
sudo ufw status | grep -E '80|443'

# 7. Test Let's Encrypt connectivity
curl -I https://acme-v02.api.letsencrypt.org/directory
```

### CORS Errors

**Symptom:** "CORS policy" errors in browser console

**Solutions:**

```bash
# 1. Check CORS configuration
[security]
enable_cors = true
cors_origins = ["https://yourdomain.com"]

# 2. Add your domain to allowed origins
export APP_SECURITY__CORS_ORIGINS='["https://yourdomain.com","https://www.yourdomain.com"]'

# 3. For local development, allow all origins
cors_origins = ["*"]  # Development only!

# 4. Restart server
docker-compose restart aiwebengine

# 5. Check response headers
curl -I -H "Origin: https://yourdomain.com" https://yourdomain.com/api/endpoint
```

### Redirect Loop

**Symptom:** Browser shows "too many redirects"

**Solutions:**

```bash
# Check Caddyfile for redirect configuration
cat Caddyfile.production

# Common issue: double redirect (Caddy + config)
# Either let Caddy handle redirects OR configure in app, not both

# Check require_https setting
[security]
require_https = false  # Let Caddy handle HTTPS

# Restart
docker-compose restart
```

---

## Emergency Procedures

### Complete System Reset (Development Only!)

**⚠️ This will delete all data!**

```bash
# Stop all services
docker-compose down

# Remove all data
docker-compose down -v
rm -rf data/ logs/*

# Rebuild from scratch
docker-compose build --no-cache
cp config.local.toml config.toml
cp .env.example .env
# Edit .env with credentials

# Start fresh
docker-compose up -d
```

### Restore from Backup

```bash
# Stop services
docker-compose down

# Restore database
gunzip < backup-20251024-120000.sql.gz | \
  docker-compose exec -T postgres psql -U aiwebengine -d aiwebengine

# Restore configuration
cp /backup/config.toml .
cp /backup/.env .

# Restart services
docker-compose up -d

# Verify
curl http://localhost:3000/health
```

---

## Getting More Help

### Collect Debugging Information

When reporting issues, include:

```bash
# 1. Version information
git log -1 --oneline
docker --version
docker-compose --version

# 2. Configuration (remove secrets!)
cat config.toml | grep -v "secret\|password\|key"

# 3. Logs
docker-compose logs --tail=100 aiwebengine > debug-logs.txt

# 4. Container status
docker-compose ps > debug-status.txt

# 5. Resource usage
docker stats --no-stream > debug-resources.txt

# 6. System information
uname -a > debug-system.txt
```

### Enable Debug Logging

```bash
# Temporary debug mode
export APP_LOGGING__LEVEL="debug"
export RUST_LOG="aiwebengine=debug"
export RUST_BACKTRACE=1

# Restart
docker-compose restart aiwebengine

# View detailed logs
docker-compose logs -f aiwebengine
```

### Contact Support

- **GitHub Issues:** [github.com/lpajunen/aiwebengine/issues](https://github.com/lpajunen/aiwebengine/issues)
- **Documentation:** [Complete docs](../INDEX.md)
- **Quick Reference:** [QUICK-REFERENCE.md](QUICK-REFERENCE.md)

---

## Related Documentation

- **[Getting Started](01-GETTING-STARTED.md)** - Setup guide
- **[Configuration](02-CONFIGURATION.md)** - Config reference
- **[Running Environments](03-RUNNING-ENVIRONMENTS.md)** - Deployment
- **[Secrets and Security](04-SECRETS-AND-SECURITY.md)** - Security
- **[Monitoring and Maintenance](05-MONITORING-AND-MAINTENANCE.md)** - Operations
