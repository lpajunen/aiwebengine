# Quick Reference

Fast lookup guide for common commands, environment variables, and operations.

## Quick Navigation

- [Common Commands](#common-commands)
- [Environment Variables](#environment-variables)
- [Configuration Files](#configuration-files)
- [Docker Commands](#docker-commands)
- [Makefile Targets](#makefile-targets)
- [Health Checks](#health-checks)
- [Secrets Generation](#secrets-generation)

---

## Common Commands

### Local Development

```bash
# Copy config and environment
cp config.local.toml config.toml
cp .env.example .env

# Edit .env with your credentials
nano .env

# Start server
source .env && cargo run

# Start with Docker
make docker-local
```

### Staging Deployment

```bash
# Use staging config
cp config.staging.toml config.toml

# Set environment variables
export APP_AUTH__JWT_SECRET="$(openssl rand -base64 48)"
export APP_REPOSITORY__DATABASE_URL="postgresql://user:pass@db/aiwebengine"

# Deploy
cargo run --release
# Or with Docker
make docker-staging
```

### Production Deployment

```bash
# Use production config
cp config.production.toml config.toml

# Set all secrets via environment (NEVER in files!)
export APP_AUTH__JWT_SECRET="$(openssl rand -base64 48)"
export APP_SECURITY__API_KEY="$(openssl rand -hex 32)"
export APP_SECURITY__CSRF_KEY="$(openssl rand -base64 32)"
export APP_SECURITY__SESSION_ENCRYPTION_KEY="$(openssl rand -base64 32)"
export APP_REPOSITORY__DATABASE_URL="postgresql://user:pass@host/db"

# Deploy
cargo run --release
# Or with Docker
make docker-prod
```

---

## Environment Variables

### Critical Variables

| Variable                                     | Purpose                      | How to Generate            |
| -------------------------------------------- | ---------------------------- | -------------------------- |
| `APP_AUTH__JWT_SECRET`                       | JWT signing key              | `openssl rand -base64 48`  |
| `APP_SECURITY__API_KEY`                      | API authentication           | `openssl rand -hex 32`     |
| `APP_SECURITY__CSRF_KEY`                     | CSRF protection              | `openssl rand -base64 32`  |
| `APP_SECURITY__SESSION_ENCRYPTION_KEY`       | Session encryption           | `openssl rand -base64 32`  |
| `APP_REPOSITORY__DATABASE_URL`               | Database connection          | Manual setup               |
| `APP_AUTH__PROVIDERS__GOOGLE__CLIENT_ID`     | Google OAuth                 | Google Console             |
| `APP_AUTH__PROVIDERS__GOOGLE__CLIENT_SECRET` | Google OAuth                 | Google Console             |

### Naming Convention

```bash
# Format: APP__{SECTION}__{SUBSECTION}__{KEY}
# Double underscores (__) represent nested config structure

# Example: [auth] jwt_secret
export APP_AUTH__JWT_SECRET="value"

# Example: [auth.providers.google] client_id
export APP_AUTH__PROVIDERS__GOOGLE__CLIENT_ID="value"

# Example: [server] host
export APP_SERVER__HOST="0.0.0.0"

# Example: [logging] level
export APP_LOGGING__LEVEL="info"
```

### Common Configuration Overrides

```bash
# Server
export APP_SERVER__HOST="0.0.0.0"
export APP_SERVER__PORT="8080"
export APP_SERVER__BASE_URL="https://yourdomain.com"

# Logging
export APP_LOGGING__LEVEL="info"              # trace, debug, info, warn, error
export APP_LOGGING__FORMAT="json"             # json, pretty, compact
export APP_LOGGING__FILE_PATH="/var/log/aiwebengine.log"

# Database
export APP_REPOSITORY__DATABASE_URL="postgresql://user:pass@host/db"

# Security
export APP_SECURITY__ENABLE_CSRF="true"
export APP_SECURITY__CSRF_KEY="$(openssl rand -base64 32)"
export APP_SECURITY__SESSION_ENCRYPTION_KEY="$(openssl rand -base64 32)"
export APP_SECURITY__CORS_ALLOWED_ORIGINS='["https://yourdomain.com"]'

# Bootstrap Admins (JSON array format)
export APP_AUTH__BOOTSTRAP_ADMINS='["admin@example.com"]'
```

### Secrets (AI Integration)

```bash
# Format: SECRET_{IDENTIFIER}
# Identifier becomes lowercase: SECRET_ANTHROPIC_API_KEY → anthropic_api_key

# Anthropic Claude
export SECRET_ANTHROPIC_API_KEY="sk-ant-api03-..."

# OpenAI
export SECRET_OPENAI_API_KEY="sk-..."

# Custom services
export SECRET_STRIPE_API_KEY="sk_live_..."
export SECRET_SENDGRID_API_KEY="SG..."
```

---

## Configuration Files

### Quick Selection

```bash
# Local Development
cp config.local.toml config.toml

# Staging
cp config.staging.toml config.toml

# Production
cp config.production.toml config.toml
```

### Key Differences

| Setting                     | Local   | Staging  | Production |
| --------------------------- | ------- | -------- | ---------- |
| `logging.level`             | `debug` | `info`   | `info`     |
| `security.enable_csrf`      | `false` | `true`   | `true`     |
| `security.cors_allowed_origins` | `["*"]` | Specific | Specific   |
| Session cookie `secure`     | `false` | `true`   | `true`     |

---

## Docker Commands

### Using Makefile (Recommended)

```bash
# Build and start
make docker-local        # Local development
make docker-staging      # Staging
make docker-prod         # Production

# Management
make docker-stop         # Stop all containers
make docker-logs         # View production logs
make docker-logs-local   # View local logs
make docker-clean        # Remove containers and volumes
make docker-shell        # Access container shell

# Setup
make docker-setup        # First-time setup (env + build)
```

### Direct Docker Compose

```bash
# Local
docker-compose -f docker-compose.local.yml up -d
docker-compose -f docker-compose.local.yml logs -f

# Staging
docker-compose -f docker-compose.staging.yml up -d
docker-compose -f docker-compose.staging.yml logs -f

# Production
docker-compose up -d
docker-compose logs -f aiwebengine

# Stop
docker-compose down
```

### Container Management

```bash
# Check status
docker-compose ps

# Restart service
docker-compose restart aiwebengine

# View logs (last 100 lines)
docker-compose logs --tail=100 aiwebengine

# Follow logs
docker-compose logs -f aiwebengine

# Access shell
docker-compose exec aiwebengine /bin/bash

# Check resource usage
docker stats
```

---

## Makefile Targets

### Development

```bash
make deps          # Install dev tools (cargo-watch, nextest, etc.)
make dev           # Run with auto-reload
make test          # Run tests
make lint          # Run clippy
make format        # Format code
make check         # Run all pre-commit checks
```

### Building

```bash
make build         # Build release binary
make clean         # Clean build artifacts
```

### Docker

```bash
make docker-setup  # First-time Docker setup
make docker-build  # Build production image
make docker-prod   # Start production
make docker-stop   # Stop all
make docker-clean  # Clean everything
```

---

## Health Checks

### HTTP Endpoints

```bash
# Health check
curl http://localhost:3000/health

# Expected response
{"status":"ok","timestamp":"2025-10-24T..."}

# HTTPS (production)
curl https://yourdomain.com/health
```

### Container Health

```bash
# Docker health status
docker-compose ps

# Should show "healthy" status
# If "unhealthy", check logs:
docker-compose logs aiwebengine
```

### Database Connection

```bash
# Check PostgreSQL
docker-compose exec postgres psql -U aiwebengine -d aiwebengine -c "SELECT 1;"

# Should return:
#  ?column?
# ----------
#         1
```

---

## Secrets Generation

### JWT Secret (48 bytes, base64)

```bash
openssl rand -base64 48
```

Example output: `7rXk+mN5...` (64 characters)

### API Key (32 bytes, hex)

```bash
openssl rand -hex 32
```

Example output: `a3f9e2c1...` (64 characters)

### CSRF Key (32 bytes, base64)

```bash
openssl rand -base64 32
```

Example output: `8kDp5Nc...` (44 characters)

**Important:** All server instances must use the same CSRF key!

### Session Encryption Key (32 bytes, base64)

```bash
openssl rand -base64 32
```

Example output: `Xq8nPm...` (44 characters)

**Important:** All server instances must use the same session encryption key!

### Database Password (strong random)

```bash
openssl rand -base64 24
```

Example output: `Yp9oQn...` (32 characters)

---

## Common File Paths

### Configuration

```plaintext
config.local.toml           # Local config template
config.staging.toml         # Staging config template
config.production.toml      # Production config template
config.toml                 # Active config (not in Git)
.env                        # Environment variables (not in Git)
.env.example                # Environment template (in Git)
```

### Data & Logs

```plaintext
logs/                       # Application logs
data/                       # Database and data files
scripts/                    # JavaScript scripts
```

### Docker

```plaintext
Dockerfile                  # Production image
Dockerfile.local            # Local/dev image
Dockerfile.staging          # Staging image
docker-compose.yml          # Production services
docker-compose.local.yml    # Local services
docker-compose.staging.yml  # Staging services
Caddyfile.production        # Production reverse proxy config
```

---

## Quick Troubleshooting

### Server Won't Start

```bash
# Check logs
docker-compose logs aiwebengine

# Check ports
sudo lsof -i :3000

# Check config
cargo run -- --validate-config
```

### Database Connection Failed

```bash
# Check PostgreSQL status
docker-compose ps postgres

# Check connection string
echo $APP_REPOSITORY__DATABASE_URL

# Test connection
docker-compose exec postgres psql -U aiwebengine -d aiwebengine
```

### OAuth Not Working

```bash
# Check credentials are set
echo $APP_AUTH__PROVIDERS__GOOGLE__CLIENT_ID
echo $APP_AUTH__PROVIDERS__GOOGLE__CLIENT_SECRET

# Check redirect URI matches Google Console
# Local: http://localhost:3000/auth/callback/google
# Prod: https://yourdomain.com/auth/callback/google
```

### Container Not Healthy

```bash
# Check health status
docker inspect $(docker-compose ps -q aiwebengine) | grep -A 5 Health

# Force restart
docker-compose restart aiwebengine

# Check logs for errors
docker-compose logs --tail=50 aiwebengine | grep -i error
```

---

## URLs and Ports

### Default Ports

| Service     | Port | URL                                  |
| ----------- | ---- | ------------------------------------ |
| aiwebengine | 3000 | `http://localhost:3000`              |
| PostgreSQL  | 5432 | `postgresql://localhost:5432`        |
| Grafana     | 3001 | `http://localhost:3001` (if enabled) |
| Prometheus  | 9090 | `http://localhost:9090` (if enabled) |

### Important Endpoints

```plaintext
/health                     # Health check
/auth/login                 # OAuth login page
/auth/callback/google       # Google OAuth callback
/engine/admin               # Admin management UI
/editor                     # Script editor (solution developers)
/graphql                    # GraphQL endpoint (if enabled)
```

---

## Useful One-Liners

### Check if server is running

```bash
curl -f http://localhost:3000/health && echo "✓ Server is running" || echo "✗ Server is down"
```

### View errors in logs

```bash
docker-compose logs --tail=100 aiwebengine | grep -i error
```

### Count database connections

```bash
docker-compose exec postgres psql -U aiwebengine -d aiwebengine -c "SELECT count(*) FROM pg_stat_activity WHERE datname='aiwebengine';"
```

### Check disk space

```bash
df -h | grep -E '/$|/var'
```

### Check Docker resource usage

```bash
docker stats --no-stream
```

### Backup database

```bash
docker-compose exec postgres pg_dump -U aiwebengine aiwebengine | gzip > backup-$(date +%Y%m%d-%H%M%S).sql.gz
```

### Restore database

```bash
gunzip < backup-20251024-120000.sql.gz | docker-compose exec -T postgres psql -U aiwebengine -d aiwebengine
```

---

## Environment-Specific Checklists

### Local Development ✓

- [ ] Copy `config.local.toml` to `config.toml`
- [ ] Copy `.env.example` to `.env`
- [ ] Edit `.env` with OAuth credentials
- [ ] Run `source .env && cargo run`
- [ ] Access `http://localhost:3000/health`

### Staging Deployment ✓

- [ ] Copy `config.staging.toml` to `config.toml`
- [ ] Set all environment variables
- [ ] Configure OAuth with staging URLs
- [ ] Deploy with `make docker-staging`
- [ ] Verify HTTPS certificates
- [ ] Test authentication flow

### Production Deployment ✓

- [ ] Copy `config.production.toml` to `config.toml`
- [ ] Generate strong secrets
- [ ] Set all secrets via environment (not files!)
- [ ] Configure OAuth with production URLs
- [ ] Deploy with `make docker-prod`
- [ ] Verify HTTPS certificates
- [ ] Configure monitoring
- [ ] Set up backups
- [ ] Test all functionality

---

For detailed explanations, see the complete guides:

- [Getting Started](01-GETTING-STARTED.md)
- [Configuration](02-CONFIGURATION.md)
- [Running Environments](03-RUNNING-ENVIRONMENTS.md)
- [Secrets and Security](04-SECRETS-AND-SECURITY.md)
- [Monitoring and Maintenance](05-MONITORING-AND-MAINTENANCE.md)
- [Troubleshooting](06-TROUBLESHOOTING.md)
