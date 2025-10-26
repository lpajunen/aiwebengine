# Database Helper Scripts

This directory contains helper scripts for managing the PostgreSQL database in Docker containers.

## Quick Start

```bash
# One-command setup (first time)
./scripts/setup-database.sh

# Daily usage
./scripts/db.sh psql              # Interactive SQL
./scripts/db.sh migrate-run       # Run migrations
```

---

## setup-database.sh

**Purpose:** First-time database setup automation

**What it does:**

1. Checks if Docker is running
2. Installs SQLx CLI if needed
3. Starts PostgreSQL container
4. Runs all migrations
5. Verifies the setup

**Usage:**

```bash
./scripts/setup-database.sh
```

**When to use:**

- First time setting up the project
- After cleaning Docker volumes
- Setting up a new development environment

---

## db.sh

**Purpose:** Daily database operations through Docker containers

### Common Commands

```bash
# Interactive PostgreSQL terminal
./scripts/db.sh psql

# Run SQL queries
./scripts/db.sh psql -c "SELECT * FROM users"
./scripts/db.sh psql -c "\dt"  # List tables
./scripts/db.sh psql -c "\d scripts"  # Describe table

# Migrations
./scripts/db.sh migrate-run      # Run pending migrations
./scripts/db.sh migrate-info     # Check migration status
./scripts/db.sh migrate-revert   # Revert last migration

# Backup and restore
./scripts/db.sh backup backup.sql      # Create backup
./scripts/db.sh restore backup.sql     # Restore from backup

# Database management
./scripts/db.sh createdb [name]  # Create database
./scripts/db.sh dropdb [name]    # Drop database

# Container operations
./scripts/db.sh shell            # Open shell in postgres container
./scripts/db.sh logs             # Show postgres logs
```

### Production Mode

Add `--prod` or `-p` flag to use production containers:

```bash
./scripts/db.sh --prod migrate-run
./scripts/db.sh --prod psql
./scripts/db.sh -p backup backup.sql
```

### Environment Detection

The script automatically determines which Docker Compose file to use:

- **Default (local development):**
  - File: `docker-compose.local.yml`
  - Container: `aiwebengine-postgres-dev`
  - User: `aiwebengine`
  - Password: `devpassword`
  - Database: `aiwebengine`

- **Production (`--prod`):**
  - File: `docker-compose.yml`
  - Container: `aiwebengine-postgres`
  - User: `aiwebengine`
  - Password: From `POSTGRES_PASSWORD` env var
  - Database: `aiwebengine`

### Full Command Reference

```bash
./scripts/db.sh --help
```

**Output:**

```
Usage: ./scripts/db.sh [--prod|-p] <command> [args...]

Commands:
  psql [args]         - Run psql (PostgreSQL interactive terminal)
  createdb [name]     - Create a database (default: aiwebengine)
  dropdb [name]       - Drop a database (default: aiwebengine)
  migrate-run         - Run SQLx migrations
  migrate-revert      - Revert last migration
  migrate-info        - Show migration status
  backup [file]       - Backup database to file
  restore <file>      - Restore database from file
  shell               - Open a shell in the postgres container
  logs                - Show postgres container logs

Options:
  --prod, -p          - Use production containers (default: local/dev)
```

---

## Examples

### First-Time Setup

```bash
# Complete setup
./scripts/setup-database.sh

# Or manual steps
docker-compose -f docker-compose.local.yml up -d postgres-dev
./scripts/db.sh migrate-run
./scripts/db.sh psql -c "\dt"
```

### Development Workflow

```bash
# Start your day
docker-compose -f docker-compose.local.yml up -d postgres-dev

# Create new migration
sqlx migrate add add_user_preferences

# Edit the migration file in migrations/

# Test the migration
./scripts/db.sh migrate-run

# Verify
./scripts/db.sh psql -c "\d users"

# If something went wrong
./scripts/db.sh migrate-revert

# Fix and retry
./scripts/db.sh migrate-run
```

### Data Operations

```bash
# Backup before major changes
./scripts/db.sh backup pre_migration_backup.sql

# Interactive queries
./scripts/db.sh psql
# aiwebengine=> SELECT * FROM users WHERE is_admin = true;
# aiwebengine=> \q

# Quick queries
./scripts/db.sh psql -c "SELECT COUNT(*) FROM scripts"
./scripts/db.sh psql -c "SELECT * FROM sessions WHERE expires_at > NOW()"

# Restore if needed
./scripts/db.sh restore pre_migration_backup.sql
```

### Troubleshooting

```bash
# Check if container is running
docker ps | grep postgres

# View logs
./scripts/db.sh logs

# Get shell access
./scripts/db.sh shell
# Then inside container:
# $ pg_isready -U aiwebengine
# $ psql -U aiwebengine -d aiwebengine

# Restart container
docker-compose -f docker-compose.local.yml restart postgres-dev

# Reset database (⚠️ destroys all data)
./scripts/db.sh dropdb
./scripts/db.sh createdb
./scripts/db.sh migrate-run
```

### Production Deployment

```bash
# Backup production data
./scripts/db.sh --prod backup prod_backup_$(date +%Y%m%d_%H%M%S).sql

# Run migrations (after testing in staging)
./scripts/db.sh --prod migrate-info   # Check current state
./scripts/db.sh --prod migrate-run    # Apply migrations

# Verify
./scripts/db.sh --prod psql -c "\dt"
```

---

## Container Configuration

### Local Development

Defined in `docker-compose.local.yml`:

```yaml
postgres-dev:
  image: postgres:16-alpine
  container_name: aiwebengine-postgres-dev
  environment:
    POSTGRES_DB: aiwebengine
    POSTGRES_USER: aiwebengine
    POSTGRES_PASSWORD: devpassword
  ports:
    - "5432:5432"
```

### Production

Defined in `docker-compose.yml`:

```yaml
postgres:
  image: postgres:16-alpine
  container_name: aiwebengine-postgres
  environment:
    POSTGRES_DB: aiwebengine
    POSTGRES_USER: aiwebengine
    POSTGRES_PASSWORD: ${POSTGRES_PASSWORD}
```

---

## Tips

1. **Always backup before migrations** in production:

   ```bash
   ./scripts/db.sh --prod backup before_migration.sql
   ./scripts/db.sh --prod migrate-run
   ```

2. **Use psql for quick checks:**

   ```bash
   ./scripts/db.sh psql -c "\dt"           # List tables
   ./scripts/db.sh psql -c "\du"           # List users
   ./scripts/db.sh psql -c "\l"            # List databases
   ./scripts/db.sh psql -c "\d+ scripts"   # Detailed table info
   ```

3. **Monitor logs during development:**

   ```bash
   ./scripts/db.sh logs
   # Or follow logs:
   docker-compose -f docker-compose.local.yml logs -f postgres-dev
   ```

4. **Connect from engine running locally:**
   ```bash
   # PostgreSQL is exposed on localhost:5432
   export APP_REPOSITORY__DATABASE_URL="postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine"
   cargo run
   ```

---

## Troubleshooting

**Script reports container not running:**

```bash
docker-compose -f docker-compose.local.yml up -d postgres-dev
```

**Permission denied:**

```bash
chmod +x scripts/db.sh
chmod +x scripts/setup-database.sh
```

**SQLx CLI not found:**

```bash
cargo install sqlx-cli --no-default-features --features postgres
```

**Can't connect to database:**

```bash
# Check container health
docker inspect aiwebengine-postgres-dev | grep Health

# Check logs
./scripts/db.sh logs

# Restart container
docker-compose -f docker-compose.local.yml restart postgres-dev
```
