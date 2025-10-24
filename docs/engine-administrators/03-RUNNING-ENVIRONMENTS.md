# 03 - Running Environments

Complete guide for deploying aiwebengine in local, staging, and production environments.

## Quick Navigation

- [Environment Overview](#environment-overview)
- [Local Development](#local-development)
- [Staging Environment](#staging-environment)
- [Production Deployment](#production-deployment)
- [Docker Deployment](#docker-deployment)
- [Bare Metal Deployment](#bare-metal-deployment)

---

## Environment Overview

### Environment Comparison

| | Local | Staging | Production |
|---|---|---|---|
| **Purpose** | Development & debugging | Integration testing | Live deployment |
| **Config File** | `config.local.toml` | `config.staging.toml` | `config.production.toml` |
| **Security** | Relaxed | Moderate | Strict |
| **Logging** | `debug` | `info` | `warn` or `error` |
| **HTTPS** | Optional | Recommended | Required |
| **Auto-migrate** | Enabled | Enabled | Disabled |
| **Console API** | Enabled | Enabled | Disabled |
| **Secrets** | `.env` file | Environment vars | Secret manager |

### Choosing Your Environment

- **Local:** Daily development, testing features, debugging
- **Staging:** QA testing, integration tests, pre-production validation
- **Production:** Live system serving real users

---

## Local Development

### Quick Start with Docker

```bash
# 1. Clone and enter directory
git clone https://github.com/lpajunen/aiwebengine.git
cd aiwebengine

# 2. Setup environment
make docker-setup

# 3. Configure credentials
nano .env
# Add your OAuth credentials

# 4. Start services
make docker-local

# 5. Access
open http://localhost:3000/health
```

### Manual Setup (Without Docker)

#### Prerequisites

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install PostgreSQL (macOS)
brew install postgresql@14
brew services start postgresql@14

# Install PostgreSQL (Ubuntu)
sudo apt update
sudo apt install postgresql postgresql-contrib
sudo systemctl start postgresql
```

#### Setup Steps

```bash
# 1. Copy configuration
cp config.local.toml config.toml

# 2. Set up environment variables
cp .env.example .env
nano .env

# 3. Create database
createdb aiwebengine

# 4. Set database URL
export APP_REPOSITORY__DATABASE_URL="postgresql://$(whoami)@localhost/aiwebengine"

# 5. Build and run
cargo build --release
source .env && cargo run --release
```

### Local Configuration

**config.toml** (from `config.local.toml`):

```toml
[server]
host = "127.0.0.1"
port = 3000

[logging]
level = "debug"
structured = true
targets = ["console", "file"]

[javascript]
enable_console = true
allowed_apis = ["fetch", "database", "logging", "filesystem"]

[repository]
database_type = "postgresql"
database_url = "${APP_REPOSITORY__DATABASE_URL}"
auto_migrate = true

[security]
cors_origins = ["*"]
require_https = false

[auth]
jwt_secret = "${APP_AUTH__JWT_SECRET}"
bootstrap_admins = ["your-email@gmail.com"]

[auth.cookie]
secure = false

[auth.providers.google]
client_id = "${APP_AUTH__PROVIDERS__GOOGLE__CLIENT_ID}"
client_secret = "${APP_AUTH__PROVIDERS__GOOGLE__CLIENT_SECRET}"
redirect_uri = "http://localhost:3000/auth/callback/google"
```

### Local Environment Variables (.env)

```bash
# Generate secrets
export APP_AUTH__JWT_SECRET="$(openssl rand -base64 48)"
export APP_SECURITY__API_KEY="$(openssl rand -hex 32)"

# Database (Docker or local)
export APP_REPOSITORY__DATABASE_URL="postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine"

# Google OAuth
export APP_AUTH__PROVIDERS__GOOGLE__CLIENT_ID="your-id.apps.googleusercontent.com"
export APP_AUTH__PROVIDERS__GOOGLE__CLIENT_SECRET="your-secret"
export APP_AUTH__PROVIDERS__GOOGLE__REDIRECT_URI="http://localhost:3000/auth/callback/google"

# Optional: AI services for testing
export SECRET_ANTHROPIC_API_KEY="sk-ant-api03-..."

# Debug logging
export RUST_LOG="aiwebengine=debug"
```

### Local Development Workflow

```bash
# Start server with auto-reload
make dev

# Run tests
make test

# Check code quality
make check

# View logs
tail -f logs/aiwebengine-dev.log

# Access services
open http://localhost:3000              # Main application
open http://localhost:3000/auth/login   # Sign in
open http://localhost:3000/manager      # Admin UI
open http://localhost:3000/editor       # Script editor
```

### Local Docker Commands

```bash
# Start
make docker-local

# View logs
make docker-logs-local

# Stop
make docker-stop

# Access container shell
make docker-shell-local

# Rebuild
docker-compose -f docker-compose.local.yml build --no-cache
docker-compose -f docker-compose.local.yml up -d

# Database access
docker-compose -f docker-compose.local.yml exec postgres \
  psql -U aiwebengine -d aiwebengine
```

---

## Staging Environment

Staging mirrors production but with testing-friendly settings.

### Setup

```bash
# 1. Copy staging configuration
cp config.staging.toml config.toml

# 2. Set environment variables (not .env file!)
export APP_AUTH__JWT_SECRET="$(openssl rand -base64 48)"
export APP_SECURITY__API_KEY="$(openssl rand -hex 32)"

# 3. Database
export APP_REPOSITORY__DATABASE_URL="postgresql://user:pass@staging-db.example.com:5432/aiwebengine"

# 4. OAuth with staging URLs
export APP_AUTH__PROVIDERS__GOOGLE__CLIENT_ID="staging-client-id.apps.googleusercontent.com"
export APP_AUTH__PROVIDERS__GOOGLE__CLIENT_SECRET="staging-client-secret"
export APP_AUTH__PROVIDERS__GOOGLE__REDIRECT_URI="https://staging.yourdomain.com/auth/callback/google"

# 5. Bootstrap admins
export APP_AUTH__BOOTSTRAP_ADMINS='["admin@yourdomain.com"]'

# 6. Deploy with Docker
make docker-staging
```

### Staging Configuration Highlights

```toml
[server]
host = "0.0.0.0"
port = 8080
request_timeout_ms = 15000

[logging]
level = "info"  # More verbose than production

[javascript]
enable_console = true  # Still enabled for debugging
allowed_apis = ["fetch", "database", "logging", "filesystem"]

[repository]
auto_migrate = true  # Test migrations

[security]
cors_origins = ["https://staging.yourdomain.com"]
require_https = false  # Can test without HTTPS
rate_limit_per_minute = 500

[auth.cookie]
secure = true  # If using HTTPS
```

### Staging Checklist

- [ ] Use `config.staging.toml`
- [ ] Generate strong secrets (don't reuse local ones)
- [ ] Set up staging database
- [ ] Configure OAuth with staging URLs
- [ ] Test HTTPS setup
- [ ] Verify auto-migrations work
- [ ] Test authentication flow
- [ ] Run integration tests
- [ ] Check logs and monitoring
- [ ] Test backup/restore procedures

---

## Production Deployment

Production requires strict security and optimized performance.

### Prerequisites

- **Server:** Ubuntu 22.04 LTS or similar (2+ CPU cores, 2+ GB RAM)
- **Database:** PostgreSQL 14+ (managed or self-hosted)
- **Domain:** Registered domain with DNS configured
- **SSL:** Let's Encrypt via Caddy (automatic) or manual certificates
- **Secrets:** AWS Secrets Manager, HashiCorp Vault, or similar

### Production Setup

#### 1. Server Preparation

```bash
# Update system
sudo apt update && sudo apt upgrade -y

# Install Docker
curl -fsSL https://get.docker.com | sh
sudo usermod -aG docker $USER

# Install Docker Compose
sudo apt install docker-compose-plugin

# Configure firewall
sudo ufw allow 22/tcp   # SSH
sudo ufw allow 80/tcp   # HTTP
sudo ufw allow 443/tcp  # HTTPS
sudo ufw allow 443/udp  # HTTP/3
sudo ufw enable
```

#### 2. DNS Configuration

```bash
# Point your domain to server IP
# A record: yourdomain.com -> YOUR_SERVER_IP
# A record: www.yourdomain.com -> YOUR_SERVER_IP

# Verify DNS
dig yourdomain.com +short
# Should return your server IP
```

#### 3. Clone and Configure

```bash
# Clone repository
git clone https://github.com/lpajunen/aiwebengine.git
cd aiwebengine

# Use production config
cp config.production.toml config.toml

# Review Caddyfile for your domain
nano Caddyfile.production
# Update with your actual domain
```

#### 4. Set Production Secrets

**Using AWS Secrets Manager:**

```bash
# JWT Secret
export APP_AUTH__JWT_SECRET="$(aws secretsmanager get-secret-value \
  --secret-id aiwebengine/jwt-secret \
  --query SecretString --output text)"

# API Key
export APP_SECURITY__API_KEY="$(aws secretsmanager get-secret-value \
  --secret-id aiwebengine/api-key \
  --query SecretString --output text)"

# Database URL
export APP_REPOSITORY__DATABASE_URL="$(aws secretsmanager get-secret-value \
  --secret-id aiwebengine/database-url \
  --query SecretString --output text)"

# Google OAuth
export APP_AUTH__PROVIDERS__GOOGLE__CLIENT_ID="$(aws secretsmanager get-secret-value \
  --secret-id aiwebengine/google-client-id \
  --query SecretString --output text)"
export APP_AUTH__PROVIDERS__GOOGLE__CLIENT_SECRET="$(aws secretsmanager get-secret-value \
  --secret-id aiwebengine/google-client-secret \
  --query SecretString --output text)"
export APP_AUTH__PROVIDERS__GOOGLE__REDIRECT_URI="https://yourdomain.com/auth/callback/google"

# CORS Origins
export APP_SECURITY__CORS_ORIGINS='["https://yourdomain.com","https://www.yourdomain.com"]'

# Bootstrap admin (only for initial setup)
export APP_AUTH__BOOTSTRAP_ADMINS='["admin@yourdomain.com"]'
```

**Using Environment Files (less secure):**

```bash
# Store in secure location (NOT in project directory)
sudo mkdir -p /etc/aiwebengine
sudo nano /etc/aiwebengine/production.env

# Add variables (without 'export')
APP_AUTH__JWT_SECRET=...
APP_SECURITY__API_KEY=...
# etc.

# Secure the file
sudo chmod 600 /etc/aiwebengine/production.env
sudo chown root:root /etc/aiwebengine/production.env
```

#### 5. Deploy

```bash
# Build and start
make docker-prod

# Or manually
docker-compose up -d

# Check status
docker-compose ps

# View logs
docker-compose logs -f aiwebengine
```

#### 6. Verify Deployment

```bash
# Check health
curl https://yourdomain.com/health

# Check SSL
curl -vI https://yourdomain.com 2>&1 | grep "SSL certificate verify"

# Check redirects
curl -I http://yourdomain.com
# Should redirect to https://yourdomain.com

curl -I https://www.yourdomain.com
# Should redirect to https://yourdomain.com
```

### Production Configuration Highlights

```toml
[server]
host = "0.0.0.0"
port = 8080
request_timeout_ms = 30000
max_body_size_mb = 10

[logging]
level = "error"  # Minimal logging
targets = ["file"]
file_path = "/var/log/aiwebengine/aiwebengine.log"
rotation = "hourly"
retention_days = 30

[javascript]
max_memory_mb = 128
enable_console = false  # Disabled for security
allowed_apis = ["fetch", "database", "logging"]  # No filesystem!

[repository]
max_connections = 50
auto_migrate = false  # Manual migrations

[security]
cors_origins = ["${APP_SECURITY__CORS_ORIGIN_1}"]
require_https = true
rate_limit_per_minute = 1000

[auth.cookie]
secure = true
same_site = "strict"
```

### Production Checklist

**Pre-Deployment:**

- [ ] Server provisioned and updated
- [ ] DNS configured and propagated
- [ ] Firewall configured
- [ ] SSL certificates ready (Caddy auto-handles)
- [ ] Database created and secured
- [ ] Secrets stored in secret manager
- [ ] OAuth configured with production URLs
- [ ] Monitoring configured
- [ ] Backup strategy defined

**Deployment:**

- [ ] Use `config.production.toml`
- [ ] All secrets from environment (never files!)
- [ ] Strong, unique secrets generated
- [ ] Database migrations run manually
- [ ] HTTPS enforced (`require_https = true`)
- [ ] Console API disabled
- [ ] Filesystem API removed
- [ ] Specific CORS origins
- [ ] Rate limiting enabled
- [ ] Secure cookies enabled

**Post-Deployment:**

- [ ] Health check passes
- [ ] HTTPS certificate valid
- [ ] Authentication works
- [ ] Test all critical paths
- [ ] Logs are written
- [ ] Monitoring alerts configured
- [ ] Backup tested
- [ ] Documentation updated
- [ ] Team notified

---

## Docker Deployment

### Docker Files Overview

- **Dockerfile** - Production multi-stage build
- **Dockerfile.local** - Local development with hot-reload
- **Dockerfile.staging** - Staging build
- **docker-compose.yml** - Production services
- **docker-compose.local.yml** - Local services
- **docker-compose.staging.yml** - Staging services

### Production Docker Compose

**docker-compose.yml** includes:

- aiwebengine application
- PostgreSQL database
- Caddy reverse proxy (HTTPS)
- Volume mounts for persistence

```yaml
services:
  aiwebengine:
    build: .
    ports:
      - "3000:8080"
    environment:
      - APP_AUTH__JWT_SECRET=${APP_AUTH__JWT_SECRET}
      # ... other variables
    volumes:
      - ./logs:/var/log/aiwebengine
      - ./data:/app/data
      - ./scripts:/app/scripts:ro
    depends_on:
      - postgres

  postgres:
    image: postgres:14-alpine
    environment:
      - POSTGRES_DB=aiwebengine
      - POSTGRES_USER=aiwebengine
      - POSTGRES_PASSWORD=${POSTGRES_PASSWORD}
    volumes:
      - postgres-data:/var/lib/postgresql/data

  caddy:
    image: caddy:2-alpine
    ports:
      - "80:80"
      - "443:443"
      - "443:443/udp"
    volumes:
      - ./Caddyfile.production:/etc/caddy/Caddyfile:ro
      - caddy-data:/data
      - caddy-config:/config
```

### Docker Management Commands

```bash
# Start
make docker-prod
# Or: docker-compose up -d

# Stop
make docker-stop
# Or: docker-compose down

# Restart
docker-compose restart aiwebengine

# View logs
make docker-logs
# Or: docker-compose logs -f aiwebengine

# Check status
docker-compose ps

# Access shell
make docker-shell
# Or: docker-compose exec aiwebengine /bin/bash

# Rebuild
docker-compose build --no-cache
docker-compose up -d

# Clean up
make docker-clean
# Or: docker-compose down -v
```

### Docker Volumes

```bash
# List volumes
docker volume ls

# Backup database
docker run --rm \
  -v aiwebengine_postgres-data:/data \
  -v $(pwd):/backup \
  alpine tar czf /backup/postgres-backup.tar.gz /data

# Restore database
docker run --rm \
  -v aiwebengine_postgres-data:/data \
  -v $(pwd):/backup \
  alpine tar xzf /backup/postgres-backup.tar.gz -C /

# Backup Caddy certificates
docker run --rm \
  -v aiwebengine_caddy-data:/data \
  -v $(pwd):/backup \
  alpine tar czf /backup/caddy-data-backup.tar.gz /data
```

---

## Bare Metal Deployment

Running directly on the host without Docker.

### Prerequisites

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install PostgreSQL
sudo apt install postgresql postgresql-contrib

# Install Caddy (optional, for HTTPS)
sudo apt install -y debian-keyring debian-archive-keyring apt-transport-https
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | sudo gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' | sudo tee /etc/apt/sources.list.d/caddy-stable.list
sudo apt update
sudo apt install caddy
```

### Build and Install

```bash
# Clone repository
git clone https://github.com/lpajunen/aiwebengine.git
cd aiwebengine

# Build release binary
cargo build --release

# Install binary
sudo cp target/release/aiwebengine /usr/local/bin/
sudo chmod +x /usr/local/bin/aiwebengine

# Create directories
sudo mkdir -p /etc/aiwebengine
sudo mkdir -p /var/log/aiwebengine
sudo mkdir -p /var/lib/aiwebengine/scripts
sudo mkdir -p /var/lib/aiwebengine/data

# Copy configuration
sudo cp config.production.toml /etc/aiwebengine/config.toml

# Create service user
sudo useradd -r -s /bin/false aiwebengine
sudo chown -R aiwebengine:aiwebengine /var/log/aiwebengine
sudo chown -R aiwebengine:aiwebengine /var/lib/aiwebengine
```

### Systemd Service

Create `/etc/systemd/system/aiwebengine.service`:

```ini
[Unit]
Description=AIWebEngine
After=network.target postgresql.service
Wants=postgresql.service

[Service]
Type=simple
User=aiwebengine
Group=aiwebengine
WorkingDirectory=/var/lib/aiwebengine
Environment="CONFIG_FILE=/etc/aiwebengine/config.toml"
EnvironmentFile=/etc/aiwebengine/production.env
ExecStart=/usr/local/bin/aiwebengine
Restart=always
RestartSec=10

[Install]
WantedBy=multi-user.target
```

Create `/etc/aiwebengine/production.env` with your environment variables.

Enable and start:

```bash
sudo systemctl daemon-reload
sudo systemctl enable aiwebengine
sudo systemctl start aiwebengine
sudo systemctl status aiwebengine
```

### Manage Service

```bash
# Start
sudo systemctl start aiwebengine

# Stop
sudo systemctl stop aiwebengine

# Restart
sudo systemctl restart aiwebengine

# Status
sudo systemctl status aiwebengine

# View logs
sudo journalctl -u aiwebengine -f

# Reload after config changes
sudo systemctl reload aiwebengine
```

---

## Related Documentation

- **[Getting Started](01-GETTING-STARTED.md)** - First-time setup
- **[Configuration](02-CONFIGURATION.md)** - Config reference
- **[Secrets and Security](04-SECRETS-AND-SECURITY.md)** - OAuth and secrets
- **[Monitoring and Maintenance](05-MONITORING-AND-MAINTENANCE.md)** - Operations
- **[Quick Reference](QUICK-REFERENCE.md)** - Command cheat sheet
