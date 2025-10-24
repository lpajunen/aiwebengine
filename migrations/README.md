# Database Migrations

This directory contains SQL migration files for AIWebEngine's PostgreSQL database.

## Migration Files

Migrations are applied in sequential order by filename:

- `20241024000001_create_scripts.sql` - Scripts table
- `20241024000002_create_assets.sql` - Assets table
- `20241024000003_create_logs.sql` - Logs table
- `20241024000004_create_route_registrations.sql` - Route registrations table

## Running Migrations

### Automatic (Development)

Set in `config.toml`:
```toml
[repository]
auto_migrate = true
```

The server will run migrations on startup.

### Manual (Production - Recommended)

```bash
# Install SQLx CLI
cargo install sqlx-cli --no-default-features --features postgres

# Set database URL
export DATABASE_URL="postgresql://user:password@localhost:5432/aiwebengine"

# Run migrations
sqlx migrate run

# Check status
sqlx migrate info
```

## Creating New Migrations

```bash
# Create new migration
sqlx migrate add <description>

# Example
sqlx migrate add add_user_preferences

# Edit the generated file: migrations/YYYYMMDDHHMMSS_add_user_preferences.sql
```

## Documentation

See `docs/engine-administrators/DATABASE-MIGRATIONS.md` for detailed guide.
