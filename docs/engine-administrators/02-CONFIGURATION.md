# 02 - Configuration

Complete reference for configuring aiwebengine using configuration files and environment variables.

## Quick Navigation

- [Configuration System Overview](#configuration-system-overview)
- [Configuration Files](#configuration-files)
- [Environment Variables](#environment-variables)
- [Configuration Sections](#configuration-sections)
- [Best Practices](#best-practices)
- [Validation](#validation)

---

## Configuration System Overview

aiwebengine uses a flexible, hierarchical configuration system that combines multiple sources with clear precedence rules.

### Configuration Sources (Precedence Order)

1. **Environment Variables** (highest) - Override everything
2. **Configuration File** (`config.toml`) - Main configuration
3. **Default Values** (lowest) - Built-in fallbacks

### When to Use Each

| Method                    | Use Case                            | Example                          |
| ------------------------- | ----------------------------------- | -------------------------------- |
| **Environment Variables** | Secrets, deployment-specific values | OAuth credentials, database URLs |
| **Configuration File**    | Environment defaults, structure     | Server settings, logging config  |
| **Default Values**        | Development convenience             | Testing with minimal setup       |

---

## Configuration Files

### Available Templates

aiwebengine provides three pre-configured environment templates:

```bash
# Local Development
cp config.local.toml config.toml

# Staging Environment
cp config.staging.toml config.toml

# Production Deployment
cp config.production.toml config.toml
```

### Environment Comparison

| Setting            | Local       | Staging    | Production      |
| ------------------ | ----------- | ---------- | --------------- |
| **Purpose**        | Development | Testing    | Live deployment |
| **Log Level**      | `debug`     | `info`     | `warn`          |
| **Database**       | PostgreSQL  | PostgreSQL | PostgreSQL      |
| **Auto-migrate**   | `true`      | `true`     | `false`         |
| **HTTPS Required** | `false`     | `true`     | `true`          |
| **CORS**           | `["*"]`     | Specific   | Specific        |
| **Console API**    | Enabled     | Enabled    | Disabled        |
| **Cookie Secure**  | `false`     | `true`     | `true`          |
| **Rate Limiting**  | Disabled    | Enabled    | Enabled         |

---

## Environment Variables

### Naming Convention

Environment variables use the `APP_` prefix with **double underscores (`__`)** to represent nested configuration:

```bash
# Format: APP__{SECTION}__{SUBSECTION}__{KEY}

# Example: [server] host
export APP_SERVER__HOST="0.0.0.0"

# Example: [auth] jwt_secret
export APP_AUTH__JWT_SECRET="secret-value"

# Example: [auth.providers.google] client_id
export APP_AUTH__PROVIDERS__GOOGLE__CLIENT_ID="client-id"
```

### Critical Variables

#### Authentication

```bash
# JWT secret for session management (minimum 32 chars, recommend 48+ bytes)
export APP_AUTH__JWT_SECRET="$(openssl rand -base64 48)"

# Google OAuth
export APP_AUTH__PROVIDERS__GOOGLE__CLIENT_ID="your-client-id.apps.googleusercontent.com"
export APP_AUTH__PROVIDERS__GOOGLE__CLIENT_SECRET="your-client-secret"
export APP_AUTH__PROVIDERS__GOOGLE__REDIRECT_URI="http://localhost:3000/auth/callback/google"

# Bootstrap administrators (JSON array format)
export APP_AUTH__BOOTSTRAP_ADMINS='["admin@example.com","your-email@example.com"]'
```

#### Database

```bash
# PostgreSQL connection string
export APP_REPOSITORY__DATABASE_URL="postgresql://user:password@host:5432/database"

# Connection pool size
export APP_REPOSITORY__MAX_CONNECTIONS="50"
```

#### Security

```bash
# API key for endpoint authentication
export APP_SECURITY__API_KEY="$(openssl rand -hex 32)"

# Require HTTPS
export APP_SECURITY__REQUIRE_HTTPS="true"

# CORS origins (JSON array format)
export APP_SECURITY__CORS_ORIGINS='["https://yourdomain.com"]'
```

#### Server

```bash
# Bind address
export APP_SERVER__HOST="0.0.0.0"

# Port
export APP_SERVER__PORT="8080"

# Request timeout (milliseconds)
export APP_SERVER__REQUEST_TIMEOUT_MS="30000"
```

#### Logging

```bash
# Log level: trace, debug, info, warn, error
export APP_LOGGING__LEVEL="info"

# Log file path
export APP_LOGGING__FILE_PATH="/var/log/aiwebengine/app.log"

# Enable structured JSON logging
export APP_LOGGING__STRUCTURED="true"
```

### Using .env Files

For local development, use `.env` files with the `source` command:

```bash
# 1. Create .env file
cp .env.example .env

# 2. Edit with your values (NO 'export' keyword in .env files)
nano .env

# 3. Load and run
source .env && cargo run
```

**Example .env contents:**

```bash
APP_AUTH__JWT_SECRET=your-jwt-secret-here
APP_AUTH__PROVIDERS__GOOGLE__CLIENT_ID=your-client-id
APP_AUTH__PROVIDERS__GOOGLE__CLIENT_SECRET=your-client-secret
APP_SECURITY__API_KEY=your-api-key
```

### Array Values in Environment Variables

Arrays must use **JSON array format**:

```bash
# ✅ Correct - JSON array
export APP_AUTH__BOOTSTRAP_ADMINS='["email1@example.com","email2@example.com"]'
export APP_SECURITY__CORS_ORIGINS='["https://app1.com","https://app2.com"]'

# ❌ Incorrect - won't work
export APP_AUTH__BOOTSTRAP_ADMINS=email1@example.com,email2@example.com
```

---

## Configuration Sections

### [server]

Controls the HTTP server behavior.

```toml
[server]
host = "127.0.0.1"              # Bind address (0.0.0.0 for all interfaces)
port = 3000                      # Listen port (1-65535)
request_timeout_ms = 5000        # Request timeout (100-300000)
max_body_size_mb = 1            # Max request body size (1-100 MB)
```

**Environment overrides:**

```bash
export APP_SERVER__HOST="0.0.0.0"
export APP_SERVER__PORT="8080"
```

### [logging]

Controls application logging.

```toml
[logging]
level = "info"                   # trace | debug | info | warn | error
structured = true                # Enable JSON logging
targets = ["console", "file"]    # Where to log: console, file, or both
file_path = "./logs/app.log"    # Log file path
rotation = "daily"               # hourly | daily | weekly
retention_days = 7               # Days to keep old logs (1-365)
```

**Environment overrides:**

```bash
export APP_LOGGING__LEVEL="debug"
export APP_LOGGING__FILE_PATH="/var/log/aiwebengine.log"
export RUST_LOG="aiwebengine=debug"  # Additional Rust logging control
```

### [javascript]

Controls the JavaScript engine (QuickJS).

```toml
[javascript]
max_memory_mb = 16              # Max memory per JS context (1-1024)
execution_timeout_ms = 1000      # Script timeout (100-60000)
max_stack_size = 65536          # Stack size (8192-1048576)
enable_console = true            # Allow console.log in scripts
allowed_apis = [                # Available APIs for scripts
    "fetch",                    # HTTP requests
    "database",                 # Database operations
    "logging",                  # Logging functions
    "filesystem"                # File operations (dev only!)
]
```

**Environment overrides:**

```bash
export APP_JAVASCRIPT__MAX_MEMORY_MB="32"
export APP_JAVASCRIPT__ENABLE_CONSOLE="false"
```

**⚠️ Security Note:** Disable `filesystem` API in production and `enable_console` should be `false` in production.

### [repository]

Controls database and script storage.

```toml
[repository]
database_type = "postgresql"     # postgresql | memory
database_url = "postgresql://user:pass@localhost:5432/aiwebengine"
max_connections = 5              # Connection pool size (1-100)
connection_timeout_ms = 2000     # Connection timeout (100-30000)
auto_migrate = true              # Auto-run migrations (disable in prod!)
```

**Environment overrides:**

```bash
export APP_REPOSITORY__DATABASE_URL="postgresql://user:pass@host/db"
export APP_REPOSITORY__MAX_CONNECTIONS="50"
export APP_REPOSITORY__AUTO_MIGRATE="false"
```

**Production tip:** Set `auto_migrate = false` and run migrations manually:

```bash
# Manual migration (when needed)
cargo run --bin migrate
```

### [security]

Controls security features.

```toml
[security]
enable_cors = true               # Enable CORS
cors_origins = ["*"]            # Allowed origins (use ["*"] for dev only!)
rate_limit_per_minute = 0        # Requests per minute (0 = disabled)
api_key = "dev-key"             # API key (override with env var!)
require_https = false            # Require HTTPS (true in production)
allowed_content_types = [        # Allowed request content types
    "application/json",
    "application/x-www-form-urlencoded",
    "multipart/form-data"
]
```

**Environment overrides:**

```bash
export APP_SECURITY__API_KEY="$(openssl rand -hex 32)"
export APP_SECURITY__REQUIRE_HTTPS="true"
export APP_SECURITY__CORS_ORIGINS='["https://yourdomain.com"]'
export APP_SECURITY__RATE_LIMIT_PER_MINUTE="100"
```

**⚠️ Production:** Never hardcode `api_key` in config files - always use environment variables!

### [performance]

Controls performance optimizations.

```toml
[performance]
cache_size_mb = 10              # Response cache size (1-1024)
cache_ttl_seconds = 60          # Cache time-to-live (1-86400)
enable_compression = false       # Enable gzip compression
worker_pool_size = 2            # Worker threads (1-32)
max_request_queue_size = 50     # Max queued requests (10-10000)
```

**Environment overrides:**

```bash
export APP_PERFORMANCE__CACHE_SIZE_MB="100"
export APP_PERFORMANCE__ENABLE_COMPRESSION="true"
export APP_PERFORMANCE__WORKER_POOL_SIZE="4"
```

### [auth]

Controls authentication and authorization.

```toml
[auth]
enabled = true                   # Enable authentication
jwt_secret = "your-secret-here"  # JWT signing key (override with env!)
session_timeout = 3600           # Session timeout in seconds (1 hour)
max_concurrent_sessions = 10     # Max sessions per user
bootstrap_admins = [             # Auto-grant admin role to these emails
    "admin@example.com"
]

[auth.cookie]
name = "aiwebengine_session"     # Session cookie name
path = "/"                       # Cookie path
secure = false                   # HTTPS only (true in production!)
http_only = true                 # No JavaScript access
same_site = "lax"               # lax | strict | none
```

**Environment overrides:**

```bash
export APP_AUTH__JWT_SECRET="$(openssl rand -base64 48)"
export APP_AUTH__SESSION_TIMEOUT="7200"
export APP_AUTH__BOOTSTRAP_ADMINS='["admin@example.com"]'
```

### [auth.providers.google]

Google OAuth configuration.

```toml
[auth.providers.google]
client_id = "${APP_AUTH__PROVIDERS__GOOGLE__CLIENT_ID}"
client_secret = "${APP_AUTH__PROVIDERS__GOOGLE__CLIENT_SECRET}"
redirect_uri = "http://localhost:3000/auth/callback/google"
scopes = ["openid", "email", "profile"]
```

**Environment overrides:**

```bash
export APP_AUTH__PROVIDERS__GOOGLE__CLIENT_ID="your-id.apps.googleusercontent.com"
export APP_AUTH__PROVIDERS__GOOGLE__CLIENT_SECRET="your-secret"
export APP_AUTH__PROVIDERS__GOOGLE__REDIRECT_URI="https://yourdomain.com/auth/callback/google"
```

See [04-SECRETS-AND-SECURITY.md](04-SECRETS-AND-SECURITY.md) for setting up OAuth providers.

### [secrets]

Secrets management for AI services and external APIs.

```toml
[secrets.values]
# ⚠️ For development ONLY - use environment variables in production!
anthropic_api_key = "${SECRET_ANTHROPIC_API_KEY}"
openai_api_key = "${SECRET_OPENAI_API_KEY}"
```

**Environment overrides (recommended):**

```bash
# Any SECRET_ prefixed variable becomes available as a secret
export SECRET_ANTHROPIC_API_KEY="sk-ant-api03-..."
export SECRET_OPENAI_API_KEY="sk-..."
export SECRET_STRIPE_API_KEY="sk_live_..."
```

See [04-SECRETS-AND-SECURITY.md](04-SECRETS-AND-SECURITY.md) for complete secrets management guide.

---

## Best Practices

### Local Development

✅ **DO:**

- Use `config.local.toml` as base
- Store secrets in `.env` file (add to `.gitignore`)
- Enable verbose logging (`level = "debug"`)
- Use `localhost` addresses
- Enable console APIs for debugging
- Use auto-migrations

```bash
cp config.local.toml config.toml
cp .env.example .env
# Edit .env with your credentials
source .env && cargo run
```

### Staging Environment

✅ **DO:**

- Use `config.staging.toml` as base
- Set all secrets via environment variables
- Mirror production structure
- Test with real OAuth providers
- Use staging-specific callback URLs
- Enable auto-migrations for testing schema changes
- Use moderate security settings

```bash
cp config.staging.toml config.toml
export APP_AUTH__JWT_SECRET="$(openssl rand -base64 48)"
export APP_REPOSITORY__DATABASE_URL="postgresql://user:pass@staging-db/aiwebengine"
export APP_AUTH__PROVIDERS__GOOGLE__REDIRECT_URI="https://staging.yourdomain.com/auth/callback/google"
cargo run --release
```

### Production Deployment

✅ **DO:**

- Use `config.production.toml` as base
- **NEVER** store secrets in config files
- Use strong, randomly generated secrets
- Require HTTPS (`require_https = true`)
- Secure session cookies (`secure = true`)
- Disable auto-migrations (`auto_migrate = false`)
- Disable console API (`enable_console = false`)
- Remove filesystem API from `allowed_apis`
- Set specific CORS origins
- Enable rate limiting
- Use production secret management (AWS Secrets Manager, Vault, etc.)
- Set appropriate timeouts
- Use log level `warn` or `error`

```bash
cp config.production.toml config.toml

# Use secret management system
export APP_AUTH__JWT_SECRET="$(aws secretsmanager get-secret-value --secret-id jwt-secret --query SecretString --output text)"
export APP_SECURITY__API_KEY="$(aws secretsmanager get-secret-value --secret-id api-key --query SecretString --output text)"
export APP_REPOSITORY__DATABASE_URL="postgresql://user:$(cat /secrets/db-pass)@db.prod.example.com/aiwebengine"

# OAuth with production URLs
export APP_AUTH__PROVIDERS__GOOGLE__CLIENT_ID="$(aws secretsmanager get-secret-value --secret-id google-client-id --query SecretString --output text)"
export APP_AUTH__PROVIDERS__GOOGLE__CLIENT_SECRET="$(aws secretsmanager get-secret-value --secret-id google-client-secret --query SecretString --output text)"
export APP_AUTH__PROVIDERS__GOOGLE__REDIRECT_URI="https://yourdomain.com/auth/callback/google"

cargo run --release
```

❌ **DON'T:**

- Commit secrets to Git
- Use development configs in production
- Hardcode API keys or passwords
- Use `CORS = ["*"]` in production
- Enable filesystem API in production
- Skip HTTPS in production
- Use weak or short secrets
- Ignore security warnings

---

## Validation

The configuration system validates all settings on startup.

### Validation Rules

- **Ports:** Must be 1-65535
- **Memory limits:** 1-1024 MB
- **Timeouts:** 100ms-300000ms (5 minutes)
- **Log levels:** Must be one of: trace, debug, info, warn, error
- **Database types:** Must be: postgresql or memory
- **JWT secret:** Minimum 32 characters
- **Required fields:** All sections have defaults, but secrets must be set

### Testing Configuration

```bash
# Validate without starting server
cargo run -- --validate-config

# Check for errors
echo $?  # 0 = success, non-zero = validation failed
```

### Common Validation Errors

**Error:** "JWT secret must be at least 32 characters"

```bash
# Generate proper secret
export APP_AUTH__JWT_SECRET="$(openssl rand -base64 48)"
```

**Error:** "Database URL is required"

```bash
export APP_REPOSITORY__DATABASE_URL="postgresql://user:pass@host/db"
```

**Error:** "Invalid log level: 'verbose'"

```bash
# Use valid level
export APP_LOGGING__LEVEL="debug"
```

---

## Configuration Reference Table

Quick lookup for all available settings:

| Section         | Key                     | Type    | Range/Options               | Default       |
| --------------- | ----------------------- | ------- | --------------------------- | ------------- |
| `[server]`      | `host`                  | string  | IP address                  | `127.0.0.1`   |
| `[server]`      | `port`                  | integer | 1-65535                     | `3000`        |
| `[server]`      | `request_timeout_ms`    | integer | 100-300000                  | `5000`        |
| `[server]`      | `max_body_size_mb`      | integer | 1-100                       | `1`           |
| `[logging]`     | `level`                 | string  | trace/debug/info/warn/error | `info`        |
| `[logging]`     | `structured`            | boolean | true/false                  | `true`        |
| `[logging]`     | `targets`               | array   | console, file               | `["console"]` |
| `[logging]`     | `rotation`              | string  | hourly/daily/weekly         | `daily`       |
| `[logging]`     | `retention_days`        | integer | 1-365                       | `7`           |
| `[javascript]`  | `max_memory_mb`         | integer | 1-1024                      | `16`          |
| `[javascript]`  | `execution_timeout_ms`  | integer | 100-60000                   | `1000`        |
| `[javascript]`  | `enable_console`        | boolean | true/false                  | `true`        |
| `[repository]`  | `database_type`         | string  | postgresql/memory           | `postgresql`  |
| `[repository]`  | `max_connections`       | integer | 1-100                       | `5`           |
| `[repository]`  | `auto_migrate`          | boolean | true/false                  | `true`        |
| `[security]`    | `enable_cors`           | boolean | true/false                  | `true`        |
| `[security]`    | `require_https`         | boolean | true/false                  | `false`       |
| `[security]`    | `rate_limit_per_minute` | integer | 0-10000                     | `0`           |
| `[auth]`        | `enabled`               | boolean | true/false                  | `true`        |
| `[auth]`        | `session_timeout`       | integer | 300-86400                   | `3600`        |
| `[auth.cookie]` | `secure`                | boolean | true/false                  | `false`       |
| `[performance]` | `cache_size_mb`         | integer | 1-1024                      | `10`          |
| `[performance]` | `enable_compression`    | boolean | true/false                  | `false`       |
| `[performance]` | `worker_pool_size`      | integer | 1-32                        | `2`           |

---

## Example Configurations

### Minimal Local Config

```toml
[server]
host = "127.0.0.1"
port = 3000

[logging]
level = "debug"

[repository]
database_type = "postgresql"
database_url = "${APP_REPOSITORY__DATABASE_URL}"

[auth]
jwt_secret = "${APP_AUTH__JWT_SECRET}"
bootstrap_admins = ["your-email@gmail.com"]

[auth.providers.google]
client_id = "${APP_AUTH__PROVIDERS__GOOGLE__CLIENT_ID}"
client_secret = "${APP_AUTH__PROVIDERS__GOOGLE__CLIENT_SECRET}"
redirect_uri = "http://localhost:3000/auth/callback/google"
```

### Production-Ready Config

```toml
[server]
host = "0.0.0.0"
port = 3000
request_timeout_ms = 30000
max_body_size_mb = 10

[logging]
level = "warn"
structured = true
targets = ["console", "file"]
file_path = "/var/log/aiwebengine/app.log"
rotation = "daily"
retention_days = 30

[javascript]
max_memory_mb = 64
execution_timeout_ms = 5000
enable_console = false
allowed_apis = ["fetch", "database", "logging"]

[repository]
database_type = "postgresql"
database_url = "${APP_REPOSITORY__DATABASE_URL}"
max_connections = 50
auto_migrate = false

[security]
enable_cors = true
cors_origins = ["https://yourdomain.com"]
rate_limit_per_minute = 100
api_key = "${APP_SECURITY__API_KEY}"
require_https = true

[auth]
jwt_secret = "${APP_AUTH__JWT_SECRET}"
session_timeout = 7200
bootstrap_admins = ["admin@yourdomain.com"]

[auth.cookie]
secure = true
http_only = true
same_site = "strict"

[auth.providers.google]
client_id = "${APP_AUTH__PROVIDERS__GOOGLE__CLIENT_ID}"
client_secret = "${APP_AUTH__PROVIDERS__GOOGLE__CLIENT_SECRET}"
redirect_uri = "https://yourdomain.com/auth/callback/google"

[performance]
cache_size_mb = 256
enable_compression = true
worker_pool_size = 8
```

---

## Related Documentation

- **[Getting Started](01-GETTING-STARTED.md)** - First-time setup
- **[Running Environments](03-RUNNING-ENVIRONMENTS.md)** - Deploy to different environments
- **[Secrets and Security](04-SECRETS-AND-SECURITY.md)** - Manage secrets and OAuth
- **[Quick Reference](QUICK-REFERENCE.md)** - Fast command lookup
