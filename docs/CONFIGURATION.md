# Configuration Documentation

## Overview

AIWebEngine uses a flexible, hierarchical configuration system built with [Figment](https://docs.rs/figment/). This allows configuration from multiple sources with a clear precedence order.

## Configuration Sources (in order of precedence)

1. **Environment Variables** (highest precedence)
2. **Configuration Files** (TOML, YAML, JSON5)
3. **Default Values** (lowest precedence)

## Environment Variables

All configuration values can be overridden using environment variables with the `APP_` prefix:

```bash
# Server configuration
export APP_SERVER_HOST="0.0.0.0"
export APP_SERVER_PORT="8080"
export APP_SERVER_REQUEST_TIMEOUT_MS="30000"

# Logging configuration
export APP_LOGGING_LEVEL="info"
export APP_LOGGING_FILE_PATH="/var/log/aiwebengine.log"

# Database configuration
export APP_REPOSITORY_DATABASE_URL="postgresql://user:pass@host/db"
export APP_REPOSITORY_MAX_CONNECTIONS="50"

# Security configuration
export APP_SECURITY_API_KEY="your-secret-api-key"
export APP_SECURITY_REQUIRE_HTTPS="true"
```

## Configuration Files

The system automatically looks for configuration files in this order:

- `config.toml`
- `config.yaml`
- `config.yml`

### Example Usage

```bash
# Development
cp config.dev.toml config.toml

# Staging
cp config.staging.toml config.toml

# Production
cp config.prod.toml config.toml
```

## Configuration Structure

### Server Configuration

```toml
[server]
host = "127.0.0.1"              # Bind address
port = 3000                      # Port number
request_timeout_ms = 5000        # Request timeout in milliseconds
max_body_size_mb = 1            # Maximum request body size in MB
```

### Logging Configuration

```toml
[logging]
level = "info"                   # Log level: trace, debug, info, warn, error
structured = true                # Enable structured JSON logging
targets = ["console", "file"]    # Log outputs: console, file, or both
file_path = "./logs/app.log"    # Log file path (when file target enabled)
rotation = "daily"               # Log rotation: hourly, daily, weekly
retention_days = 7               # Days to keep old log files
```

### JavaScript Engine Configuration

```toml
[javascript]
max_memory_mb = 16              # Maximum memory per JS context
execution_timeout_ms = 1000      # Script execution timeout
max_stack_size = 65536          # Maximum stack size
enable_console = true            # Enable console.log in scripts
allowed_apis = [                # Available APIs for scripts
    "fetch",                    # HTTP requests
    "database",                 # Database operations
    "logging",                  # Logging functions
    "filesystem"                # File operations (dev only)
]
```

### Repository Configuration

```toml
[repository]
database_type = "sqlite"         # Database type: sqlite, postgresql
database_url = "./app.db"       # Database connection string
max_connections = 5              # Connection pool size
connection_timeout_ms = 2000     # Connection timeout
auto_migrate = true              # Auto-run database migrations
```

### Security Configuration

```toml
[security]
enable_cors = true               # Enable CORS headers
cors_origins = ["*"]            # Allowed CORS origins
rate_limit_per_minute = 0        # Rate limit (0 = disabled)
api_key = "dev-key"             # API key for authentication
require_https = false            # Require HTTPS connections
allowed_content_types = [        # Allowed request content types
    "application/json",
    "application/x-www-form-urlencoded"
]
```

### Performance Configuration

```toml
[performance]
cache_size_mb = 10              # Cache size in MB
cache_ttl_seconds = 60          # Cache time-to-live
enable_compression = false       # Enable response compression
worker_pool_size = 2            # Worker thread pool size
max_request_queue_size = 50     # Maximum queued requests
```

## Validation

The configuration system includes comprehensive validation:

- **Port ranges**: 1-65535
- **Memory limits**: 1-1024 MB
- **Timeout ranges**: 100ms-300000ms (5 minutes)
- **Log levels**: trace, debug, info, warn, error
- **Database types**: sqlite, postgresql
- **Required fields**: All configuration sections have sensible defaults

## Error Handling

Invalid configurations will cause the application to exit with detailed error messages:

```
Configuration validation failed: Server port 99999 is out of valid range (1-65535)
```

## Best Practices

### Development

- Use `config.dev.toml` with verbose logging and relaxed security
- Enable console logging and filesystem APIs for debugging
- Use SQLite for simple local development

### Staging

- Use `config.staging.toml` that mirrors production structure
- Enable auto-migrations for testing schema changes
- Use moderate security settings for integration testing

### Production

- Use `config.prod.toml` with minimal logging and strict security
- Disable auto-migrations and console APIs
- Use PostgreSQL with connection pooling
- Set secrets via environment variables, not config files

### Environment Variables in Production

```bash
# Never put secrets in config files
export APP_SECURITY_API_KEY="$(cat /etc/secrets/api-key)"
export APP_REPOSITORY_DATABASE_URL="postgresql://$(cat /etc/secrets/db-user):$(cat /etc/secrets/db-pass)@localhost/aiwebengine"
```

## Configuration Testing

Test your configuration with:

```bash
# Validate configuration without starting server
cargo run --bin server -- --validate-config

# Start with specific config file
RUST_LOG=info cargo run --bin server
```
