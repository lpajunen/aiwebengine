# Database Integration - Quick Start

AIWebEngine uses **SQLx** for PostgreSQL database integration with type-safe queries and automated migrations.

## Quick Setup

### 1. Install SQLx CLI

```bash
cargo install sqlx-cli --no-default-features --features postgres
```

### 2. Create Database

```bash
# Local PostgreSQL
createdb aiwebengine

# Or with custom user
createuser -P aiwebengine  # Enter password when prompted
createdb -O aiwebengine aiwebengine
```

### 3. Configure Environment

```bash
# Copy environment template
cp .env.example .env

# Edit .env and set:
export APP_REPOSITORY__DATABASE_URL="postgresql://aiwebengine:your-password@localhost:5432/aiwebengine"
export APP_REPOSITORY__AUTO_MIGRATE=true

# Load environment
source .env
```

### 4. Run Migrations

**Option A: Automatic (Development)**

Migrations run automatically on server startup when `auto_migrate = true`:

```bash
cargo run
# Server starts and runs migrations automatically
```

**Option B: Manual**

```bash
# Run migrations manually
sqlx migrate run

# Then start server
cargo run
```

### 5. Verify Setup

```bash
# Check migrations applied
sqlx migrate info

# Check tables created
psql aiwebengine -c "\dt"

# Should show: scripts, assets, logs, route_registrations
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

```bash
# Create new migration
sqlx migrate add create_my_table

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

Update `docker-compose.yml` to include PostgreSQL:

```yaml
services:
  postgres:
    image: postgres:15-alpine
    environment:
      POSTGRES_DB: aiwebengine
      POSTGRES_USER: aiwebengine
      POSTGRES_PASSWORD: ${POSTGRES_PASSWORD}
    volumes:
      - postgres_data:/var/lib/postgresql/data
    ports:
      - "5432:5432"

  aiwebengine:
    depends_on:
      - postgres
    environment:
      APP_REPOSITORY__DATABASE_URL: postgresql://aiwebengine:${POSTGRES_PASSWORD}@postgres:5432/aiwebengine

volumes:
  postgres_data:
```

---

## Troubleshooting

**Connection refused:**
```bash
# Check PostgreSQL is running
pg_isready
brew services list | grep postgresql  # macOS

# Start if needed
brew services start postgresql@15  # macOS
```

**Role does not exist:**
```bash
createuser -P aiwebengine
```

**Database does not exist:**
```bash
createdb aiwebengine
```

**Migration errors:**
```bash
# Check status
sqlx migrate info

# View applied migrations
psql aiwebengine -c "SELECT * FROM _sqlx_migrations;"
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
