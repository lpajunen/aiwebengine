# Database Migrations Guide

This guide covers database schema migrations for AIWebEngine using SQLx.

## Overview

AIWebEngine uses **SQLx** for database migrations. Migrations are SQL files stored in the `migrations/` directory that are versioned and applied sequentially.

## Migration Files

Migrations are located in `/migrations/` with numbered filenames:

```
migrations/
├── 20241024000001_create_scripts.sql
├── 20241024000002_create_assets.sql
├── 20241024000003_create_logs.sql
├── 20241024000004_create_users.sql
└── 20241024000005_create_sessions.sql
```

### Current Schema

**scripts** - Stores JavaScript scripts
- `id` (UUID, primary key)
- `uri` (TEXT, unique) - Script identifier
- `code` (TEXT) - JavaScript code
- `created_at`, `updated_at` (TIMESTAMPTZ)
- `initialized` (BOOLEAN) - Init function status
- `init_error` (TEXT, nullable) - Init error message
- `last_init_time` (TIMESTAMPTZ, nullable)

**assets** - Stores static assets
- `id` (UUID, primary key)
- `public_path` (TEXT, unique) - URL path
- `mimetype` (TEXT) - Content type
- `content` (BYTEA) - Binary data
- `created_at`, `updated_at` (TIMESTAMPTZ)

**logs** - Stores script execution logs
- `id` (UUID, primary key)
- `script_uri` (TEXT) - Related script
- `message` (TEXT) - Log entry
- `created_at` (TIMESTAMPTZ)

**users** - Stores user accounts
- `id` (UUID, primary key)
- `user_id` (TEXT, unique) - User identifier
- `email` (TEXT, unique) - User email address
- `name` (TEXT, nullable) - Display name
- `provider` (TEXT) - OAuth provider (google, microsoft, apple)
- `is_admin` (BOOLEAN) - Admin role flag
- `is_editor` (BOOLEAN) - Editor role flag
- `created_at`, `updated_at`, `last_login_at` (TIMESTAMPTZ)

**sessions** - Stores user sessions
- `id` (UUID, primary key)
- `session_id` (TEXT, unique) - Session identifier
- `user_id` (TEXT) - Related user
- `data` (JSONB) - Session data
- `created_at`, `expires_at`, `last_accessed_at` (TIMESTAMPTZ)

---

## Installation

### Install SQLx CLI

```bash
# Install with PostgreSQL support only (faster)
cargo install sqlx-cli --no-default-features --features postgres

# Verify installation
sqlx --version
```

### Start PostgreSQL Container

```bash
# For local development
docker-compose -f docker-compose.local.yml up -d postgres-dev

# For production
docker-compose up -d postgres
```

---

## Configuration

Set your database URL:

```bash
# For local development with Docker
export DATABASE_URL="postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine"

# For production (set proper password)
export DATABASE_URL="postgresql://aiwebengine:your-password@localhost:5432/aiwebengine"

# Or in .env file
echo 'DATABASE_URL="postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine"' >> .env
```

### Using the Database Helper Script

The project includes `scripts/db.sh` for easy database operations:

```bash
# Make executable (first time)
chmod +x scripts/db.sh

# Show all commands
./scripts/db.sh --help

# Common operations
./scripts/db.sh psql              # Interactive PostgreSQL
./scripts/db.sh migrate-run       # Run migrations
./scripts/db.sh migrate-info      # Check migration status
./scripts/db.sh backup backup.sql # Backup database
```

---

## Running Migrations

### Using Helper Script (Recommended for Docker)

```bash
# Run all pending migrations
./scripts/db.sh migrate-run

# Check migration status
./scripts/db.sh migrate-info

# Revert the last migration
./scripts/db.sh migrate-revert

# For production containers
./scripts/db.sh --prod migrate-run
```

### Automatic (Development)

Set `auto_migrate = true` in your `config.toml`:

```toml
[repository]
database_type = "postgresql"
database_url = "${APP_REPOSITORY__DATABASE_URL}"
auto_migrate = true  # Runs migrations on startup
```

The server will automatically run pending migrations on startup.

### Manual with SQLx CLI (Alternative)

```bash
# Set DATABASE_URL first
export DATABASE_URL="postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine"

# Check migration status
sqlx migrate info

# Run all pending migrations
sqlx migrate run

# Revert the last migration
sqlx migrate revert
```

**Production:** Set `auto_migrate = false` in production config for manual control.

---

## Creating New Migrations

### Using SQLx CLI

```bash
# Create a new migration
sqlx migrate add <name>

# Example: Add users table
sqlx migrate add create_users_table

# This creates: migrations/YYYYMMDDHHMMSS_create_users_table.sql
```

### Manual Creation

Create a file in `migrations/` with format: `YYYYMMDDHHMMSS_description.sql`

**Example:** `migrations/20241024000005_create_users.sql`

```sql
-- Create users table
CREATE TABLE IF NOT EXISTS users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    email TEXT NOT NULL UNIQUE,
    name TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Create index on email
CREATE INDEX idx_users_email ON users(email);
```

**Important:**
- Use `IF NOT EXISTS` for idempotency
- Include both schema changes and indexes
- Use `TIMESTAMPTZ` for timestamps
- Add comments for clarity

---

## Testing Migrations

### Test in Development with Docker

```bash
# Using helper script
./scripts/db.sh psql -c "CREATE DATABASE aiwebengine_test;"

# Run migrations against test database
export DATABASE_URL="postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine_test"
sqlx migrate run

# Verify schema
./scripts/db.sh psql -d aiwebengine_test -c "\dt"  # List tables
./scripts/db.sh psql -d aiwebengine_test -c "\d scripts"  # Describe table

# Cleanup
./scripts/db.sh psql -c "DROP DATABASE aiwebengine_test;"
```

### Revert and Retry

```bash
# Revert last migration
sqlx migrate revert

# Make changes to migration file

# Re-run
sqlx migrate run
```

---

## Migration Best Practices

### 1. **Make Migrations Idempotent**

Always use `IF NOT EXISTS`:

```sql
CREATE TABLE IF NOT EXISTS scripts (...);
CREATE INDEX IF NOT EXISTS idx_scripts_uri ON scripts(uri);
```

### 2. **Use Transactions Carefully**

SQLx wraps each migration in a transaction automatically. For complex migrations:

```sql
-- Your migration runs in a transaction by default
ALTER TABLE scripts ADD COLUMN IF NOT EXISTS new_field TEXT;
UPDATE scripts SET new_field = 'default_value' WHERE new_field IS NULL;
```

### 3. **Version Your Changes**

Never modify existing migration files after they've been deployed. Create new migrations instead:

```bash
# ❌ DON'T modify: 20241024000001_create_scripts.sql
# ✅ DO create:    20241024000005_add_scripts_metadata.sql
```

### 4. **Test Rollbacks**

Ensure you can revert migrations:

```bash
sqlx migrate run
sqlx migrate revert  # Should cleanly undo the last migration
```

### 5. **Handle Data Carefully**

For migrations that modify data:

```sql
-- Add column with default
ALTER TABLE scripts ADD COLUMN IF NOT EXISTS priority INTEGER DEFAULT 0;

-- Update existing rows
UPDATE scripts SET priority = 1 WHERE uri LIKE '%core%';

-- Make required after population
ALTER TABLE scripts ALTER COLUMN priority SET NOT NULL;
```

---

## Troubleshooting

### Migration Failed

```bash
# Check which migrations have been applied
sqlx migrate info

# View PostgreSQL logs
docker-compose logs postgres

# Or check directly
psql aiwebengine -c "SELECT * FROM _sqlx_migrations;"
```

### Locked State

If migrations fail mid-way:

```sql
-- Check migration lock
SELECT * FROM _sqlx_migrations WHERE success = false;

-- Clean up if needed (BE CAREFUL)
DELETE FROM _sqlx_migrations WHERE version = <failed_version>;
```

### Reset Database (Development Only)

```bash
# ⚠️  WARNING: This deletes all data!
dropdb aiwebengine
createdb aiwebengine
sqlx migrate run
```

---

## CI/CD Integration

### Pre-deployment Check

```bash
# In your CI pipeline
sqlx migrate info --database-url $DATABASE_URL

# Run migrations (production)
sqlx migrate run --database-url $DATABASE_URL
```

### GitHub Actions Example

```yaml
- name: Run Database Migrations
  env:
    DATABASE_URL: ${{ secrets.DATABASE_URL }}
  run: |
    cargo install sqlx-cli --no-default-features --features postgres
    sqlx migrate run
```

---

## Compile-Time Verification

SQLx can verify queries at compile time:

```rust
// This query is checked against your database at compile time
let script = sqlx::query_as::<_, Script>(
    "SELECT id, uri, code FROM scripts WHERE uri = $1"
)
.bind(uri)
.fetch_one(&pool)
.await?;
```

To enable:

```bash
# Create .sqlx folder for offline mode
cargo sqlx prepare

# Commit .sqlx to git for CI builds
git add .sqlx
```

---

## Production Workflow

1. **Develop migration locally**
   ```bash
   sqlx migrate add my_feature
   # Edit migrations/YYYYMMDDHHMMSS_my_feature.sql
   sqlx migrate run
   ```

2. **Test thoroughly**
   ```bash
   # Test forward
   sqlx migrate run
   
   # Test rollback
   sqlx migrate revert
   
   # Re-apply
   sqlx migrate run
   ```

3. **Commit to repository**
   ```bash
   git add migrations/
   git commit -m "Add my_feature migration"
   ```

4. **Deploy to staging**
   ```bash
   # Staging environment
   export DATABASE_URL="postgresql://user:pass@staging-db/aiwebengine"
   sqlx migrate run
   ```

5. **Deploy to production**
   ```bash
   # Production environment (with backup!)
   pg_dump -Fc aiwebengine > backup_$(date +%Y%m%d_%H%M%S).dump
   
   export DATABASE_URL="postgresql://user:pass@prod-db/aiwebengine"
   sqlx migrate run
   ```

---

## See Also

- [SQLx Documentation](https://github.com/launchbadge/sqlx)
- [SQLx CLI Reference](https://github.com/launchbadge/sqlx/tree/main/sqlx-cli)
- [PostgreSQL Documentation](https://www.postgresql.org/docs/)
