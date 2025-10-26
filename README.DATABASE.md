# Database Integration - Quick Start

AIWebEngine uses **SQLx** for PostgreSQL database integration with type-safe queries and automated migrations.

## Fastest Setup (One Command)

```bash
# Run the setup script
./scripts/setup-database.sh
```

This will:

1. Install SQLx CLI (if needed)
2. Start PostgreSQL in Docker
3. Run all migrations
4. Verify the setup

---

## Manual Setup

### 1. Install SQLx CLI

```bash
cargo install sqlx-cli --no-default-features --features postgres
```

### 2. Start PostgreSQL Container

```bash
# Start PostgreSQL in Docker (local development)
docker-compose -f docker-compose.local.yml up -d postgres-dev

# Or for production
docker-compose up -d postgres
```

### 3. Database Helper Script

The project includes a helper script for database operations:

```bash
# Make script executable (first time only)
chmod +x scripts/db.sh

# Create database (using Docker container)
./scripts/db.sh createdb

# Run psql interactive terminal
./scripts/db.sh psql

# Show all available commands
./scripts/db.sh --help
```

**Note:** The database is created automatically when using Docker Compose. The helper script is mainly for running migrations and queries.

### 4. Configure Environment

```bash
# Copy environment template
cp .env.example .env

# For local development with Docker:
export APP_REPOSITORY__DATABASE_URL="postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine"
export APP_REPOSITORY__AUTO_MIGRATE=true

# For production (set proper password):
# export APP_REPOSITORY__DATABASE_URL="postgresql://aiwebengine:your-secure-password@localhost:5432/aiwebengine"

# Load environment
source .env
```

### 5. Run Migrations

**Option A: Using Helper Script (Recommended)**

```bash
# Run migrations (uses Docker container)
./scripts/db.sh migrate-run

# Check migration status
./scripts/db.sh migrate-info

# Revert last migration if needed
./scripts/db.sh migrate-revert
```

**Option B: Automatic (Development)**

Migrations run automatically on server startup when `auto_migrate = true` in config:

```bash
cargo run
# Server starts and runs migrations automatically
```

**Option C: Manual with SQLx CLI**

```bash
# Set DATABASE_URL (for local Docker)
export DATABASE_URL="postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine"

# Run migrations
sqlx migrate run

# Then start server
cargo run
```

### 6. Verify Setup

```bash
# Using helper script
./scripts/db.sh psql -c "\dt"

# Or check migration status
./scripts/db.sh migrate-info

# Should show tables: scripts, assets, logs, users, sessions
```

---

## Current Schema

- **scripts** - JavaScript code storage
- **assets** - Static assets (CSS, JS, images)
- **logs** - Script execution logs
- **users** - User accounts and authentication
- **sessions** - User session data

See `migrations/` directory for detailed schema.

---

## Usage in Code

The database module is already integrated:

```rust
use aiwebengine::database;

// Initialize database connection
let db = database::init_database(&config.repository, true).await?;

// Get connection pool
let pool = db.pool();

// Query example
let script = sqlx::query!("SELECT * FROM scripts WHERE uri = $1", uri)
    .fetch_one(pool)
    .await?;
```

---

## Common Commands

### Using Helper Script (Recommended for Docker)

```bash
# Interactive psql
./scripts/db.sh psql

# Run SQL query
./scripts/db.sh psql -c "SELECT * FROM users"

# List tables
./scripts/db.sh psql -c "\dt"

# Run migrations
./scripts/db.sh migrate-run

# Check migration status
./scripts/db.sh migrate-info

# Revert last migration
./scripts/db.sh migrate-revert

# Backup database
./scripts/db.sh backup backup.sql

# Restore from backup
./scripts/db.sh restore backup.sql

# View logs
./scripts/db.sh logs

# Production mode (use --prod flag)
./scripts/db.sh --prod migrate-run
```

### Using SQLx CLI Directly

```bash
# Create new migration
sqlx migrate add <description>

# Set DATABASE_URL first
export DATABASE_URL="postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine"

# Run pending migrations
sqlx migrate run

# Revert last migration
sqlx migrate revert

# Check migration status
sqlx migrate info

# Verify queries at compile time
cargo sqlx prepare
```

---

## Configuration Options

In `config.toml`:

```toml
[repository]
storage_type = "postgresql"
database_url = "${APP_REPOSITORY__DATABASE_URL}"
auto_migrate = true  # Auto-run migrations on startup
max_connections = 5  # Connection pool size
```

---

## Production Notes

**⚠️ Important for Production:**

1. **Disable auto-migration** - Set `auto_migrate = false`
2. **Run migrations manually** - Use `sqlx migrate run` in deployment pipeline
3. **Backup first** - Always backup before running migrations
4. **Test in staging** - Validate migrations in staging environment

```bash
# Production deployment
pg_dump -Fc aiwebengine > backup.dump
sqlx migrate run
cargo run --release
```

---

## Docker Setup

PostgreSQL is already configured in the Docker Compose files:

**Local Development** (`docker-compose.local.yml`):

- Container: `aiwebengine-postgres-dev`
- Database: `aiwebengine`
- User: `aiwebengine`
- Password: `devpassword`
- Port: `5432` (exposed to host)

**Production** (`docker-compose.yml`):

- Container: `aiwebengine-postgres`
- Database: `aiwebengine`
- User: `aiwebengine`
- Password: Set via `POSTGRES_PASSWORD` env var
- Port: Not exposed (internal network only)

### Starting Services

```bash
# Local development
docker-compose -f docker-compose.local.yml up -d

# Production
docker-compose up -d

# Start only PostgreSQL
docker-compose -f docker-compose.local.yml up -d postgres-dev
```

### Accessing PostgreSQL

```bash
# Using helper script (recommended)
./scripts/db.sh psql

# Or directly
docker exec -it aiwebengine-postgres-dev psql -U aiwebengine -d aiwebengine
```

---

## Troubleshooting

### Container Issues

**PostgreSQL container not running:**

```bash
# Check container status
docker ps -a | grep postgres

# Start the container
docker-compose -f docker-compose.local.yml up -d postgres-dev

# View logs
./scripts/db.sh logs
# Or: docker-compose -f docker-compose.local.yml logs postgres-dev
```

**Connection refused:**

```bash
# Check if container is healthy
docker inspect aiwebengine-postgres-dev | grep Health

# Restart container
docker-compose -f docker-compose.local.yml restart postgres-dev

# Check port is exposed
docker ps | grep postgres
```

### Database Issues

**Database does not exist:**

```bash
# The database is created automatically by Docker
# If needed, recreate it:
./scripts/db.sh psql -c "DROP DATABASE IF EXISTS aiwebengine;"
./scripts/db.sh createdb
```

**Migration errors:**

```bash
# Check status
./scripts/db.sh migrate-info

# View applied migrations
./scripts/db.sh psql -c "SELECT * FROM _sqlx_migrations;"

# Revert if needed
./scripts/db.sh migrate-revert
```

**Permission denied:**

```bash
# Make sure script is executable
chmod +x scripts/db.sh

# Check Docker is running
docker ps
```

### Running Engine Locally with Containerized PostgreSQL

When running the engine locally (not in Docker) but using containerized PostgreSQL:

```bash
# 1. Start only PostgreSQL
docker-compose -f docker-compose.local.yml up -d postgres-dev

# 2. Set connection string (localhost instead of container name)
export APP_REPOSITORY__DATABASE_URL="postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine"

# 3. Run migrations
./scripts/db.sh migrate-run

# 4. Start engine locally
cargo run
```

---

## Next Steps

- **Documentation:** See `docs/engine-administrators/DATABASE-MIGRATIONS.md` for detailed migration guide
- **Examples:** Check `src/database.rs` for implementation details
- **SQLx Docs:** https://github.com/launchbadge/sqlx

---

## Migration from In-Memory Storage

Currently, the repository uses in-memory storage. The database integration is ready but not yet connected to the repository layer. Future work will:

1. Create database-backed repository implementation
2. Support dual mode (memory/database)
3. Migrate existing in-memory data to PostgreSQL

The foundation is ready - migrations are in place and can be run now!
