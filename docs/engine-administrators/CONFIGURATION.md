# Configuration Documentation

## Overview

AIWebEngine uses a flexible, hierarchical configuration system built with [Figment](https://docs.rs/figment/). This allows configuration from multiple sources with a clear precedence order.

## Quick Start

### Local Development

```bash
# 1. Copy the local development config
cp config.local.toml config.toml

# 2. Set up environment variables
cp .env.example .env
# Edit .env with your OAuth credentials and secrets

# 3. Source environment variables and run
source .env && cargo run
```

### Staging/Production

```bash
# 1. Choose appropriate config
cp config.staging.toml config.toml      # or config.production.toml

# 2. Set environment variables (use your secret management system)
export APP_AUTH_JWT_SECRET="$(openssl rand -base64 48)"
export APP_SECURITY_API_KEY="$(openssl rand -hex 32)"
export APP_REPOSITORY_DATABASE_URL="postgresql://user:pass@host/db"
# ... set other required variables

# 3. Run the application
cargo run --release
```

## Configuration Files

AIWebEngine provides three environment-specific configuration files:

| File | Purpose | Database | Security | Use Case |
|------|---------|----------|----------|----------|
| `config.local.toml` | Local development | SQLite | Relaxed | Development, debugging |
| `config.staging.toml` | Testing environment | PostgreSQL | Moderate | Integration testing, QA |
| `config.production.toml` | Production deployment | PostgreSQL | Strict | Production servers |

**Usage**: Copy the appropriate file to `config.toml`:
```bash
cp config.local.toml config.toml        # Local development
cp config.staging.toml config.toml      # Staging
cp config.production.toml config.toml   # Production
```

**Best Practice**: Add `config.toml` to `.gitignore` to avoid committing environment-specific configurations.

## Configuration Sources (in order of precedence)

1. **Environment Variables** (highest precedence) - prefix with `APP_`
2. **Configuration File** (`config.toml`) - TOML format
3. **Default Values** (lowest precedence) - built into the application

## Environment Variables

All configuration values can be overridden using environment variables with the `APP_` prefix. Use double underscores (`__`) for nested configuration:

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

# Authentication - OAuth providers
export APP_AUTH_PROVIDERS_GOOGLE_CLIENT_ID="your-client-id.apps.googleusercontent.com"
export APP_AUTH_PROVIDERS_GOOGLE_CLIENT_SECRET="your-client-secret"
```

### Environment Variable Template

See `.env.example` for a complete template of all available environment variables:

```bash
cp .env.example .env
# Edit .env with your values
source .env  # or use direnv for automatic loading
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

### Secrets Management Configuration

AIWebEngine includes a secure secrets management system that keeps sensitive values (like API keys) in Rust and never exposes them to JavaScript code.

#### Environment Variables (Recommended for Production)

Any environment variable with the `SECRET_` prefix is automatically loaded as a secret:

```bash
# Anthropic API key for AI assistant
export SECRET_ANTHROPIC_API_KEY="sk-ant-api03-..."

# OpenAI API key (if using OpenAI)
export SECRET_OPENAI_API_KEY="sk-..."

# Custom secrets for your application
export SECRET_MY_SERVICE_TOKEN="token123"
export SECRET_DATABASE_ENCRYPTION_KEY="key456"
```

The identifier is the lowercase version of the part after `SECRET_`. For example:

- `SECRET_ANTHROPIC_API_KEY` → identifier: `anthropic_api_key`
- `SECRET_MY_SERVICE_TOKEN` → identifier: `my_service_token`

#### Configuration File (Development Only)

For local development, you can configure secrets in your config file:

```toml
[secrets]
# Direct values (development only - never commit these!)
[secrets.values]
anthropic_api_key = "sk-ant-api03-..."
openai_api_key = "sk-..."

# Or reference environment variables
[secrets.values]
anthropic_api_key = "${ANTHROPIC_API_KEY}"
database_password = "${DB_PASSWORD}"
```

**⚠️ SECURITY WARNING**: Never commit secrets to version control! Use environment variables in production.

#### JavaScript API

Scripts can check if secrets are configured without accessing their values:

```javascript
// Check if a secret exists
if (Secrets.exists("anthropic_api_key")) {
  console.log("API key is configured");
} else {
  console.log("Please configure SECRET_ANTHROPIC_API_KEY");
}

// List all configured secret identifiers
const availableSecrets = Secrets.list();
console.log("Available secrets:", availableSecrets);
// Output: ['anthropic_api_key', 'openai_api_key']

// ❌ Secret values are NOT accessible from JavaScript
// Secrets.get('anthropic_api_key'); // This function does not exist!
```

Secret values are automatically injected by the Rust layer when making HTTP requests:

```javascript
// Secrets are injected using template syntax
const response = await fetch("https://api.anthropic.com/v1/messages", {
  method: "POST",
  headers: {
    "x-api-key": "{{secret:anthropic_api_key}}", // Injected by Rust
    "content-type": "application/json",
  },
  body: JSON.stringify({
    model: "claude-3-haiku-20240307",
    messages: [{ role: "user", content: "Hello!" }],
  }),
});
```

#### Common Secrets for AI Integration

```bash
# Anthropic Claude
export SECRET_ANTHROPIC_API_KEY="sk-ant-api03-..."

# OpenAI
export SECRET_OPENAI_API_KEY="sk-..."

# Google/Gemini
export SECRET_GOOGLE_API_KEY="..."

# Custom API services
export SECRET_STRIPE_API_KEY="sk_live_..."
export SECRET_SENDGRID_API_KEY="SG..."
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

### Local Development (`config.local.toml`)

- ✅ Use SQLite for simple setup (`database_type = "sqlite"`)
- ✅ Enable verbose logging (`level = "debug"`)
- ✅ Enable console and filesystem APIs for debugging
- ✅ Use `.env` file for OAuth credentials (not committed to git)
- ✅ Disable HTTPS requirement (`require_https = false`)
- ✅ Add your email to `bootstrap_admins` array

```bash
# Local development setup
cp config.local.toml config.toml
cp .env.example .env
# Edit .env with your OAuth credentials
source .env && cargo run
```

### Staging Environment (`config.staging.toml`)

- ✅ Use PostgreSQL that mirrors production
- ✅ Enable auto-migrations for testing schema changes
- ✅ Use moderate security settings for integration testing
- ✅ Set all secrets via environment variables
- ✅ Use staging-specific OAuth callback URLs

```bash
# Staging setup
cp config.staging.toml config.toml

# Set environment variables in your CI/CD or deployment system
export APP_AUTH_JWT_SECRET="$(openssl rand -base64 48)"
export APP_REPOSITORY_DATABASE_URL="postgresql://user:pass@staging-db/aiwebengine"
export APP_AUTH_PROVIDERS_GOOGLE_CLIENT_ID="..."
export APP_AUTH_PROVIDERS_GOOGLE_CLIENT_SECRET="..."
export APP_AUTH_PROVIDERS_GOOGLE_REDIRECT_URI="https://staging.yourdomain.com/auth/callback/google"

cargo run --release
```

### Production Deployment (`config.production.toml`)

- ✅ Use PostgreSQL with connection pooling
- ✅ Disable auto-migrations (`auto_migrate = false`)
- ✅ Disable console APIs (`enable_console = false`)
- ✅ Use strict security settings
- ✅ Require HTTPS (`require_https = true`)
- ✅ Set ALL secrets via environment variables, NEVER in config files
- ✅ Use secure session cookies (`secure = true`)
- ✅ Use production secret management (AWS Secrets Manager, HashiCorp Vault, etc.)

```bash
# Production setup - NEVER put secrets in config files!
cp config.production.toml config.toml

# Use your secret management system
export APP_AUTH_JWT_SECRET="$(aws secretsmanager get-secret-value --secret-id jwt-secret --query SecretString --output text)"
export APP_SECURITY_API_KEY="$(aws secretsmanager get-secret-value --secret-id api-key --query SecretString --output text)"
export APP_REPOSITORY_DATABASE_URL="postgresql://$(cat /etc/secrets/db-user):$(cat /etc/secrets/db-pass)@db.example.com/aiwebengine"

# OAuth credentials from secret manager
export APP_AUTH_PROVIDERS_GOOGLE_CLIENT_ID="$(aws secretsmanager get-secret-value --secret-id google-client-id --query SecretString --output text)"
export APP_AUTH_PROVIDERS_GOOGLE_CLIENT_SECRET="$(aws secretsmanager get-secret-value --secret-id google-client-secret --query SecretString --output text)"
export APP_AUTH_PROVIDERS_GOOGLE_REDIRECT_URI="https://yourdomain.com/auth/callback/google"

cargo run --release
```

### Security Best Practices

1. **Never commit secrets to version control**
   - ✅ Use `.env` for local development (add to `.gitignore`)
   - ✅ Use environment variables in staging/production
   - ❌ Never hardcode secrets in config files

2. **Use strong, randomly generated secrets**
   ```bash
   # JWT secret (base64, 48 bytes)
   openssl rand -base64 48
   
   # API key (hex, 32 bytes)
   openssl rand -hex 32
   ```

3. **Rotate secrets regularly**
   - Update environment variables
   - Restart the application
   - No code changes required

4. **Use different secrets per environment**
   - Development uses weak secrets (convenience)
   - Staging uses strong secrets (testing)
   - Production uses strongest secrets (security)

### Secrets Management Best Practices

1. **Always use environment variables in production**

   ```bash
   # Good - secrets from environment
   export SECRET_ANTHROPIC_API_KEY="sk-ant-..."

   # Bad - secrets in config files
   # [secrets.values]
   # anthropic_api_key = "sk-ant-..."  # ❌ Never do this!
   ```

2. **Use secret stores in production**

   ```bash
   # AWS Secrets Manager
   export SECRET_ANTHROPIC_API_KEY="$(aws secretsmanager get-secret-value --secret-id anthropic-key --query SecretString --output text)"

   # HashiCorp Vault
   export SECRET_ANTHROPIC_API_KEY="$(vault kv get -field=key secret/anthropic)"

   # Kubernetes secrets
   # Mount secrets as files or environment variables
   ```

3. **Rotate secrets regularly**
   - Update environment variables
   - Restart the application to load new values
   - No code changes required

4. **Test secret availability**

   ```javascript
   // In your JavaScript code
   if (!Secrets.exists("anthropic_api_key")) {
     throw new Error("Required secret anthropic_api_key not configured");
   }
   ```

5. **Monitor secret usage**
   - Check application logs for missing secret warnings
   - The editor's AI assistant will report if API keys are not configured

## Configuration Testing

Test your configuration with:

```bash
# Validate configuration without starting server
cargo run --bin server -- --validate-config

# Start with specific config file
RUST_LOG=info cargo run --bin server
```
