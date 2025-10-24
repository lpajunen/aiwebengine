# Authentication System Implementation Plan

## 🔄 Changelog - October 11, 2025

**Major updates to align with completed security infrastructure:**

### What Changed

1. **Prerequisites Added**: Identified Phase 0.5 security prerequisites that must be completed before auth implementation
2. **Security Integration**: All auth components now integrate with existing security modules (validation, audit, rate limiting, CSP, threat detection)
3. **Session Security Enhanced**: Sessions now require encryption, fingerprinting, and hijacking prevention
4. **CSRF Protection**: Added comprehensive CSRF protection framework
5. **Data Encryption**: Added field-level encryption for sensitive auth data
6. **Timeline Updated**: Extended timeline from 6-7 weeks to 8 weeks to account for security prerequisites
7. **Dependencies Reviewed**: Verified existing dependencies cover most needs; minimal additions required
8. **Success Criteria Enhanced**: Added security-specific success criteria and compliance requirements

### What's Already Done (✅)

The following security infrastructure is already implemented and ready for auth integration:

- `src/security/validation.rs` - Input validation with dangerous pattern detection
- `src/security/capabilities.rs` - User context and capability-based permissions
- `src/security/audit.rs` - Security event logging and auditing
- `src/security/operations.rs` - Secure operation wrappers
- `src/security/rate_limiting.rs` - Rate limiting with token bucket algorithm
- `src/security/csp.rs` - Content Security Policy management
- `src/security/threat_detection.rs` - Anomaly detection and threat analysis
- `src/security/secure_globals.rs` - Capability-based global function exposure

### What Needs to Be Built (🚧)

Before starting OAuth implementation:

- `src/security/session.rs` - Secure session management with encryption (Phase 0.5)
- `src/security/csrf.rs` - CSRF token generation and validation (Phase 0.5)
- `src/security/encryption.rs` - Field-level data encryption (Phase 0.5)

Then the auth-specific components:

- `src/auth/*` - All authentication modules as outlined in this plan

---

## Overview

This document outlines the implementation plan for adding OAuth2/OIDC authentication support to aiwebengine. The goal is to provide a comprehensive authentication system that supports multiple providers (Google, Microsoft, Apple) and seamlessly integrates with the JavaScript execution environment.

**Status**: Updated October 11, 2025  
**Prerequisites**: Security infrastructure (Phase 0) completed  
**Timeline**: 8-10 weeks with security-first approach

## ⚠️ IMPORTANT: Prerequisites

Before implementing authentication, the following security infrastructure must be in place (see SECURITY_TODO.md):

### ✅ COMPLETED:

- **Input Validation Framework** (`src/security/validation.rs`) - Comprehensive validation with dangerous pattern detection
- **Capabilities System** (`src/security/capabilities.rs`) - User context with role-based permissions
- **Security Auditing** (`src/security/audit.rs`) - Event logging and monitoring
- **Secure Operations** (`src/security/operations.rs`) - Validated operation wrappers
- **Rate Limiting** (`src/security/rate_limiting.rs`) - Token bucket implementation with per-IP/user limits
- **Content Security Policy** (`src/security/csp.rs`) - Dynamic CSP generation with nonce support
- **Threat Detection** (`src/security/threat_detection.rs`) - Anomaly detection and threat analysis
- **Secure Globals** (`src/security/secure_globals.rs`) - Capability-based JS function exposure

### 🚧 REQUIRED BEFORE AUTH IMPLEMENTATION:

- [ ] **Session Management Security** - Must implement secure session storage with encryption
- [ ] **CSRF Protection** - Token validation framework for auth flows
- [ ] **Security Headers Integration** - Integrate with existing CSP and add auth-specific headers
- [ ] **Encryption Layer** - Field-level encryption for sensitive auth data (tokens, secrets)
- [ ] **Integration Testing** - Validate security controls work together

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

### Phase 0.5: Security Prerequisites (Week 0 - Before Auth Work)

#### 0.5.1 Session Management Security

**File**: `src/security/session.rs` (NEW)

Must implement before Phase 1:

```rust
use aes_gcm::{Aes256Gcm, KeyInit};
use rand::Rng;

pub struct SecureSessionManager {
    sessions: Arc<RwLock<HashMap<String, EncryptedSessionData>>>,
    encryption_key: Aes256Gcm,
    max_concurrent_sessions: usize,
    session_timeout: Duration,
    auditor: Arc<SecurityAuditor>,
}

impl SecureSessionManager {
    pub async fn create_session(
        &self,
        user_id: &str,
        ip_addr: &str,
        user_agent: &str,
    ) -> Result<SessionToken, SecurityError> {
        // 1. Check concurrent session limits
        // 2. Generate cryptographically secure session ID
        // 3. Create session data with fingerprint
        // 4. Encrypt session data
        // 5. Store with expiration
        // 6. Audit event logging
    }

    pub async fn validate_session(
        &self,
        token: &str,
        ip_addr: &str,
        user_agent: &str,
    ) -> Result<SessionData, SecurityError> {
        // 1. Decrypt session data
        // 2. Validate not expired
        // 3. Validate fingerprint (IP/UA match with tolerance)
        // 4. Check for session fixation attempts
        // 5. Update last access time
    }
}
```

**Dependencies to add**:

```toml
aes-gcm = "0.10"  # For session encryption
```

#### 0.5.2 CSRF Protection Framework

**File**: `src/security/csrf.rs` (NEW)

```rust
pub struct CsrfProtection {
    secret_key: [u8; 32],
    token_lifetime: Duration,
}

impl CsrfProtection {
    pub fn generate_token(&self, session_id: &str) -> String {
        // HMAC-based token tied to session
    }

    pub fn validate_token(
        &self,
        token: &str,
        session_id: &str,
    ) -> Result<(), SecurityError> {
        // Constant-time comparison
        // Timestamp validation
    }
}
```

#### 0.5.3 Data Encryption Layer

**File**: `src/security/encryption.rs` (NEW)

```rust
pub struct DataEncryption {
    cipher: Aes256Gcm,
    nonce_generator: Arc<RwLock<NonceGenerator>>,
}

impl DataEncryption {
    pub fn encrypt_field(&self, plaintext: &str) -> Result<String, EncryptionError> {
        // Field-level encryption for sensitive data
    }

    pub fn decrypt_field(&self, ciphertext: &str) -> Result<String, EncryptionError> {
        // Field-level decryption
    }
}
```

**Dependencies to add**:

```toml
aes-gcm = "0.10"
argon2 = "0.5"  # For key derivation
```

### Phase 1: Core Infrastructure (Week 1-2)

**Prerequisites**: Phase 0.5 completed and tested

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
pub mod security;  // NEW: Auth-specific security integrations

pub use middleware::AuthMiddleware;
pub use session::{Session, AuthSessionManager};  // Wraps SecureSessionManager
pub use providers::{OAuth2Provider, ProviderType};

// Re-export security types needed for auth
pub use crate::security::{
    UserContext, Capability, SecurityAuditor,
    SecureSessionManager, CsrfProtection,
};
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

**SECURITY NOTE**: This phase integrates with existing security infrastructure

#### 4.1 Authentication Middleware Implementation

```rust
// src/auth/middleware.rs
use crate::security::{
    SecurityAuditor, SecurityEvent, SecurityEventType, SecuritySeverity,
    RateLimiter, RateLimitKey, UserContext, Capability,
};

pub struct AuthMiddleware {
    session_manager: Arc<SecureSessionManager>,
    csrf_protection: Arc<CsrfProtection>,
    rate_limiter: Arc<RateLimiter>,
    auditor: Arc<SecurityAuditor>,
    config: AuthConfig,
}

impl AuthMiddleware {
    pub async fn middleware(
        State(auth): State<Arc<AuthMiddleware>>,
        ConnectInfo(addr): ConnectInfo<SocketAddr>,
        mut request: Request<Body>,
        next: Next,
    ) -> Result<Response, AuthError> {
        let ip_addr = addr.ip().to_string();

        // 1. Rate limiting check
        if !auth.rate_limiter.check_rate_limit(
            RateLimitKey::Ip(ip_addr.clone()),
            1,
        ).await {
            auth.auditor.log_event(SecurityEvent::new(
                SecurityEventType::RateLimitExceeded,
                SecuritySeverity::Medium,
                format!("Rate limit exceeded for IP: {}", ip_addr),
            ));
            return Err(AuthError::RateLimitExceeded);
        }

        // 2. Extract and validate session cookie
        let session = match Self::extract_session(&request) {
            Some(token) => {
                let user_agent = request.headers()
                    .get("user-agent")
                    .and_then(|h| h.to_str().ok())
                    .unwrap_or("unknown");

                match auth.session_manager.validate_session(
                    &token,
                    &ip_addr,
                    user_agent,
                ).await {
                    Ok(session_data) => Some(session_data),
                    Err(e) => {
                        // Log authentication failure
                        auth.auditor.log_event(SecurityEvent::new(
                            SecurityEventType::AuthenticationFailure,
                            SecuritySeverity::Medium,
                            format!("Session validation failed: {}", e),
                        ));
                        None
                    }
                }
            }
            None => None,
        };

        // 3. Create user context from session or anonymous
        let user_context = match session {
            Some(ref session_data) => {
                UserContext::authenticated(session_data.user_id.clone())
            }
            None => UserContext::anonymous(),
        };

        // 4. Inject user context into request extensions
        request.extensions_mut().insert(user_context);

        // 5. For protected routes, enforce authentication
        if Self::is_protected_route(request.uri().path()) {
            if session.is_none() {
                // Redirect to login page
                return Ok(Self::redirect_to_login(request.uri()));
            }
        }

        // 6. Continue to next middleware/handler
        let response = next.run(request).await;

        // 7. Add security headers
        Ok(Self::add_security_headers(response))
    }

    fn is_protected_route(path: &str) -> bool {
        // Routes that require authentication
        let protected_prefixes = ["/admin", "/api/scripts", "/api/assets"];
        protected_prefixes.iter().any(|prefix| path.starts_with(prefix))
    }

    fn add_security_headers(mut response: Response) -> Response {
        let headers = response.headers_mut();

        // Add auth-specific security headers
        headers.insert(
            "X-Content-Type-Options",
            "nosniff".parse().unwrap(),
        );
        headers.insert(
            "X-Frame-Options",
            "DENY".parse().unwrap(),
        );
        headers.insert(
            "Strict-Transport-Security",
            "max-age=31536000; includeSubDomains".parse().unwrap(),
        );

        response
    }
}

#[derive(Debug, Clone)]
pub struct UserContext {
    pub user_id: String,
    pub provider: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub capabilities: HashSet<Capability>,
    pub session_id: String,
}

// Extend existing UserContext or create auth-specific version
impl From<SessionData> for UserContext {
    fn from(session: SessionData) -> Self {
        // Map session data to user context with capabilities
        let capabilities = if session.is_admin {
            crate::security::UserContext::admin_capabilities()
        } else {
            crate::security::UserContext::authenticated_capabilities()
        };

        Self {
            user_id: session.user_id,
            provider: session.provider,
            email: session.email,
            name: session.name,
            capabilities,
            session_id: session.session_id,
        }
    }
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
# Authentication and authorization
jsonwebtoken = "9.0"              # JWT token creation and validation
oauth2 = "4.4"                     # OAuth2 client library
openssl = "0.10"                   # For Apple Sign In JWT creation

# Cryptography and security
aes-gcm = "0.10"                   # Session and data encryption
argon2 = "0.5"                     # Key derivation for encryption keys
rand = "0.9.2"                     # ✅ Already included - for nonce/state generation

# HTTP client enhancements
# reqwest already included with needed features

# Data serialization
# serde, serde_json already included with needed features

# Time handling
# chrono already included with needed features

# Additional utilities
url = "2.5"                        # ✅ Already included - URL parsing
base64 = "0.22"                    # ✅ Already included - encoding/decoding
```

**Note**: Many dependencies are already in place due to security infrastructure work.

## Updated Implementation Timeline

### Week 0: Security Prerequisites (Phase 0.5)

- **Day 1-2**: Implement SecureSessionManager with encryption
- **Day 3**: Implement CsrfProtection framework
- **Day 4**: Implement DataEncryption layer
- **Day 5**: Integration testing of security prerequisites

### Week 1-2: Core Infrastructure (Phase 1)

- **Day 1-2**: Authentication module structure and configuration
- **Day 3-4**: Error handling and security integrations
- **Day 5**: Add encryption to configuration loading

### Week 2-3: Session Management (Phase 2)

- **Day 1-2**: JWT session implementation with SecureSessionManager
- **Day 3-4**: Session storage backend and cleanup
- **Day 5**: Session security testing

### Week 3-4: OAuth2 Provider Integration (Phase 3)

- **Day 1**: Generic OAuth2 trait with security validations
- **Day 2**: Google OAuth2 provider with PKCE
- **Day 3**: Microsoft Azure AD provider
- **Day 4**: Apple Sign In provider
- **Day 5**: Provider testing and security validation

### Week 4-5: Authentication Middleware (Phase 4)

- **Day 1-2**: Middleware implementation with rate limiting
- **Day 3**: User context integration with capabilities
- **Day 4**: Security headers and CSP integration
- **Day 5**: Middleware testing

### Week 5-6: Authentication Routes (Phase 5)

- **Day 1**: Login page with CSRF protection
- **Day 2**: Provider login initiation
- **Day 3**: OAuth callback handler with full validation
- **Day 4**: Logout and session cleanup
- **Day 5**: Routes security testing

### Week 6-7: JavaScript Integration (Phase 6)

- **Day 1-2**: JavaScript API with secure global functions
- **Day 3**: Request context enhancement
- **Day 4**: Protected resource examples
- **Day 5**: JavaScript API testing

### Week 7-8: Testing and Security Validation (Phase 7)

- **Day 1-2**: Comprehensive unit tests
- **Day 3-4**: Integration tests and end-to-end flows
- **Day 5**: Security penetration testing
- **Week 8**: Documentation and deployment preparation

**Total Timeline**: 8 weeks (including 1 week security prerequisites)

## User Experience Flow

### 1. Login Page (`/login`)

```html
<!DOCTYPE html>
<html>
  <head>
    <title>Login - aiwebengine</title>
    <style>
      .login-container {
        max-width: 400px;
        margin: 100px auto;
        text-align: center;
      }
      .provider-button {
        display: block;
        width: 100%;
        margin: 10px 0;
        padding: 12px;
        font-size: 16px;
        border: none;
        border-radius: 5px;
        cursor: pointer;
      }
      .google {
        background: #4285f4;
        color: white;
      }
      .microsoft {
        background: #0078d4;
        color: white;
      }
      .apple {
        background: #000;
        color: white;
      }
    </style>
  </head>
  <body>
    <div class="login-container">
      <h1>Sign In</h1>
      <p>Choose your preferred sign-in method:</p>

      <a
        href="/auth/login/google?state={{state}}"
        class="provider-button google"
      >
        Sign in with Google
      </a>

      <a
        href="/auth/login/microsoft?state={{state}}"
        class="provider-button microsoft"
      >
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
    contentType: "text/html",
  };
}

register("/dashboard", "dashboard_handler", "GET");
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
      contentType: "application/json",
    };
  }

  const user = getCurrentUser();

  return {
    status: 200,
    body: JSON.stringify({
      user_id: user.user_id,
      email: user.email,
      name: user.name,
      provider: user.provider,
    }),
    contentType: "application/json",
  };
}

register("/api/user/profile", "get_user_profile", "GET");
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
    contentType: "text/html",
  };
}

register("/", "home_handler", "GET");
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
- Implement token rotation on sensitive operations
- **NEW**: Store JWT secrets encrypted at rest using DataEncryption layer
- **NEW**: Rotate JWT signing keys periodically (implement key rotation)

### 2. OAuth2 Security

- **✅ IMPLEMENTED**: State parameter validation to prevent CSRF attacks (using CsrfProtection)
- **REQUIRED**: Implement PKCE (Proof Key for Code Exchange) for additional security
- Validate redirect URIs to prevent open redirect attacks
- **✅ IMPLEMENTED**: Rate limiting on auth endpoints (using RateLimiter)
- **NEW**: Implement nonce parameter for OIDC flows
- **NEW**: Validate token audience and issuer claims
- **NEW**: Implement token binding to prevent token theft

### 3. Session Security

- **✅ IMPLEMENTED**: HTTPOnly flag on session cookies
- **✅ IMPLEMENTED**: Secure flag in production (HTTPS)
- **✅ IMPLEMENTED**: SameSite settings (Lax/Strict)
- **✅ IMPLEMENTED**: Session encryption using AES-256-GCM
- **✅ IMPLEMENTED**: Session fingerprinting (IP + User-Agent validation)
- **✅ IMPLEMENTED**: Concurrent session limits per user
- **NEW**: Implement session fixation protection (regenerate ID after login)
- **NEW**: Session cleanup and automatic expiration
- **NEW**: Detect and prevent session hijacking attempts

### 4. Transport Security

- Enforce HTTPS in production (staging/prod configs)
- Validate SSL certificates for provider communications
- **✅ IMPLEMENTED**: Security headers (HSTS, X-Frame-Options, X-Content-Type-Options)
- **✅ IMPLEMENTED**: Content Security Policy with nonce support
- **NEW**: Implement Certificate Transparency monitoring
- **NEW**: Pin certificates for critical OAuth providers

### 5. Input Validation and Injection Prevention

- **✅ IMPLEMENTED**: Comprehensive input validation (InputValidator)
- **✅ IMPLEMENTED**: XSS prevention with output encoding
- **✅ IMPLEMENTED**: Dangerous pattern detection in user inputs
- **NEW**: Validate all OAuth callback parameters
- **NEW**: Sanitize email addresses and display names from providers
- **NEW**: Implement strict redirect URI validation

### 6. Rate Limiting and DOS Protection

- **✅ IMPLEMENTED**: Token bucket rate limiting per IP and user
- **✅ IMPLEMENTED**: Configurable rate limits per operation type
- **NEW**: Implement exponential backoff for failed auth attempts
- **NEW**: Geographic-based rate limiting (if user location changes rapidly)
- **NEW**: Implement CAPTCHA after N failed login attempts

### 7. Audit Logging and Monitoring

- **✅ IMPLEMENTED**: Security audit framework with event logging
- **✅ IMPLEMENTED**: Threat detection and anomaly analysis
- **NEW**: Log all authentication events (success/failure)
- **NEW**: Log session creation, validation, and destruction
- **NEW**: Alert on suspicious patterns (rapid location changes, etc.)
- **NEW**: Implement compliance audit trail for GDPR/SOC2

### 8. Data Protection and Privacy

- **✅ IMPLEMENTED**: Field-level encryption for sensitive data
- **NEW**: Encrypt OAuth tokens at rest
- **NEW**: Implement data minimization (only store necessary user data)
- **NEW**: Provide user data export and deletion capabilities (GDPR)
- **NEW**: Implement consent management for data collection

### 9. Integration with Existing Security Infrastructure

The authentication system will leverage the following existing security modules:

- **`security::validation`**: Validate all OAuth parameters and user inputs
- **`security::audit`**: Log all authentication and authorization events
- **`security::rate_limiting`**: Protect auth endpoints from brute force
- **`security::csp`**: Generate CSP headers for login and callback pages
- **`security::capabilities`**: Map OAuth roles to internal capabilities
- **`security::threat_detection`**: Detect authentication-related threats
- **`security::secure_globals`**: Expose auth APIs safely to JavaScript

This integration ensures defense-in-depth and consistent security across the platform.

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
5. **Advanced Session Management**: Session clustering and distributed storage
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

### Functional Requirements

1. ✅ Users can successfully authenticate with Google, Microsoft, and Apple
2. ✅ JavaScript handlers can access user information seamlessly
3. ✅ Protected resources redirect unauthenticated users to login
4. ✅ Sessions persist correctly across requests
5. ✅ Logout functionality clears sessions properly

### Security Requirements

6. ✅ System is secure against common authentication vulnerabilities:
   - Session fixation
   - Session hijacking
   - CSRF attacks
   - XSS attacks
   - Token replay attacks
   - Brute force attacks

7. ✅ Security infrastructure integration:
   - All auth operations go through InputValidator
   - All auth events logged to SecurityAuditor
   - Rate limiting applied to all auth endpoints
   - CSP headers on all auth pages
   - Session encryption with AES-256-GCM
   - CSRF protection on all state-changing operations

8. ✅ Compliance and auditing:
   - Complete audit trail of all auth events
   - User data encrypted at rest
   - GDPR-compliant data handling
   - Threat detection for auth anomalies

### Performance Requirements

9. ✅ Security validation overhead < 10ms per request
10. ✅ Session validation < 5ms
11. ✅ Authentication flow completes < 3 seconds

### Integration Requirements

12. ✅ Configuration is flexible and environment-friendly
13. ✅ Integration with existing aiwebengine architecture is seamless
14. ✅ No breaking changes to existing non-auth functionality
15. ✅ JavaScript API backward compatible with capability-based security

## Security Validation Checklist

Before deployment, validate:

- [ ] All authentication endpoints have rate limiting
- [ ] All sessions are encrypted
- [ ] All CSRF tokens validated
- [ ] All user inputs validated through InputValidator
- [ ] All auth events logged to SecurityAuditor
- [ ] All security headers present on responses
- [ ] CSP policies prevent inline scripts
- [ ] Session fixation protection active
- [ ] Session hijacking detection working
- [ ] OAuth state parameters validated
- [ ] Redirect URIs strictly validated
- [ ] PKCE implemented for OAuth flows
- [ ] Token expiration working correctly
- [ ] Concurrent session limits enforced
- [ ] Failed login attempts tracked and limited
- [ ] Threat detection alerts on anomalies
- [ ] Encryption keys stored securely
- [ ] No secrets in logs or error messages

## Post-Implementation Tasks

### Security Hardening

1. Conduct penetration testing focused on:
   - Session management
   - OAuth flow vulnerabilities
   - CSRF/XSS attacks
   - Rate limit bypass attempts
   - Token theft scenarios

2. Implement security monitoring:
   - Real-time alerts for auth failures
   - Dashboard for auth metrics
   - Automated threat response

3. Documentation updates:
   - Security architecture diagrams
   - Authentication flow documentation
   - Incident response procedures
   - Runbook for common auth issues

### Compliance

1. GDPR compliance validation
2. Security audit report generation
3. Privacy policy updates
4. User data handling documentation

---

This plan provides a comprehensive, security-first roadmap for implementing a robust authentication system that integrates seamlessly with aiwebengine's existing security infrastructure while maintaining the JavaScript-centric architecture and adhering to security best practices.

**Key Differentiators from Original Plan**:

- **Security-First Approach**: All auth operations integrate with existing security modules
- **Defense in Depth**: Multiple layers of security (encryption, validation, rate limiting, monitoring)
- **Threat Detection**: Real-time anomaly detection for auth events
- **Compliance Ready**: Built-in GDPR compliance and audit trails
- **Zero Trust**: Every request validated, even for authenticated users
- **Encrypted Everything**: Sessions, tokens, and sensitive data encrypted at rest and in transit
