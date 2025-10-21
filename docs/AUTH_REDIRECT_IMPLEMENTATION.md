# Authentication Redirect Implementation

## Overview

This document describes the implementation of automatic redirect to login for unauthenticated users accessing protected endpoints (`/editor` and `/graphql`).

## Implementation Details

### Protected Endpoints

Authentication requirements are implemented in two ways:

1. **GraphQL Endpoints** (enforced at router level):
   - `/graphql` - GraphQL API endpoint (GET for GraphiQL, POST for queries)
   - `/graphql/sse` - GraphQL Server-Sent Events endpoint

2. **JavaScript-based Protection** (enforced in script handlers using `auth.requireAuth()`):
   - `/editor` - Web-based editor interface
   - Any custom endpoint that calls `auth.requireAuth()` in its handler

### Redirect Flow

1. **Unauthenticated Access**: When an unauthenticated user tries to access a protected endpoint, they are automatically redirected to `/auth/login?redirect={original_url}`

2. **Login Page**: The login page displays available OAuth providers and preserves the redirect URL

3. **Provider Selection**: When a user selects a provider, the redirect URL is stored in the authentication security context keyed by the OAuth state token

4. **OAuth Flow**: The user is redirected to the OAuth provider for authentication

5. **Callback**: After successful authentication, the system:
   - Retrieves the stored redirect URL using the state token
   - Creates a session with a secure cookie
   - Redirects the user back to their original destination

### Key Components

#### 1. Redirect to Login Middleware (`src/auth/middleware.rs`)

```rust
pub async fn redirect_to_login_middleware(
    State(auth_manager): State<Arc<AuthManager>>,
    mut req: Request,
    next: Next,
) -> Response
```

This middleware:
- Checks if the user has a valid session
- If authenticated, passes the request through
- If not authenticated, redirects to `/auth/login?redirect={url}`

#### 2. OAuth State with Redirect Storage (`src/auth/security.rs`)

```rust
pub async fn create_oauth_state_with_redirect(
    &self,
    provider: &str,
    ip_addr: &str,
    redirect_url: String,
) -> Result<String, AuthError>

pub async fn take_redirect_url(&self, state: &str) -> Option<String>
```

The security context now maintains a map of OAuth state tokens to redirect URLs, ensuring secure storage and one-time use.

#### 3. Enhanced Auth Manager (`src/auth/manager.rs`)

```rust
pub async fn start_login_with_redirect(
    &self,
    provider_name: &str,
    ip_addr: &str,
    redirect_url: String,
) -> Result<(String, String), AuthError>

pub async fn get_redirect_url(&self, state: &str) -> Option<String>
```

New methods to support redirect URL storage and retrieval during the OAuth flow.

#### 4. Updated Routes (`src/auth/routes.rs`)

- **Login Page**: Now accepts and forwards the `redirect` query parameter
- **Start Login**: Stores the redirect URL when initiating OAuth flow
- **OAuth Callback**: Retrieves and uses the redirect URL after successful authentication

#### 5. Main Router Configuration (`src/lib.rs`)

- `/graphql` endpoints are wrapped with `redirect_to_login_middleware` when auth is enabled
- JavaScript endpoints like `/editor` handle authentication in their request handlers using `auth.requireAuth()`
- When `auth.requireAuth()` is called and user is not authenticated, the handler returns a 302 redirect to login

### Security Considerations

1. **URL Encoding**: All redirect URLs are properly URL-encoded to prevent injection attacks

2. **State Token Binding**: Redirect URLs are bound to OAuth state tokens, ensuring they can only be used once during the specific OAuth flow

3. **Automatic Cleanup**: Redirect URLs are removed from storage when consumed (via `take_redirect_url`)

4. **Session Validation**: Standard session validation with IP and user agent checking is still applied

### Configuration

Authentication must be enabled in the configuration file:

```toml
[auth]
enabled = true

[auth.cookie]
name = "auth_session"
secure = true
http_only = true
same_site = "Lax"

[auth.providers.google]
client_id = "your-client-id"
client_secret = "your-client-secret"
redirect_uri = "http://localhost:8080/auth/callback/google"
```

### Usage Examples

#### Accessing Protected Endpoint Without Authentication

```bash
# User visits /editor without authentication
curl -L http://localhost:8080/editor

# Gets redirected to /auth/login?redirect=%2Feditor
# After login, user is automatically redirected back to /editor
```

#### GraphQL Query Without Authentication

```bash
# User tries to access GraphQL
curl http://localhost:8080/graphql

# Gets 302 redirect to /auth/login?redirect=%2Fgraphql
```

### Testing

To test the redirect functionality:

1. Start the server with authentication enabled
2. Visit `/editor` or `/graphql` without being logged in
3. Verify you're redirected to `/auth/login`
4. Complete the OAuth flow with a provider
5. Verify you're redirected back to the original page

### Future Enhancements

Possible improvements:
- Support for additional protected endpoints via configuration
- Custom redirect logic per endpoint
- Redirect URL validation (whitelist)
- Remember last visited page across sessions
- Support for query parameters in redirect URLs

## Code Changes Summary

### New Files
- None

### Modified Files
- `src/auth/middleware.rs` - Added `redirect_to_login_middleware`
- `src/auth/security.rs` - Added redirect URL storage and methods
- `src/auth/manager.rs` - Added `start_login_with_redirect` and `get_redirect_url`
- `src/auth/routes.rs` - Updated login page and OAuth flow to handle redirects
- `src/auth/mod.rs` - Exported new middleware function
- `src/lib.rs` - Applied redirect middleware to protected endpoints
- `Cargo.toml` - Added `urlencoding` dependency

### Dependencies Added
- `urlencoding = "2.1.3"` - For safe URL encoding of redirect parameters
