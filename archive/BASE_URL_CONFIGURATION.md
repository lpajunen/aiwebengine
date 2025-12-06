# Base URL Configuration Fix

## Problem

The OAuth 2.1 authorization server metadata endpoint (`/.well-known/oauth-authorization-server`) was returning a hardcoded issuer URL of `http://localhost:8080`, which broke OAuth discovery for clients connecting to production domains like `softagen.com`.

## Solution

Added a configurable `base_url` field to `ServerConfig` that:

1. Can be explicitly set in configuration files for production/staging environments
2. Auto-constructs from host and port when not specified (for local development)
3. Is used consistently for OAuth metadata issuer and auth manager base URL

## Changes Made

### 1. Configuration Structure (`src/config.rs`)

- Added `base_url: Option<String>` field to `ServerConfig`
- Added `get_base_url()` helper method that:
  - Returns configured `base_url` if present
  - Auto-constructs from host/port with smart defaults:
    - Uses "localhost" when host is `0.0.0.0` or `::`
    - Selects https/http based on port (443 → https)
    - Omits standard ports (80, 443) from URL
- Updated `Default` implementation to include `base_url: None`

### 2. Authentication Initialization (`src/lib.rs`)

- Updated `initialize_auth_manager()` to accept `&ServerConfig` parameter
- Changed from hardcoded `"http://localhost:8080"` to `server_config.get_base_url()`
- Updated call site to pass `&config.server`

### 3. OAuth Metadata Configuration (`src/lib.rs`)

- Updated `setup_routes()` to use `config` instead of `_config`
- Changed `MetadataConfig::issuer` from hardcoded to `config.server.get_base_url()`

### 4. Configuration Files

Updated all environment configs with `base_url` field:

**Production (`config.production.toml`):**

```toml
[server]
base_url = "https://softagen.com"
```

**Staging (`config.staging.toml`):**

```toml
[server]
base_url = "https://staging.softagen.com"
```

**Local Development (`config.local.toml`, `config.toml`):**

```toml
[server]
# Optional: Base URL for OAuth metadata and redirects
# If not set, will auto-construct from host and port
# base_url = "http://localhost:3000"
```

## Usage

### Production Deployment

Set the base_url in your config file or via environment variable:

```bash
export APP_SERVER__BASE_URL="https://yourdomain.com"
```

### Local Development

No configuration needed - will auto-construct to `http://localhost:3000` (or your configured port).

### Docker/Container Deployments

When running behind a reverse proxy (like Caddy), set the public-facing URL:

```toml
[server]
host = "0.0.0.0"
port = 3000
base_url = "https://softagen.com"  # Public URL, not internal container address
```

## Verification

✅ Code compiles without errors
✅ All clippy warnings resolved
✅ OAuth metadata endpoint will now return correct issuer URL
✅ Authentication manager uses correct base URL for redirects

## Impact

- **OAuth Discovery**: Clients can now discover authorization endpoints using the correct issuer
- **OIDC Compliance**: Issuer URL matches the actual server domain
- **Environment Flexibility**: Same code works for local, staging, and production with proper config
- **Backward Compatible**: Auto-construction ensures existing setups continue to work

## Related RFCs

- RFC 8414 (OAuth 2.0 Authorization Server Metadata)
- RFC 7591 (OAuth 2.0 Dynamic Client Registration Protocol)
