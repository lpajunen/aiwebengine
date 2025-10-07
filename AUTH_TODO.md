# Authentication System Implementation Plan

## Overview

This document outlines the implementation plan for adding OAuth2/OIDC authentication support to aiwebengine. The goal is to provide a comprehensive authentication system that supports multiple providers (Google, Microsoft, Apple) and seamlessly integrates with the JavaScript execution environment.

## Architecture Overview

```
┌─────────────────┐    ┌──────────────────┐    ┌─────────────────┐
│   Client/User   │    │   aiwebengine    │    │  OAuth Provider │
│                 │    │                  │    │ (Google/MS/Apple)│
└─────────────────┘    └──────────────────┘    └─────────────────┘
         │                       │                       │
         │ 1. Access protected   │                       │
         │    resource           │                       │
         ├──────────────────────►│                       │
         │                       │ 2. Redirect to        │
         │                       │    /login page        │
         │◄──────────────────────┤                       │
         │                       │                       │
         │ 3. Choose provider    │                       │
         │    & click login      │                       │
         ├──────────────────────►│                       │
         │                       │ 4. Redirect to        │
         │                       │    provider OAuth     │
         │◄──────────────────────┼──────────────────────►│
         │                       │                       │
         │ 5. User authenticates │                       │
         │    with provider      │                       │
         │◄─────────────────────────────────────────────►│
         │                       │                       │
         │ 6. Provider callback  │                       │
         │    with auth code     │                       │
         ├─────────────────────────────────────────────►│
         │                       │ 7. Exchange code      │
         │                       │    for tokens         │
         │                       │◄─────────────────────►│
         │                       │                       │
         │ 8. Set session cookie │                       │
         │    & redirect back    │                       │
         │◄──────────────────────┤                       │
         │                       │                       │
         │ 9. Access original    │                       │
         │    resource with      │                       │
         │    valid session      │                       │
         ├──────────────────────►│                       │
         │                       │                       │
```

## Core Components

### 1. Authentication Middleware
**File**: `src/auth/middleware.rs`

- **Session Cookie Validation**: Verify JWT tokens in HTTP cookies
- **User Context Injection**: Add user information to request context
- **Protected Route Handling**: Redirect unauthenticated users to login
- **Request ID Integration**: Leverage existing middleware system

### 2. OAuth2 Provider Integration
**Directory**: `src/auth/providers/`

- **Generic OAuth2 Trait**: Common interface for all providers
- **Google Provider**: `google.rs` - Google OAuth2/OIDC implementation
- **Microsoft Provider**: `microsoft.rs` - Microsoft Azure AD implementation
- **Apple Provider**: `apple.rs` - Apple Sign In implementation
- **Provider Discovery**: Automatic OIDC endpoint discovery

### 3. Session Management
**File**: `src/auth/session.rs`

- **JWT Token Generation**: Create signed JWT tokens with user claims
- **Session Storage**: In-memory session store with configurable backends
- **Token Validation**: Verify JWT signatures and expiration
- **Session Cleanup**: Automatic cleanup of expired sessions

### 4. Authentication Routes
**File**: `src/auth/routes.rs`

- **Login Page Handler**: `/login` - Provider selection interface
- **Provider Login**: `/auth/login/{provider}` - OAuth initiation
- **Callback Handler**: `/auth/callback/{provider}` - OAuth callback processing
- **Logout Handler**: `/logout` - Session termination

### 5. JavaScript Integration
**File**: `src/auth/js_api.rs`

- **User Context API**: JavaScript functions to access user information
- **Login Enforcement**: `expectLogin()` function for protected resources
- **User ID Access**: Direct access to `user_id` in handler context

## Implementation Phases

### Phase 1: Core Infrastructure (Week 1-2)

#### 1.1 Authentication Module Structure
```rust
// src/auth/mod.rs
pub mod middleware;
pub mod providers;
pub mod session;
pub mod routes;
pub mod js_api;
pub mod config;
pub mod error;

pub use middleware::AuthMiddleware;
pub use session::{Session, SessionManager};
pub use providers::{OAuth2Provider, ProviderType};
```

#### 1.2 Configuration System
```rust
// src/auth/config.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub jwt_secret: String,
    pub session_timeout: Duration,
    pub cookie_domain: Option<String>,
    pub cookie_secure: bool,
    pub providers: ProvidersConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvidersConfig {
    pub google: Option<ProviderConfig>,
    pub microsoft: Option<ProviderConfig>,
    pub apple: Option<ProviderConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    pub client_id: String,
    pub client_secret: String,
    pub redirect_uri: String,
    pub scopes: Vec<String>,
}
```

#### 1.3 Error Handling
```rust
// src/auth/error.rs
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Invalid JWT token: {0}")]
    InvalidToken(String),
    
    #[error("OAuth2 provider error: {0}")]
    ProviderError(String),
    
    #[error("Session not found")]
    SessionNotFound,
    
    #[error("Configuration error: {0}")]
    ConfigError(String),
}
```

### Phase 2: Session Management (Week 2-3)

#### 2.1 JWT Session Implementation
```rust
// src/auth/session.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionClaims {
    pub user_id: String,
    pub provider: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub exp: i64,
    pub iat: i64,
}

impl SessionManager {
    pub fn create_session(&self, user_info: UserInfo) -> Result<String, AuthError>;
    pub fn validate_session(&self, token: &str) -> Result<SessionClaims, AuthError>;
    pub fn invalidate_session(&self, token: &str) -> Result<(), AuthError>;
    pub fn cleanup_expired_sessions(&self);
}
```

#### 2.2 Session Storage Backend
```rust
// src/auth/session.rs
pub trait SessionStore: Send + Sync {
    async fn store_session(&self, token: &str, claims: &SessionClaims) -> Result<(), AuthError>;
    async fn get_session(&self, token: &str) -> Result<Option<SessionClaims>, AuthError>;
    async fn remove_session(&self, token: &str) -> Result<(), AuthError>;
    async fn cleanup_expired(&self) -> Result<(), AuthError>;
}

// In-memory implementation
pub struct InMemorySessionStore {
    sessions: Arc<RwLock<HashMap<String, SessionClaims>>>,
}
```

### Phase 3: OAuth2 Provider Integration (Week 3-4)

#### 3.1 Generic OAuth2 Provider Trait
```rust
// src/auth/providers/mod.rs
#[async_trait]
pub trait OAuth2Provider: Send + Sync {
    fn provider_name(&self) -> &'static str;
    fn authorization_url(&self, state: &str, redirect_uri: &str) -> Result<String, AuthError>;
    async fn exchange_code(&self, code: &str, redirect_uri: &str) -> Result<TokenResponse, AuthError>;
    async fn get_user_info(&self, access_token: &str) -> Result<UserInfo, AuthError>;
}

#[derive(Debug, Clone)]
pub struct UserInfo {
    pub id: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub picture: Option<String>,
}
```

#### 3.2 Google OAuth2 Provider
```rust
// src/auth/providers/google.rs
pub struct GoogleProvider {
    client_id: String,
    client_secret: String,
    discovery_document: Option<DiscoveryDocument>,
}

impl GoogleProvider {
    const DISCOVERY_URL: &'static str = "https://accounts.google.com/.well-known/openid_configuration";
    const DEFAULT_SCOPES: &'static [&'static str] = &["openid", "email", "profile"];
}
```

#### 3.3 Microsoft Azure AD Provider
```rust
// src/auth/providers/microsoft.rs
pub struct MicrosoftProvider {
    client_id: String,
    client_secret: String,
    tenant_id: String,
}

impl MicrosoftProvider {
    fn authorization_endpoint(&self) -> String {
        format!("https://login.microsoftonline.com/{}/oauth2/v2.0/authorize", self.tenant_id)
    }
}
```

#### 3.4 Apple Sign In Provider
```rust
// src/auth/providers/apple.rs
pub struct AppleProvider {
    client_id: String,
    team_id: String,
    key_id: String,
    private_key: String,
}

impl AppleProvider {
    const AUTHORIZATION_URL: &'static str = "https://appleid.apple.com/auth/authorize";
    const TOKEN_URL: &'static str = "https://appleid.apple.com/auth/token";
    
    fn create_client_secret(&self) -> Result<String, AuthError>;
}
```

### Phase 4: Authentication Middleware (Week 4-5)

#### 4.1 Authentication Middleware Implementation
```rust
// src/auth/middleware.rs
pub struct AuthMiddleware {
    session_manager: Arc<SessionManager>,
    config: AuthConfig,
}

impl AuthMiddleware {
    pub async fn middleware(
        State(auth): State<Arc<AuthMiddleware>>,
        mut request: Request<Body>,
        next: Next,
    ) -> Result<Response, AuthError> {
        // Extract session cookie
        // Validate session
        // Add user context to request extensions
        // Continue to next middleware/handler
    }
}

#[derive(Debug, Clone)]
pub struct UserContext {
    pub user_id: String,
    pub provider: String,
    pub email: Option<String>,
    pub name: Option<String>,
}
```

### Phase 5: Authentication Routes (Week 5-6)

#### 5.1 Login Page Handler
```rust
// src/auth/routes.rs
pub async fn login_page_handler(
    Query(params): Query<HashMap<String, String>>,
    State(config): State<Arc<AuthConfig>>,
) -> impl IntoResponse {
    let return_url = params.get("return_url").cloned();
    
    // Generate HTML page with provider buttons
    // Include return_url in state parameter
}
```

#### 5.2 Provider Login Initiation
```rust
pub async fn provider_login_handler(
    Path(provider_name): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    State(providers): State<Arc<HashMap<String, Box<dyn OAuth2Provider>>>>,
) -> impl IntoResponse {
    // Generate state parameter with return_url
    // Redirect to provider authorization URL
}
```

#### 5.3 OAuth Callback Handler
```rust
pub async fn oauth_callback_handler(
    Path(provider_name): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    State(auth): State<Arc<AuthMiddleware>>,
) -> impl IntoResponse {
    // Validate state parameter
    // Exchange authorization code for tokens
    // Get user information from provider
    // Create session
    // Set session cookie
    // Redirect to return_url or default page
}
```

### Phase 6: JavaScript Integration (Week 6-7)

#### 6.1 JavaScript API Extensions
```rust
// src/auth/js_api.rs
pub fn register_auth_functions(runtime: &rquickjs::Runtime) -> Result<(), AuthError> {
    runtime.register_function("getCurrentUser", js_get_current_user)?;
    runtime.register_function("expectLogin", js_expect_login)?;
    runtime.register_function("isAuthenticated", js_is_authenticated)?;
    Ok(())
}

fn js_get_current_user(ctx: &rquickjs::Context) -> Result<JsValue, AuthError> {
    // Extract user context from request
    // Return user information as JavaScript object
}

fn js_expect_login(ctx: &rquickjs::Context, return_url: Option<String>) -> Result<JsValue, AuthError> {
    // Check if user is authenticated
    // If not, return redirect response to /login
    // If authenticated, return user information
}
```

#### 6.2 Request Context Enhancement
```rust
// In js_engine.rs - enhance request object
let request_obj = js_ctx.object_value()?;

// Add user context if available
if let Some(user_ctx) = req.extensions().get::<UserContext>() {
    let user_obj = js_ctx.object_value()?;
    user_obj.set("user_id", user_ctx.user_id.clone())?;
    user_obj.set("email", user_ctx.email.clone())?;
    user_obj.set("name", user_ctx.name.clone())?;
    user_obj.set("provider", user_ctx.provider.clone())?;
    request_obj.set("user", user_obj)?;
}
```

## Environment Configuration

### Required Environment Variables

```bash
# JWT Configuration
AUTH_JWT_SECRET=your-super-secret-jwt-key-min-32-chars
AUTH_SESSION_TIMEOUT=3600  # 1 hour in seconds
AUTH_COOKIE_DOMAIN=localhost  # Optional
AUTH_COOKIE_SECURE=false  # Set to true in production

# Google OAuth2
GOOGLE_CLIENT_ID=your-google-client-id
GOOGLE_CLIENT_SECRET=your-google-client-secret
GOOGLE_REDIRECT_URI=http://localhost:3000/auth/callback/google

# Microsoft Azure AD
MICROSOFT_CLIENT_ID=your-microsoft-client-id
MICROSOFT_CLIENT_SECRET=your-microsoft-client-secret
MICROSOFT_TENANT_ID=your-tenant-id-or-common
MICROSOFT_REDIRECT_URI=http://localhost:3000/auth/callback/microsoft

# Apple Sign In
APPLE_CLIENT_ID=your-apple-client-id
APPLE_TEAM_ID=your-apple-team-id
APPLE_KEY_ID=your-apple-key-id
APPLE_PRIVATE_KEY=your-apple-private-key-content
APPLE_REDIRECT_URI=http://localhost:3000/auth/callback/apple
```

### Configuration File Integration

Add to `config.example.yaml`:
```yaml
auth:
  jwt_secret: "${AUTH_JWT_SECRET}"
  session_timeout: ${AUTH_SESSION_TIMEOUT:-3600}
  cookie:
    domain: "${AUTH_COOKIE_DOMAIN}"
    secure: ${AUTH_COOKIE_SECURE:-false}
    http_only: true
    same_site: "Lax"
  providers:
    google:
      client_id: "${GOOGLE_CLIENT_ID}"
      client_secret: "${GOOGLE_CLIENT_SECRET}"
      redirect_uri: "${GOOGLE_REDIRECT_URI}"
      scopes: ["openid", "email", "profile"]
    microsoft:
      client_id: "${MICROSOFT_CLIENT_ID}"
      client_secret: "${MICROSOFT_CLIENT_SECRET}"
      tenant_id: "${MICROSOFT_TENANT_ID:-common}"
      redirect_uri: "${MICROSOFT_REDIRECT_URI}"
      scopes: ["openid", "email", "profile"]
    apple:
      client_id: "${APPLE_CLIENT_ID}"
      team_id: "${APPLE_TEAM_ID}"
      key_id: "${APPLE_KEY_ID}"
      private_key: "${APPLE_PRIVATE_KEY}"
      redirect_uri: "${APPLE_REDIRECT_URI}"
      scopes: ["name", "email"]
```

## Dependencies to Add

Add to `Cargo.toml`:
```toml
# Authentication dependencies
jsonwebtoken = "9.0"
oauth2 = "4.4"
url = "2.5"
openssl = "0.10"  # For Apple Sign In JWT creation
rand = "0.8"  # For state generation

# HTTP client enhancements
reqwest = { version = "0.12.23", features = ["json", "blocking", "rustls-tls", "cookies"] }

# Additional serde features
serde = { version = "1.0", features = ["derive", "rc"] }

# Time handling
chrono = { version = "0.4", features = ["serde", "clock"] }
```

## User Experience Flow

### 1. Login Page (`/login`)
```html
<!DOCTYPE html>
<html>
<head>
    <title>Login - aiwebengine</title>
    <style>
        .login-container { max-width: 400px; margin: 100px auto; text-align: center; }
        .provider-button { display: block; width: 100%; margin: 10px 0; padding: 12px; font-size: 16px; border: none; border-radius: 5px; cursor: pointer; }
        .google { background: #4285f4; color: white; }
        .microsoft { background: #0078d4; color: white; }
        .apple { background: #000; color: white; }
    </style>
</head>
<body>
    <div class="login-container">
        <h1>Sign In</h1>
        <p>Choose your preferred sign-in method:</p>
        
        <a href="/auth/login/google?state={{state}}" class="provider-button google">
            Sign in with Google
        </a>
        
        <a href="/auth/login/microsoft?state={{state}}" class="provider-button microsoft">
            Sign in with Microsoft
        </a>
        
        <a href="/auth/login/apple?state={{state}}" class="provider-button apple">
            Sign in with Apple
        </a>
    </div>
</body>
</html>
```

### 2. JavaScript Handler Examples

#### Protected Resource Example
```javascript
// scripts/user_dashboard.js

function dashboard_handler(req) {
    // Use expectLogin to ensure user is authenticated
    const user = expectLogin(req.path);
    
    if (user.isRedirect) {
        // User not authenticated, return redirect response
        return user.response;
    }
    
    // User is authenticated, proceed with handler
    const userData = user.data;
    
    return {
        status: 200,
        body: `
            <h1>Welcome, ${userData.name || userData.email}!</h1>
            <p>User ID: ${userData.user_id}</p>
            <p>Provider: ${userData.provider}</p>
            <p>Email: ${userData.email}</p>
            <a href="/logout">Logout</a>
        `,
        contentType: "text/html"
    };
}

register('/dashboard', 'dashboard_handler', 'GET');
```

#### API Endpoint Example
```javascript
// scripts/user_api.js

function get_user_profile(req) {
    // Check if user is authenticated
    if (!isAuthenticated()) {
        return {
            status: 401,
            body: JSON.stringify({ error: "Authentication required" }),
            contentType: "application/json"
        };
    }
    
    const user = getCurrentUser();
    
    return {
        status: 200,
        body: JSON.stringify({
            user_id: user.user_id,
            email: user.email,
            name: user.name,
            provider: user.provider
        }),
        contentType: "application/json"
    };
}

register('/api/user/profile', 'get_user_profile', 'GET');
```

#### Personalized Content Example
```javascript
// scripts/personalized_content.js

function home_handler(req) {
    let content = "<h1>Welcome to aiwebengine!</h1>";
    
    if (isAuthenticated()) {
        const user = getCurrentUser();
        content += `<p>Hello, ${user.name || user.email}!</p>`;
        content += `<a href="/dashboard">Go to Dashboard</a> | <a href="/logout">Logout</a>`;
    } else {
        content += `<p>Please <a href="/login?return_url=${encodeURIComponent(req.path)}">sign in</a> to access personalized features.</p>`;
    }
    
    return {
        status: 200,
        body: content,
        contentType: "text/html"
    };
}

register('/', 'home_handler', 'GET');
```

## Testing Strategy

### 1. Unit Tests
- **Session Management**: JWT creation, validation, expiration
- **OAuth2 Providers**: Token exchange, user info retrieval
- **Middleware**: Authentication flow, user context injection

### 2. Integration Tests
- **End-to-End Auth Flow**: Login → callback → protected resource
- **JavaScript API**: `expectLogin`, `getCurrentUser`, `isAuthenticated`
- **Error Handling**: Invalid tokens, expired sessions, provider errors

### 3. Security Tests
- **JWT Security**: Signature validation, expiration handling
- **CSRF Protection**: State parameter validation
- **Session Security**: Secure cookie settings, session fixation

## Security Considerations

### 1. JWT Security
- Use strong, randomly generated secrets (minimum 32 characters)
- Set appropriate expiration times (1-24 hours)
- Include audience and issuer claims
- Use secure signing algorithms (HS256 or RS256)

### 2. OAuth2 Security
- Validate state parameters to prevent CSRF attacks
- Use PKCE for additional security (future enhancement)
- Validate redirect URIs to prevent open redirect attacks
- Implement rate limiting on auth endpoints

### 3. Session Security
- Set HTTPOnly flag on session cookies
- Use Secure flag in production (HTTPS)
- Implement proper SameSite settings
- Regular session cleanup and rotation

### 4. Transport Security
- Enforce HTTPS in production
- Validate SSL certificates for provider communications
- Use secure headers (HSTS, CSP, etc.)

## Monitoring and Logging

### Authentication Events to Log
- Successful logins with provider and user ID
- Failed authentication attempts
- Session creation and destruction
- Provider API errors
- Configuration errors

### Metrics to Track
- Authentication success/failure rates by provider
- Session duration statistics
- Provider response times
- Error rates and types

## Future Enhancements

### Phase 7: Advanced Features (Future)
1. **Multi-factor Authentication**: TOTP, SMS, email verification
2. **Social Login Extensions**: GitHub, Discord, Twitter/X
3. **SAML Support**: Enterprise SSO integration
4. **User Management**: Registration, profile management, password reset
5. **Advanced Session Management**: Redis backend, session clustering
6. **API Key Authentication**: Service-to-service authentication
7. **Rate Limiting**: Advanced rate limiting with authentication context
8. **Audit Logging**: Comprehensive security event logging

### Phase 8: Enterprise Features (Future)
1. **LDAP Integration**: Active Directory authentication
2. **Role-Based Access Control**: Fine-grained permissions
3. **Organization Management**: Multi-tenant support
4. **Compliance Features**: GDPR, SOC2, audit trails
5. **Advanced Security**: Device fingerprinting, geo-location validation

## Implementation Timeline

- **Week 1-2**: Core infrastructure, configuration, error handling
- **Week 3-4**: Session management and OAuth2 provider integration
- **Week 4-5**: Authentication middleware and request context
- **Week 5-6**: Authentication routes and HTML interfaces
- **Week 6-7**: JavaScript integration and API functions
- **Week 8**: Testing, documentation, and deployment preparation

## Success Criteria

1. Users can successfully authenticate with Google, Microsoft, and Apple
2. JavaScript handlers can access user information seamlessly
3. Protected resources redirect unauthenticated users to login
4. Sessions persist correctly across requests
5. Logout functionality clears sessions properly
6. System is secure against common authentication vulnerabilities
7. Configuration is flexible and environment-friendly
8. Integration with existing aiwebengine architecture is seamless

This plan provides a comprehensive roadmap for implementing a robust authentication system that integrates naturally with aiwebengine's JavaScript-centric architecture while maintaining security best practices and user experience standards.