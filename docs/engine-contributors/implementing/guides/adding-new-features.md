# Adding New Features to aiwebengine

**Last Updated:** October 24, 2025

This guide provides a step-by-step process for adding new functional features to the aiwebengine core codebase.

---

## Overview

Adding a feature to aiwebengine involves more than just writing code. This guide ensures your feature:

- Integrates properly with existing architecture
- Maintains code quality standards
- Includes comprehensive testing
- Is well-documented for users and developers

**Time to read:** 15 minutes  
**Prerequisites:** Familiarity with [DEVELOPMENT.md](../DEVELOPMENT.md)

---

## When to Use This Guide

Use this guide when:

- ✅ Adding new functional capabilities (APIs, endpoints, integrations)
- ✅ Implementing features from the roadmap
- ✅ Building user-facing functionality

**Don't use for:**

- ❌ Bug fixes (use standard PR process)
- ❌ Code improvements (see improvement guides)
- ❌ Documentation updates

---

## Prerequisites

Before starting:

- [ ] Feature is approved and in [ROADMAP.md](../ROADMAP.md)
- [ ] You've read the feature guide (e.g., `features/authentication.md`)
- [ ] Development environment is set up
- [ ] You understand the existing architecture

---

## Step-by-Step Process

### Phase 1: Planning & Design

#### 1.1 Create or Review Feature Document

**If feature guide exists:**

- Read the feature guide in `/features/`
- Understand requirements and design
- Note any changes needed

**If creating new feature:**

- Create feature document using template
- Define requirements and scope
- Propose technical design
- Get approval via GitHub Discussion

#### 1.2 Identify Integration Points

Map where your feature touches existing code:

```rust
// Example: Authentication feature integration points
- src/lib.rs        // Add middleware to router
- src/config.rs     // Add auth configuration
- src/middleware.rs // Create auth middleware module
- src/js_engine.rs  // Add user context to JavaScript
```

**Document:**

- Which modules will be modified
- Which new modules will be created
- What configuration changes are needed
- What dependencies must be added

#### 1.3 Plan Testing Strategy

Before writing code, plan how you'll test:

**Unit Tests:**

- Which functions need tests?
- What are the edge cases?
- What error scenarios exist?

**Integration Tests:**

- What end-to-end flows need testing?
- Which modules interact?
- What external dependencies exist?

**Example test plan:**

```markdown
## Testing Strategy for Authentication

### Unit Tests

- OAuth provider token exchange
- Session token generation/validation
- User context creation
- Error handling for each provider

### Integration Tests

- Complete OAuth flow (redirect → callback → session)
- Session persistence across requests
- JavaScript API access to user context
- Middleware authentication enforcement

### Manual Testing

- Test with each OAuth provider
- Session expiration behavior
- Error scenarios (invalid tokens, etc.)
```

---

### Phase 2: Implementation

#### 2.1 Create Module Structure

**Best practices:**

- Group related functionality in modules
- Keep modules focused and cohesive
- Follow existing patterns

**Example:**

```rust
// For authentication feature:
src/auth/
  ├── mod.rs              // Public API exports
  ├── config.rs           // Auth configuration
  ├── error.rs            // Auth-specific errors
  ├── middleware.rs       // Authentication middleware
  ├── session.rs          // Session management
  └── providers/
      ├── mod.rs          // Provider trait
      ├── google.rs       // Google OAuth
      ├── microsoft.rs    // Microsoft OAuth
      └── apple.rs        // Apple Sign In
```

#### 2.2 Implement Core Functionality

**Start with data structures:**

```rust
// 1. Define your types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    pub jwt_secret: String,
    pub session_timeout: Duration,
    // ...
}

// 2. Define error types
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("Invalid token: {0}")]
    InvalidToken(String),
    // ...
}

// 3. Define traits/interfaces
#[async_trait]
pub trait OAuth2Provider: Send + Sync {
    fn provider_name(&self) -> &'static str;
    async fn exchange_code(&self, code: &str) -> Result<Token, AuthError>;
}
```

**Then implement functionality:**

```rust
// 4. Implement core logic
pub struct AuthMiddleware {
    session_manager: Arc<SessionManager>,
    config: AuthConfig,
}

impl AuthMiddleware {
    pub fn new(config: AuthConfig) -> Self {
        Self {
            session_manager: Arc::new(SessionManager::new(&config)),
            config,
        }
    }

    pub async fn middleware(
        State(auth): State<Arc<AuthMiddleware>>,
        request: Request<Body>,
        next: Next,
    ) -> Result<Response, AuthError> {
        // Implementation with proper error handling
        // No unwrap() calls!
        // Clear error messages
    }
}
```

#### 2.3 Follow Coding Standards

**Mandatory:**

✅ **DO:**

- Use `Result<T, E>` for all fallible operations
- Add comprehensive error handling (zero `unwrap()`)
- Write doc comments for public APIs
- Use descriptive names
- Keep functions small (<50 lines)
- Follow Rust idioms

❌ **DON'T:**

- Use `unwrap()` or `expect()` in production code
- Add `TODO` comments without GitHub issues
- Leave commented-out code
- Ignore compiler warnings

**Example of good code:**

````rust
/// Validates an OAuth2 authorization code and exchanges it for an access token.
///
/// # Arguments
///
/// * `code` - The authorization code from the OAuth provider
/// * `redirect_uri` - The redirect URI used in the authorization request
///
/// # Returns
///
/// Returns `Ok(AccessToken)` on success, or an error if:
/// - The code is invalid or expired
/// - Network communication fails
/// - The provider returns an error
///
/// # Examples
///
/// ```rust
/// let token = provider.exchange_code("auth_code_123", "https://example.com/callback").await?;
/// ```
pub async fn exchange_code(
    &self,
    code: &str,
    redirect_uri: &str,
) -> Result<AccessToken, AuthError> {
    // Validate inputs
    if code.is_empty() {
        return Err(AuthError::InvalidCode("Code cannot be empty".into()));
    }

    // Make request with proper error handling
    let response = self.client
        .post(&self.token_url)
        .form(&self.build_token_request(code, redirect_uri))
        .send()
        .await
        .map_err(|e| AuthError::NetworkError(e.to_string()))?;

    // Parse response
    let token = response
        .json::<TokenResponse>()
        .await
        .map_err(|e| AuthError::ParseError(e.to_string()))?
        .into();

    Ok(token)
}
````

#### 2.4 Integrate with Existing Systems

**Configuration integration:**

```rust
// 1. Add to src/config.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    // ... existing fields
    #[serde(default)]
    pub auth: Option<AuthConfig>,
}

// 2. Update config files
// config.toml
[auth]
jwt_secret = "${JWT_SECRET}"
session_timeout_secs = 3600
```

**Main application integration:**

```rust
// 3. Update src/lib.rs or src/main.rs
pub async fn create_app(config: AppConfig) -> Router {
    let mut router = Router::new()
        // ... existing routes
        ;

    // Add feature-specific routes and middleware
    if let Some(auth_config) = config.auth {
        let auth_middleware = AuthMiddleware::new(auth_config);
        router = router
            .route("/login", get(login_handler))
            .route("/auth/callback/:provider", get(callback_handler))
            .layer(middleware::from_fn_with_state(
                Arc::new(auth_middleware),
                auth_middleware_fn
            ));
    }

    router
}
```

**JavaScript API integration:**

```rust
// 4. Add JavaScript APIs in src/js_engine.rs
pub fn setup_feature_globals(
    ctx: &Context,
    feature_state: Arc<FeatureState>,
) -> Result<(), JsError> {
    // Add global functions accessible from JavaScript
    ctx.globals().set(
        "getCurrentUser",
        Func::from(move |ctx: Ctx| {
            // Implementation
        })
    )?;

    Ok(())
}
```

---

### Phase 3: Testing

#### 3.1 Write Unit Tests

**Coverage requirements:**

- > 90% for new code
- All public functions tested
- All error paths tested
- Edge cases covered

**Example:**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_creation() {
        let manager = SessionManager::new(&test_config());
        let session = manager.create_session("user123").unwrap();

        assert!(!session.token.is_empty());
        assert_eq!(session.user_id, "user123");
        assert!(session.expires_at > Utc::now());
    }

    #[test]
    fn test_session_validation_with_expired_token() {
        let manager = SessionManager::new(&test_config());
        let expired_token = create_expired_token();

        let result = manager.validate_session(&expired_token);

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), SessionError::Expired));
    }

    #[test]
    fn test_session_validation_with_invalid_signature() {
        let manager = SessionManager::new(&test_config());
        let invalid_token = "invalid.token.signature";

        let result = manager.validate_session(invalid_token);

        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), SessionError::InvalidSignature));
    }
}
```

#### 3.2 Write Integration Tests

**Location:** `tests/` directory or `#[cfg(test)]` with `#[tokio::test]`

**Example:**

```rust
// tests/auth_integration.rs
#[tokio::test]
async fn test_complete_oauth_flow() {
    // 1. Setup test app
    let app = create_test_app_with_auth().await;

    // 2. Initiate OAuth flow
    let response = app
        .request()
        .uri("/auth/login/google")
        .send()
        .await;

    assert_eq!(response.status(), StatusCode::FOUND);
    let redirect_url = response.headers().get("Location").unwrap();
    assert!(redirect_url.to_str().unwrap().contains("accounts.google.com"));

    // 3. Simulate OAuth callback
    let callback_response = app
        .request()
        .uri("/auth/callback/google?code=test_code&state=test_state")
        .send()
        .await;

    assert_eq!(callback_response.status(), StatusCode::FOUND);

    // 4. Verify session cookie is set
    let cookies = callback_response.headers().get_all("Set-Cookie");
    assert!(cookies.iter().any(|c| {
        c.to_str().unwrap().contains("session_token")
    }));
}
```

#### 3.3 Manual Testing

Create a checklist for manual testing:

```markdown
## Manual Testing Checklist

- [ ] Happy path works end-to-end
- [ ] Error messages are clear
- [ ] Configuration is flexible
- [ ] Performance is acceptable
- [ ] Security measures work
- [ ] Documentation is accurate
```

---

### Phase 4: Documentation

#### 4.1 Code Documentation

**Required:**

````rust
// 1. Module-level documentation
//! # Authentication Module
//!
//! This module provides OAuth2 authentication for aiwebengine.
//! It supports Google, Microsoft, and Apple as OAuth providers.
//!
//! ## Usage
//!
//! ```rust
//! let config = AuthConfig { /* ... */ };
//! let auth = AuthMiddleware::new(config);
//! ```

// 2. Public API documentation
/// Creates a new session for an authenticated user.
///
/// # Arguments
///
/// * `user_id` - Unique identifier for the user
///
/// # Returns
///
/// Returns `Ok(Session)` with a signed JWT token, or an error if
/// session creation fails.
///
/// # Examples
///
/// ```rust
/// let session = manager.create_session("user123")?;
/// println!("Token: {}", session.token);
/// ```
pub fn create_session(&self, user_id: &str) -> Result<Session, SessionError> {
    // Implementation
}
````

#### 4.2 User Documentation

**Update for user-facing features:**

- `docs/solution-developers/` - For JavaScript developers
- `docs/engine-administrators/` - For deployment/operations

**Example for authentication:**

```markdown
<!-- docs/solution-developers/guides/authentication.md -->

# Using Authentication in Your Scripts

You can access the authenticated user's information in your JavaScript handlers:

\`\`\`javascript
function protectedHandler(req) {
// Access user information
if (!req.user) {
return {
status: 401,
body: "Authentication required"
};
}

return {
status: 200,
body: `Hello, ${req.user.name}!`
};
}

register("/protected", "protectedHandler", "GET");
\`\`\`
```

#### 4.3 Example Scripts

Create examples in `scripts/example_scripts/`:

```javascript
// scripts/example_scripts/authentication_example.js

// Simple authentication check
function checkAuthHandler(req) {
  if (req.user) {
    return {
      status: 200,
      body: JSON.stringify({
        message: "You are authenticated",
        user_id: req.user.id,
        email: req.user.email,
      }),
      contentType: "application/json",
    };
  } else {
    return {
      status: 401,
      body: JSON.stringify({
        error: "Authentication required",
      }),
      contentType: "application/json",
    };
  }
}

register("/api/check-auth", "checkAuthHandler", "GET");
```

#### 4.4 Update Roadmap and Guides

- [ ] Update [ROADMAP.md](../ROADMAP.md) - Mark feature as implemented
- [ ] Update feature guide - Add implementation status
- [ ] Update [CONTRIBUTING.md](../CONTRIBUTING.md) if process changed
- [ ] Update main [INDEX.md](../../INDEX.md) if needed

---

### Phase 5: Review and Merge

#### 5.1 Self-Review

Before submitting PR:

- [ ] All tests pass locally
- [ ] No compiler warnings
- [ ] No clippy warnings
- [ ] Code is formatted (`cargo fmt`)
- [ ] Documentation is complete
- [ ] Examples work

**Run these commands:**

```bash
# Format code
cargo fmt --all

# Check for issues
cargo clippy --all-targets -- -D warnings

# Run tests
cargo test --all-features

# Generate coverage
cargo llvm-cov --all-features --html

# Build documentation
cargo doc --no-deps --open
```

#### 5.2 Create Pull Request

**PR Title:** Clear and descriptive

```
Add OAuth2 authentication with Google, Microsoft, and Apple providers
```

**PR Description:** Use the template from [CONTRIBUTING.md](../CONTRIBUTING.md)

**Include:**

- What was implemented
- How it was tested
- Documentation updates
- Breaking changes (if any)
- Screenshots (if UI changes)

#### 5.3 Address Review Feedback

- Respond to all comments
- Make requested changes
- Re-request review when ready
- Be patient and professional

---

## Common Pitfalls

### 1. Forgetting Error Handling

❌ **Don't:**

```rust
let config = load_config().unwrap();
```

✅ **Do:**

```rust
let config = load_config()
    .map_err(|e| AppError::ConfigError(e.to_string()))?;
```

### 2. Inadequate Testing

❌ **Don't:**

```rust
#[test]
fn test_feature() {
    assert!(feature_works());
}
```

✅ **Do:**

```rust
#[test]
fn test_feature_success_case() { /* ... */ }

#[test]
fn test_feature_error_invalid_input() { /* ... */ }

#[test]
fn test_feature_edge_case_empty_string() { /* ... */ }

#[test]
fn test_feature_concurrent_access() { /* ... */ }
```

### 3. Poor Documentation

❌ **Don't:**

```rust
// Creates session
pub fn create_session(&self, id: &str) -> Session { }
```

✅ **Do:**

```rust
/// Creates an authenticated session for a user.
///
/// Generates a signed JWT token with the user's ID and creates
/// a session record that expires after the configured timeout.
///
/// # Arguments
///
/// * `user_id` - Unique identifier for the authenticated user
///
/// # Returns
///
/// Returns `Ok(Session)` containing a JWT token and expiration time.
/// Returns `Err` if token generation fails or session storage is unavailable.
pub fn create_session(&self, user_id: &str) -> Result<Session, SessionError> { }
```

### 4. Tight Coupling

❌ **Don't:**

```rust
// Direct dependency on specific implementation
pub struct AuthMiddleware {
    google_provider: GoogleProvider,
}
```

✅ **Do:**

```rust
// Depend on trait, not implementation
pub struct AuthMiddleware {
    providers: HashMap<String, Box<dyn OAuth2Provider>>,
}
```

### 5. Missing Configuration

❌ **Don't:**

```rust
// Hardcoded values
const SESSION_TIMEOUT: u64 = 3600;
```

✅ **Do:**

```rust
// Configurable with sensible defaults
#[derive(Deserialize)]
pub struct AuthConfig {
    #[serde(default = "default_session_timeout")]
    pub session_timeout_secs: u64,
}

fn default_session_timeout() -> u64 {
    3600
}
```

---

## Checklist

Use this checklist to ensure completeness:

### Planning

- [ ] Feature is in roadmap and approved
- [ ] Feature guide exists or created
- [ ] Integration points identified
- [ ] Testing strategy planned
- [ ] Dependencies identified

### Implementation

- [ ] Module structure created
- [ ] Core functionality implemented
- [ ] Error handling comprehensive
- [ ] Configuration integrated
- [ ] JavaScript APIs added (if needed)
- [ ] No `unwrap()` in production code

### Testing

- [ ] Unit tests written (>90% coverage)
- [ ] Integration tests written
- [ ] All tests pass
- [ ] Manual testing completed
- [ ] Performance acceptable

### Documentation

- [ ] Code documentation complete
- [ ] User guides updated
- [ ] Admin guides updated (if needed)
- [ ] Example scripts created
- [ ] Roadmap updated

### Quality

- [ ] `cargo fmt` run
- [ ] `cargo clippy` passes
- [ ] No compiler warnings
- [ ] Coverage report generated
- [ ] Security checklist reviewed

### Submission

- [ ] PR created with good description
- [ ] CI/CD passes
- [ ] Review feedback addressed
- [ ] Approved by maintainers

---

## Related Resources

- [CONTRIBUTING.md](../CONTRIBUTING.md) - Contribution process
- [DEVELOPMENT.md](../DEVELOPMENT.md) - Development guidelines
- [testing-guidelines.md](./testing-guidelines.md) - Testing best practices
- [security-checklist.md](./security-checklist.md) - Security review
- [ROADMAP.md](../ROADMAP.md) - Development priorities

---

_This guide is a living document. Suggest improvements via PR!_
