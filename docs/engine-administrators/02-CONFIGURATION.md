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

| Setting             | Local       | Staging    | Production      |
| ------------------- | ----------- | ---------- | --------------- |
| **Purpose**         | Development | Testing    | Live deployment |
| **Log Level**       | `debug`     | `info`     | `info`          |
| **Database**        | PostgreSQL  | PostgreSQL | PostgreSQL      |
| **CORS**            | `["*"]`     | Specific   | Specific        |
| **Cookie Secure**   | `false`     | `true`     | `true`          |
| **Rate Limiting**   | Disabled    | Enabled    | Enabled         |
| **CSRF Protection** | Disabled    | Enabled    | Enabled         |

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

# Enable CORS
export APP_SECURITY__ENABLE_CORS="true"

# CORS allowed origins (JSON array format)
export APP_SECURITY__CORS_ALLOWED_ORIGINS='["https://yourdomain.com"]'

# Enable CSRF protection
export APP_SECURITY__ENABLE_CSRF="true"

# CSRF key (base64-encoded 32-byte key)
export APP_SECURITY__CSRF_KEY="$(openssl rand -base64 32)"

# Session encryption key (base64-encoded 32-byte key)
export APP_SECURITY__SESSION_ENCRYPTION_KEY="$(openssl rand -base64 32)"

# Enable rate limiting
export APP_SECURITY__ENABLE_RATE_LIMITING="true"

# Rate limit per minute
export APP_SECURITY__RATE_LIMIT_PER_MINUTE="1000"

# Maximum request body size in bytes
export APP_SECURITY__MAX_REQUEST_BODY_BYTES="10485760"  # 10 MB
```

#### Server

```bash
# Bind address
export APP_SERVER__HOST="0.0.0.0"

# Port
export APP_SERVER__PORT="8080"

# Base URL for OAuth and redirects
export APP_SERVER__BASE_URL="https://yourdomain.com"

# Request timeout (seconds)

# Keep-alive timeout (seconds)
export APP_SERVER__KEEP_ALIVE_TIMEOUT_SECS="75"

# Maximum concurrent connections
export APP_SERVER__MAX_CONNECTIONS="10000"
```

#### Logging

```bash
# Log level: trace, debug, info, warn, error
export APP_LOGGING__LEVEL="info"

# Log format: json, pretty, compact
export APP_LOGGING__FORMAT="json"

# Enable file logging
export APP_LOGGING__FILE_ENABLED="true"

# Log file path
export APP_LOGGING__FILE_PATH="/var/log/aiwebengine/app.log"

# Log file rotation size in MB
export APP_LOGGING__FILE_MAX_SIZE_MB="100"

# Number of rotated log files to keep
export APP_LOGGING__FILE_MAX_FILES="30"

# Enable console logging
export APP_LOGGING__CONSOLE_ENABLED="true"
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
host = "127.0.0.1"                    # Bind address (0.0.0.0 for all interfaces)
port = 3000                            # Listen port (1-65535)
base_url = "http://localhost:3000"    # Base URL for OAuth and redirects
graceful_shutdown = true               # Enable graceful shutdown
shutdown_timeout_secs = 30            # Shutdown timeout in seconds
```

**Environment overrides:**

```bash
export APP_SERVER__HOST="0.0.0.0"
export APP_SERVER__PORT="8080"
export APP_SERVER__BASE_URL="https://yourdomain.com"
export APP_SERVER__REQUEST_TIMEOUT_SECS="30"
```

### [logging]

Controls application logging.

```toml
[logging]
level = "info"                   # trace | debug | info | warn | error
format = "pretty"                # json | pretty | compact
```

**Environment overrides:**

```bash
export APP_LOGGING__LEVEL="debug"
export APP_LOGGING__FORMAT="json"
export RUST_LOG="aiwebengine=debug"  # Additional Rust logging control
```

### [javascript]

Controls the JavaScript engine (QuickJS).

```toml
[javascript]
execution_timeout_ms = 5000              # Script execution timeout (milliseconds)
max_memory_bytes = 10485760              # Max memory per script (bytes, 10MB)
max_concurrent_executions = 100          # Scripts running at once; the rest queue
stack_size_bytes = 524288                # Script stack (bytes); ~1KB per JS frame
enable_init_functions = true             # Enable script init() function calls
init_timeout_ms = 5000                   # Init function timeout (defaults to execution_timeout_ms)
```

**Environment overrides:**

```bash
export APP_JAVASCRIPT__MAX_MEMORY_BYTES="33554432"  # 32 MB
export APP_JAVASCRIPT__EXECUTION_TIMEOUT_MS="10000"
```

### [repository]

Controls database and script storage. PostgreSQL is the only supported storage backend.

```toml
[repository]
database_url = "postgresql://user:pass@localhost:5432/aiwebengine"
max_script_size_bytes = 1048576          # Maximum script size (bytes, 1MB)
```

### CORS

`cors_allowed_origins` names the origins allowed to read **engine-owned**
responses — `/engine`, `/auth`, `/graphql`, `/mcp`, `/health`, `/.well-known`.
Empty, the default, means same-origin only.

The engine does not apply a policy to script routes. A solution serving a
public API knows which callers it wants and sets its own headers, exactly as it
does for the Content-Security-Policy; a policy applied on its behalf would
either break it or over-permit it.

Entries are matched exactly against the `Origin` a browser sends, after
lower-casing and dropping a trailing slash. `https://example.com` does not match
`https://example.com:8443`, `http://example.com` or
`https://example.com.attacker.test`. An entry that is not an origin — a URL with
a path, a bare hostname, an unexpanded `${VAR}` — is dropped at startup with a
warning rather than kept as something that can never match.

**A named origin may send the session cookie; `"*"` may not.** A browser
refuses to honour `Access-Control-Allow-Origin: *` together with
`Access-Control-Allow-Credentials: true`, so a wildcard grants only
unauthenticated reads. Reflecting an arbitrary origin *and* allowing
credentials would mean every site a signed-in administrator visits could read
`/engine/*` as them, so the engine will not do it.

The OAuth2 protocol endpoints keep a wide policy of their own. Browser-based
MCP clients reach the token endpoint from origins nobody can enumerate, and a
PKCE code exchange carries no cookie.

### Script execution limits

`max_concurrent_executions` bounds how many scripts run at once. Each one holds
a blocking thread and a QuickJS runtime allowed `max_memory_bytes`, so the two
settings together are the engine's memory ceiling. A caller past the limit
waits for a slot rather than being refused, and the wait happens inside the
timeout the call already had — so a saturated engine answers slowly and then
times out, rather than returning an error of its own kind.

`stack_size_bytes` is where QuickJS stops a runaway recursion. A JavaScript
frame costs on the order of a kilobyte, so the number is only meaningful as a
frame count: 512 KB is about 500 frames, 64 KB about 59 — fewer than a
recursive walk over any real structure needs. Exceeding it raises an error the
script can catch; it does not take the process down.

### Script logs

How much of each script's log survives. Both dials are enforced and either one
alone is enough to remove a line: the count bounds a script that logs in a
loop, and the age bounds the dormant scripts that would otherwise sit on their
full quota forever. A background pass applies them on `prune_interval_secs`;
in a cluster one instance does the work on any given tick.

```toml
[logs]
prune_enabled = true                     # Off means the table grows without bound
retention_hours = 24                     # Delete lines older than this; 0 disables the age bound
keep_per_script = 100                    # Keep this many newest lines per script
prune_interval_secs = 3600               # How often a pass runs
```

**Environment overrides:**

```bash
export APP_REPOSITORY__DATABASE_URL="postgresql://user:pass@host/db"
export APP_REPOSITORY__MAX_SCRIPT_SIZE_BYTES="2097152"  # 2 MB
export APP_REPOSITORY__LOG_RETENTION_HOURS="168"  # 7 days
```

**Note:** The `database_url` field is internally renamed to `connection_string` in the code.

### [security]

Controls security features.

```toml
[security]
enable_cors = true                           # Apply a CORS policy to engine paths
cors_allowed_origins = []                    # Origins allowed to read them; empty = same-origin
csrf_key = "${APP_SECURITY__CSRF_KEY}"      # Base64-encoded 32-byte CSRF key
enable_security_headers = true               # Enable security headers
content_security_policy = "default-src 'self'" # Content Security Policy
max_request_body_bytes = 1048576            # Max request body size (bytes, 1MB)
session_encryption_key = "${APP_SECURITY__SESSION_ENCRYPTION_KEY}"  # Base64-encoded 32-byte key
api_key = "${APP_SECURITY__API_KEY}"        # API key (override with env var!)
```

**Environment overrides:**

```bash
export APP_SECURITY__API_KEY="$(openssl rand -hex 32)"
export APP_SECURITY__CSRF_KEY="$(openssl rand -base64 32)"
export APP_SECURITY__SESSION_ENCRYPTION_KEY="$(openssl rand -base64 32)"
export APP_SECURITY__CORS_ALLOWED_ORIGINS='["https://yourdomain.com"]'
export APP_SECURITY__RATE_LIMIT_PER_MINUTE="1000"
export APP_SECURITY__MAX_REQUEST_BODY_BYTES="10485760"  # 10 MB
```

**⚠️ Production:**

- Never hardcode secrets in config files - always use environment variables!
- Enable CSRF protection (`- Use strong encryption keys for CSRF and session encryption
- All server instances must use the same CSRF and session encryption keys

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
secure = false                   # HTTPS only (true in production!)
```

The cookie is host-only and `SameSite=Lax` unconditionally, and `HttpOnly`
always. There are no settings for those: a `Domain` would defeat the per-host
sign-in the `__Host-` prefix enforces, `Strict` would withhold the session on
the cross-site navigation an MCP client makes into `/auth/oauth2/authorize`,
and a cookie readable from JavaScript is not a session cookie.

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

The `[secrets]` configuration section has been removed. Secrets are now stored
exclusively in the database and managed via the `secretStorage` JavaScript API.

See [04-SECRETS-AND-SECURITY.md](04-SECRETS-AND-SECURITY.md) for the complete secrets management guide.

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
- Secure session cookies (`secure = true`)
- Set specific CORS origins
- Enable rate limiting and CSRF protection
- Use production secret management (AWS Secrets Manager, Vault, etc.)
- Set appropriate timeouts
- Use log level `info` or `warn`
- Generate strong CSRF and session encryption keys
- Ensure all server instances use the same encryption keys

```bash
cp config.production.toml config.toml

# Use secret management system
export APP_AUTH__JWT_SECRET="$(aws secretsmanager get-secret-value --secret-id jwt-secret --query SecretString --output text)"
export APP_SECURITY__API_KEY="$(aws secretsmanager get-secret-value --secret-id api-key --query SecretString --output text)"
export APP_SECURITY__CSRF_KEY="$(aws secretsmanager get-secret-value --secret-id csrf-key --query SecretString --output text)"
export APP_SECURITY__SESSION_ENCRYPTION_KEY="$(aws secretsmanager get-secret-value --secret-id session-key --query SecretString --output text)"
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
- Use `cors_allowed_origins = ["*"]` in production — a wildcard cannot carry credentials, so it grants unauthenticated reads and nothing an admin UI needs
- Skip HTTPS for production deployments
- Use weak or short secrets
- Ignore security warnings
- Use different encryption keys across server instances
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

| Section         | Key                           | Type    | Range/Options               | Default               |
| --------------- | ----------------------------- | ------- | --------------------------- | --------------------- |
| `[server]`      | `host`                        | string  | IP address                  | `127.0.0.1`           |
| `[server]`      | `port`                        | integer | 1-65535                     | `8080`                |
| `[server]`      | `base_url`                    | string  | URL                         | `None`                |
| `[server]`      | `graceful_shutdown`           | boolean | true/false                  | `true`                |
| `[server]`      | `shutdown_timeout_secs`       | integer | 1-300                       | `30`                  |
| `[logging]`     | `level`                       | string  | trace/debug/info/warn/error | `info`                |
| `[logging]`     | `format`                      | string  | json/pretty/compact         | `pretty`              |
| `[javascript]`  | `execution_timeout_ms`        | integer | 100-60000                   | `5000`                |
| `[javascript]`  | `max_memory_bytes`            | integer | 1048576-1073741824          | `10485760`            |
| `[javascript]`  | `max_concurrent_executions`   | integer | 1-1000                      | `100`                 |
| `[javascript]`  | `stack_size_bytes`            | integer | 65536-10485760              | `524288`              |
| `[javascript]`  | `enable_init_functions`       | boolean | true/false                  | `true`                |
| `[repository]`  | `database_url`                | string  | Connection string           | (required)            |
| `[repository]`  | `max_script_size_bytes`       | integer | 1024-10485760               | `1048576`             |
| `[logs]`        | `prune_enabled`               | boolean | true/false                  | `true`                |
| `[logs]`        | `retention_hours`             | integer | 0-720 (0 = no age bound)    | `24`                  |
| `[logs]`        | `keep_per_script`             | integer | 1-10000                     | `100`                 |
| `[logs]`        | `prune_interval_secs`         | integer | >= 60                       | `3600`                |
| `[security]`    | `enable_cors`                 | boolean | true/false                  | `true`                |
| `[security]`    | `cors_allowed_origins`        | array   | Origins, or `["*"]`         | `[]`                  |
| `[security]`    | `csrf_key`                    | string  | Base64 key                  | `None`                |
| `[security]`    | `enable_security_headers`     | boolean | true/false                  | `true`                |
| `[security]`    | `max_request_body_bytes`      | integer | 1024-104857600              | `1048576`             |
| `[security]`    | `session_encryption_key`      | string  | Base64 key                  | `None`                |
| `[security]`    | `api_key`                     | string  | API key                     | `None`                |
| `[auth]`        | `enabled`                     | boolean | true/false                  | `true`                |
| `[auth]`        | `jwt_secret`                  | string  | Min 32 chars                | Required              |
| `[auth]`        | `session_timeout`             | integer | 60-604800                   | `3600`                |
| `[auth]`        | `max_concurrent_sessions`     | integer | 1-10                        | `3`                   |
| `[auth]`        | `bootstrap_admins`            | array   | Email addresses             | `[]`                  |
| `[auth.cookie]` | `name`                        | string  | Cookie name                 | `aiwebengine_session` |
| `[auth.cookie]` | `secure`                      | boolean | true/false                  | `false`               |

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
base_url = "https://yourdomain.com"
max_connections = 10000
graceful_shutdown = true
shutdown_timeout_secs = 30

[logging]
level = "info"
format = "json"

[javascript]
execution_timeout_ms = 10000
max_memory_bytes = 134217728  # 128 MB
max_concurrent_executions = 200
stack_size_bytes = 524288
enable_init_functions = true

[repository]
database_url = "${APP_REPOSITORY__DATABASE_URL}"
max_script_size_bytes = 1048576

[logs]
prune_enabled = true
retention_hours = 168
keep_per_script = 1000
prune_interval_secs = 3600

[security]
enable_cors = true
cors_allowed_origins = ["https://yourdomain.com"]
csrf_key = "${APP_SECURITY__CSRF_KEY}"
enable_security_headers = true
content_security_policy = "default-src 'self'; script-src 'self'; style-src 'self'"
max_request_body_bytes = 10485760
session_encryption_key = "${APP_SECURITY__SESSION_ENCRYPTION_KEY}"
api_key = "${APP_SECURITY__API_KEY}"

[auth]
jwt_secret = "${APP_AUTH__JWT_SECRET}"
session_timeout = 7200
max_concurrent_sessions = 3
bootstrap_admins = ["admin@yourdomain.com"]

[auth.cookie]
name = "aiwebengine_session"
path = "/"
secure = true

[auth.providers.google]
client_id = "${APP_AUTH__PROVIDERS__GOOGLE__CLIENT_ID}"
client_secret = "${APP_AUTH__PROVIDERS__GOOGLE__CLIENT_SECRET}"
redirect_uri = "https://yourdomain.com/auth/callback/google"
scopes = ["openid", "email", "profile"]

[secrets]
# All secrets via environment variables in production!
api_key = "${APP_SECURITY__API_KEY}"
require_https = true

[auth]
jwt_secret = "${APP_AUTH__JWT_SECRET}"
session_timeout = 7200
bootstrap_admins = ["admin@yourdomain.com"]

[auth.cookie]
secure = true

[auth.providers.google]
client_id = "${APP_AUTH__PROVIDERS__GOOGLE__CLIENT_ID}"
client_secret = "${APP_AUTH__PROVIDERS__GOOGLE__CLIENT_SECRET}"
redirect_uri = "https://yourdomain.com/auth/callback/google"

```

---

## Related Documentation

- **[Getting Started](01-GETTING-STARTED.md)** - First-time setup
- **[Running Environments](03-RUNNING-ENVIRONMENTS.md)** - Deploy to different environments
- **[Secrets and Security](04-SECRETS-AND-SECURITY.md)** - Manage secrets and OAuth
- **[Quick Reference](QUICK-REFERENCE.md)** - Fast command lookup
