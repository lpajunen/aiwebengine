# 01 - Getting Started

This guide helps you deploy aiwebengine for the first time. Follow these steps to get your instance running quickly.

## Prerequisites

### System Requirements

**Minimum** (for testing/development):

- CPU: 1 core
- RAM: 512 MB
- Disk: 1 GB free space
- OS: Linux, macOS, or Windows with WSL2

**Recommended** (for production):

- CPU: 2+ cores
- RAM: 2+ GB
- Disk: 10+ GB for logs and data
- OS: Ubuntu 22.04 LTS or similar

### Software Requirements

Choose **one** deployment method:

#### Option A: Docker (Recommended)

- Docker 20.10 or later
- Docker Compose 2.0 or later

```bash
# Check versions
docker --version
docker-compose --version
```

#### Option B: Build from Source

- Rust (latest stable version)
- PostgreSQL 14 or later

```bash
# Install Rust
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Check version
rustc --version
```

### OAuth Provider Setup (Required for Authentication)

You need OAuth credentials from at least one provider:

**Google OAuth** (recommended for getting started):

1. Go to [Google Cloud Console](https://console.cloud.google.com/apis/credentials)
2. Create a new project or select existing
3. Go to "Credentials" → "Create Credentials" → "OAuth 2.0 Client ID"
4. Choose "Web application"
5. Add authorized redirect URIs:
   - Local: `http://localhost:3000/auth/callback/google`
   - Production: `https://yourdomain.com/auth/callback/google`
6. Save the **Client ID** and **Client Secret**

> **Note:** For other providers (Microsoft, Apple), see [04-SECRETS-AND-SECURITY.md](04-SECRETS-AND-SECURITY.md)

---

## Quick Start (5 Minutes)

### Local Development with Docker

This is the fastest way to get aiwebengine running on your local machine:

```bash
# 1. Clone the repository
git clone https://github.com/lpajunen/aiwebengine.git
cd aiwebengine

# 2. Set up environment
make docker-setup

# This runs:
#   - cp .env.example .env
#   - docker build

# 3. Edit .env with your OAuth credentials
nano .env  # or use your preferred editor

# Set these variables:
# APP_AUTH__PROVIDERS__GOOGLE__CLIENT_ID=your-client-id
# APP_AUTH__PROVIDERS__GOOGLE__CLIENT_SECRET=your-client-secret

# 4. Start the server
make docker-local

# 5. Verify it's running
curl http://localhost:3000/health
```

**Success!** Your aiwebengine instance is now running at `http://localhost:3000`

#### What Just Happened?

1. ✅ Created `.env` file with environment variables
2. ✅ Built Docker image with all dependencies
3. ✅ Started PostgreSQL database
4. ✅ Started aiwebengine server
5. ✅ Configured with `config.local.toml` (development settings)

#### Next Steps

- View logs: `make docker-logs-local`
- Access the server: `http://localhost:3000`
- Sign in: `http://localhost:3000/auth/login`
- Stop server: `make docker-stop`

---

## Detailed Setup

### Step 1: Clone Repository

```bash
git clone https://github.com/lpajunen/aiwebengine.git
cd aiwebengine
```

### Step 2: Choose Configuration

aiwebengine provides three pre-configured environments:

```bash
# Local development (relaxed security, verbose logging)
cp config.local.toml config.toml

# Staging (moderate security, integration testing)
cp config.staging.toml config.toml

# Production (strict security, optimized performance)
cp config.production.toml config.toml
```

**For first-time setup, use `config.local.toml`**

### Step 3: Configure Environment Variables

```bash
# Copy the template
cp .env.example .env

# Edit with your values
nano .env
```

**Required variables for local development:**

```bash
# JWT Secret (generate with: openssl rand -base64 48)
export APP_AUTH__JWT_SECRET="your-generated-secret-here"

# API Key (generate with: openssl rand -hex 32)
export APP_SECURITY__API_KEY="your-generated-api-key-here"

# Google OAuth
export APP_AUTH__PROVIDERS__GOOGLE__CLIENT_ID="your-client-id.apps.googleusercontent.com"
export APP_AUTH__PROVIDERS__GOOGLE__CLIENT_SECRET="your-client-secret"
export APP_AUTH__PROVIDERS__GOOGLE__REDIRECT_URI="http://localhost:3000/auth/callback/google"
```

**Generate secrets:**

```bash
# JWT Secret
openssl rand -base64 48

# API Key
openssl rand -hex 32
```

### Step 4: Set Bootstrap Admin

Add your email address to automatically get administrator privileges on first sign-in.

**Option A: In config.toml**

```toml
[auth]
bootstrap_admins = [
    "your-email@gmail.com"  # Use the email from your OAuth provider
]
```

**Option B: Via environment variable**

```bash
export APP_AUTH__BOOTSTRAP_ADMINS='["your-email@gmail.com"]'
```

### Step 5: Deploy

#### With Docker (Recommended)

```bash
# Start services
make docker-local

# View logs
make docker-logs-local

# Access at http://localhost:3000
```

#### Without Docker (Build from Source)

```bash
# Install dependencies
cargo build --release

# Start PostgreSQL (if not using Docker)
# Install and start PostgreSQL according to your OS

# Set database URL
export APP_REPOSITORY__DATABASE_URL="postgresql://username:password@localhost:5432/aiwebengine"

# Load environment and run
source .env && cargo run --release
```

### Step 6: Verify Installation

```bash
# Check health endpoint
curl http://localhost:3000/health

# Expected response:
# {"status":"ok","timestamp":"2025-10-24T..."}
```

```bash
# Check Docker containers (if using Docker)
docker-compose ps

# Should show:
# - aiwebengine-dev (running, healthy)
# - postgres (running)
```

### Step 7: Sign In

1. Open browser: `http://localhost:3000/auth/login`
2. Click "Sign in with Google"
3. Authenticate with your Google account
4. You should be redirected back and signed in
5. Your account will have **Administrator** role (because of bootstrap_admins)

### Step 8: Verify Admin Access

```bash
# Access the management UI
open http://localhost:3000/manager
```

You should see the management interface where you can:

- View all users
- Manage user roles
- View system status

---

## Common Setup Issues

### Issue: OAuth Redirect Mismatch

**Error:** "redirect_uri_mismatch" after OAuth sign-in

**Solution:**

1. Check your redirect URI in `.env`:

   ```bash
   echo $APP_AUTH__PROVIDERS__GOOGLE__REDIRECT_URI
   # Should be: http://localhost:3000/auth/callback/google
   ```

2. Verify it matches exactly in Google Cloud Console
3. Restart the server after changing

### Issue: Port Already in Use

**Error:** "Address already in use (os error 48)" or similar

**Solution:**

```bash
# Find process using port 3000
lsof -i :3000

# Kill the process (replace PID)
kill -9 <PID>

# Or change the port in config.toml
[server]
port = 3001
```

### Issue: Database Connection Failed

**Error:** "Connection refused" or "role does not exist"

**Solution:**

For Docker deployment:

```bash
# Check PostgreSQL container
docker-compose ps postgres

# View PostgreSQL logs
docker-compose logs postgres

# Restart PostgreSQL
docker-compose restart postgres
```

For local PostgreSQL:

```bash
# Check PostgreSQL status
pg_isready

# Create user and database
psql postgres -c "CREATE USER aiwebengine WITH PASSWORD 'your-password';"
psql postgres -c "CREATE DATABASE aiwebengine OWNER aiwebengine;"

# Update DATABASE_URL in .env
export APP_REPOSITORY__DATABASE_URL="postgresql://aiwebengine:your-password@localhost:5432/aiwebengine"
```

### Issue: Secret Too Short

**Error:** "JWT secret must be at least 32 characters"

**Solution:**

```bash
# Generate proper secret
openssl rand -base64 48

# Update in .env
export APP_AUTH__JWT_SECRET="<generated-value>"

# Restart server
```

### Issue: Not Getting Admin Role

**Problem:** Signed in but don't have admin access

**Solution:**

1. Check your email is in bootstrap_admins:

   ```bash
   grep bootstrap_admins config.toml
   ```

2. Make sure email matches your OAuth account exactly

3. Sign out and sign in again

4. Check logs for bootstrap messages:

   ```bash
   docker-compose logs aiwebengine | grep -i bootstrap
   ```

---

## Understanding the Setup

### What Was Installed?

```plaintext
aiwebengine/
├── config.toml              # Active configuration
├── .env                     # Environment variables (secrets)
├── data/                    # Database data (Docker volume)
├── logs/                    # Application logs
└── scripts/                 # JavaScript scripts (solution code)
```

### Services Running

When using Docker:

| Service | Container | Port | Purpose |
|---------|-----------|------|---------|
| aiwebengine | `aiwebengine-dev` | 3000 | Main application |
| PostgreSQL | `postgres` | 5432 | Database |

### Configuration Active

- **Config file:** `config.local.toml`
- **Environment overrides:** From `.env` file
- **Log level:** `debug` (verbose output)
- **Security:** Relaxed (for development)
- **Database:** PostgreSQL with auto-migrations
- **HTTPS:** Disabled (local development)

---

## Next Steps

### Start Building

Now that your instance is running:

1. **Explore the Management UI:** `http://localhost:3000/manager`
2. **Write JavaScript scripts:** See [Solution Developer Documentation](../solution-developers/)
3. **Try the editor:** `http://localhost:3000/editor` (if enabled)

### Configure for Your Needs

- **[Configuration Guide](02-CONFIGURATION.md)** - Customize settings
- **[Secrets and Security](04-SECRETS-AND-SECURITY.md)** - Add API keys for AI services
- **[Running Environments](03-RUNNING-ENVIRONMENTS.md)** - Deploy to staging/production

### Learn Operations

- **[Monitoring and Maintenance](05-MONITORING-AND-MAINTENANCE.md)** - Keep it healthy
- **[Troubleshooting](06-TROUBLESHOOTING.md)** - Solve problems

---

## Quick Commands Reference

```bash
# Start local development
make docker-local

# View logs
make docker-logs-local

# Stop everything
make docker-stop

# Clean up (removes data!)
make docker-clean

# Access container shell
make docker-shell-local

# Check status
docker-compose ps
curl http://localhost:3000/health
```

---

## Getting Help

- **Documentation:** [Complete guide index](../INDEX.md)
- **Issues:** [GitHub Issues](https://github.com/lpajunen/aiwebengine/issues)
- **Quick reference:** [QUICK-REFERENCE.md](QUICK-REFERENCE.md)

---

**Congratulations! 🎉** You've successfully set up aiwebengine. Ready to deploy to production? See [03-RUNNING-ENVIRONMENTS.md](03-RUNNING-ENVIRONMENTS.md).
