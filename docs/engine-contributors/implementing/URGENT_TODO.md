# URGENT TODO: Pre-Authentication Development Requirements

**Date Created:** October 11, 2025  
**Status:** CRITICAL - BLOCKING for Authentication Development  
**Total Estimated Effort:** 8-12 days of focused work

---

## 🎯 Executive Summary

Before implementing session management and authentication, **7 critical areas** require improvement to ensure stability, security, and maintainability. The codebase (~121K lines) has a solid security foundation but has implementation gaps that must be closed.

**Current Test Status:** 125/126 passing (99.2%) - 1 failing test must be fixed  
**Current Issues:** 20+ `unwrap()` calls, 9 compiler warnings, 18 security TODOs

---

## 1. 🔴 CRITICAL: Error Handling & Stability

### Current State

- ❌ **20+ `unwrap()` calls** in production code paths (lib.rs, js_engine.rs)
- ❌ **Mutex poisoning recovery** partially implemented but incomplete
- ❌ **No timeout enforcement** for JavaScript execution
- ⚠️ 1 test currently failing (`test_register_web_stream_invalid_path`)

### Impact on Authentication

Session management **requires** bullet-proof error handling. A single `unwrap()` panic during authentication can:

- Crash the server and lose all sessions
- Expose security vulnerabilities
- Break mutex locks permanently

### Required Actions

#### Fix All Unwrap Calls

```rust
// BEFORE (Current - Dangerous):
// src/js_engine.rs:1250
let result_value = result.as_string().unwrap().to_string()?;

// AFTER (Required):
let result_value = result.as_string()
    .ok_or_else(|| JsError::InvalidReturnType("Expected string".into()))?
    .to_string()?;
```

#### Locations to Fix

- `src/js_engine.rs`: Lines 994, 1250, 1447, 1466, 1486, 1764
- `src/lib.rs`: Lines 75, 308, 319, 379, 441-442, 448-449, 529-530, 558-559, 567-568, 851

#### Checklist

- [ ] Replace all `unwrap()`/`expect()` with proper `Result<T, E>` handling
- [ ] Fix the failing test: `test_register_web_stream_invalid_path`
- [ ] Implement comprehensive timeout mechanism for JS execution
- [ ] Add circuit breakers for mutex locks with automatic recovery
- [ ] Implement graceful degradation when components fail
- [ ] Add integration tests for error paths

**Estimated Effort:** 2-3 days

---

## 2. 🟠 HIGH: Security Integration Gaps

### Current State

- ✅ Security framework structure exists (validation, audit, capabilities)
- ❌ **Security not enforced** in actual execution paths
- ❌ `setup_global_functions()` marked as LEGACY but still present (line 92, unused warning)
- ⚠️ **18 TODO items** in security module indicating incomplete implementation

### Critical Gaps

**Security Operations Not Connected:**

```
src/security/operations.rs:79  - TODO: Call actual repository layer here
src/security/operations.rs:109 - TODO: Call actual asset storage here
src/security/operations.rs:151 - TODO: Implement actual HTTP client here
src/security/operations.rs:175 - TODO: Implement actual GraphQL schema update
src/security/operations.rs:211 - TODO: Implement actual stream creation
```

**Audit Integration Incomplete:**

```
src/security/audit.rs:192 - TODO: Store in database and/or send to SIEM
src/security/audit.rs:349 - TODO: Integrate with firewall/WAF API
src/security/audit.rs:361 - TODO: Integrate with alerting system
src/security/audit.rs:371 - TODO: Integrate with user management system
src/security/audit.rs:432 - TODO: Implement alerting mechanism
```

**Secure Globals Not Integrated:**

```
src/security/secure_globals.rs:514  - TODO: Proper async handling for asset operations
src/security/secure_globals.rs:694  - TODO: Proper async handling for GraphQL operations
src/security/secure_globals.rs:805  - TODO: Proper async handling for GraphQL operations
src/security/secure_globals.rs:917  - TODO: Proper async handling for GraphQL operations
src/security/secure_globals.rs:1043 - TODO: Proper async handling for stream operations
src/security/secure_globals.rs:1180 - TODO: Call actual stream message sending
src/security/secure_globals.rs:1245 - TODO: Call actual path-specific stream message sending
src/security/secure_globals.rs:1316 - TODO: Call actual GraphQL subscription message sending
```

### Impact on Authentication

- Authentication tokens could bypass security validation
- User contexts may not enforce capabilities properly
- Secure global functions not actually being used

### Required Actions

#### Priority 1: Connect Security to Execution Flow

```rust
// In js_engine.rs - Replace all instances like this:
fn execute_script(...) -> ExecutionResult {
    // CURRENT: No security validation

    // REQUIRED: Enforce security first
    let user_context = params.user_context;
    let config = GlobalSecurityConfig::default();

    // Use secure_global_functions, not legacy version
    setup_secure_global_functions(ctx, script_uri, user_context, &config, ...)?;
}
```

#### Checklist

- [ ] Remove `setup_global_functions()` completely (currently dead code at js_engine.rs:92)
- [ ] Integrate `SecureOperations` into all repository calls
- [ ] Implement all 18 security TODOs or document why they're deferred
- [ ] Add integration tests proving security enforcement works
- [ ] Ensure `UserContext` is validated on every request
- [ ] Connect audit logging to all security-sensitive operations
- [ ] Implement async-safe security validation for all global functions

**Estimated Effort:** 3-4 days

---

## 3. 🟡 MEDIUM: Testing Infrastructure

### Current State

- ✅ 126 tests, 125 passing (99.2% pass rate)
- ✅ Good test coverage for config, error types, middleware
- ❌ **Integration tests exist but may be incomplete**
- ❌ **No security integration tests** (validating actual enforcement)
- ❌ Missing tests for critical paths (graphql.rs module mentioned in TODO.md)

### Impact on Authentication

Without comprehensive tests, you can't verify that:

- Sessions are properly validated
- Authentication failures are handled correctly
- Security bypasses are impossible

### Required Actions

#### Fix Failing Test

```bash
# Run and analyze the failing test
cargo test test_register_web_stream_invalid_path -- --nocapture

# Fix the root cause before proceeding
```

#### Add Security Integration Tests

```rust
// tests/security_integration.rs - CREATE NEW FILE

#[tokio::test]
async fn test_capability_enforcement_blocks_unauthorized() {
    // Verify that missing WriteScripts capability prevents script upsert
    let user = UserContext::new("test_user", vec![Capability::ReadScripts]);
    let ops = SecureOperations::new();
    let request = UpsertScriptRequest {
        script_name: "test.js".to_string(),
        js_script: "console.log('test')".to_string(),
    };

    let result = ops.upsert_script(&user, request).await;
    assert!(result.is_ok());
    let op_result = result.unwrap();
    assert!(!op_result.success);
    assert!(op_result.error.unwrap().contains("Access denied"));
}

#[tokio::test]
async fn test_validation_prevents_dangerous_patterns() {
    // Verify that eval(), __proto__ etc. are blocked
    let user = UserContext::new("test_user", vec![Capability::WriteScripts]);
    let ops = SecureOperations::new();
    let request = UpsertScriptRequest {
        script_name: "malicious.js".to_string(),
        js_script: "eval('malicious code')".to_string(),
    };

    let result = ops.upsert_script(&user, request).await;
    assert!(result.is_ok());
    let op_result = result.unwrap();
    assert!(!op_result.success);
    assert!(op_result.error.unwrap().contains("Dangerous pattern"));
}

#[tokio::test]
async fn test_xss_prevention_in_validation() {
    // Test XSS pattern detection
}

#[tokio::test]
async fn test_path_traversal_prevention() {
    // Test path traversal detection
}

#[tokio::test]
async fn test_rate_limiting_enforcement() {
    // Test rate limiter actually blocks requests
}
```

#### Checklist

- [ ] Fix failing test: `test_register_web_stream_invalid_path`
- [ ] Create `tests/security_integration.rs` with comprehensive security tests
- [ ] Add tests for `graphql.rs` module (currently 0 tests per TODO.md)
- [ ] Add error path tests for all critical functions
- [ ] Add load/stress tests for session management scenarios
- [ ] Set up CI/CD with test coverage reporting (aim for >85%)
- [ ] Add property-based testing for input validation
- [ ] Test mutex poisoning recovery mechanisms

**Estimated Effort:** 2-3 days

---

## 4. 🟡 MEDIUM: Configuration & Environment Management

### Current State

- ✅ Comprehensive config structure (`AppConfig` with 6 major sections)
- ✅ Multiple config files (dev, staging, prod)
- ❌ **Environment-specific security settings** not clearly defined
- ❌ **Secrets management** not implemented (needed for JWT keys, OAuth secrets)

### Impact on Authentication

- Where will you store JWT signing keys?
- How will OAuth client secrets be managed?
- How do security settings differ between dev/prod?
- How are secrets rotated?

### Required Actions

#### Add Authentication Configuration

```rust
// src/config.rs - ADD TO AppConfig

/// Authentication and session configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// Session secret for signing tokens (MUST be from env var)
    #[serde(skip_serializing)]  // Never serialize secrets
    pub session_secret: String,

    /// Session timeout in seconds
    pub session_timeout_secs: u64,

    /// JWT issuer identifier
    pub jwt_issuer: String,

    /// JWT audience identifier
    pub jwt_audience: String,

    /// Enable session persistence
    pub enable_session_persistence: bool,

    /// Session storage backend (memory, redis)
    pub session_storage: String,

    /// Redis connection string (if using redis storage)
    pub redis_url: Option<String>,

    /// OAuth provider configurations
    pub oauth_providers: Vec<OAuthProviderConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthProviderConfig {
    pub provider_name: String,
    pub client_id: String,
    #[serde(skip_serializing)]
    pub client_secret: String,
    pub auth_url: String,
    pub token_url: String,
    pub redirect_url: String,
    pub scopes: Vec<String>,
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            // Session secret MUST come from environment
            session_secret: std::env::var("SESSION_SECRET")
                .expect("SESSION_SECRET environment variable required"),
            session_timeout_secs: 3600,  // 1 hour
            jwt_issuer: "aiwebengine".to_string(),
            jwt_audience: "aiwebengine-users".to_string(),
            enable_session_persistence: false,
            session_storage: "memory".to_string(),
            redis_url: None,
            oauth_providers: vec![],
        }
    }
}
```

#### Create .env.example

```bash
# .env.example - CREATE NEW FILE

# Security & Authentication
SESSION_SECRET=change-this-to-a-random-secret-min-32-chars
JWT_SECRET=another-random-secret-for-jwt-signing

# OAuth Providers (optional)
GOOGLE_CLIENT_ID=your-google-client-id
GOOGLE_CLIENT_SECRET=your-google-client-secret

# Database (when implemented)
DATABASE_URL=postgresql://user:pass@localhost/aiwebengine

# Redis (for session storage)
REDIS_URL=redis://localhost:6379

# Environment
ENVIRONMENT=development
LOG_LEVEL=debug
```

#### Update Documentation

```markdown
# docs/CONFIGURATION.md - UPDATE

## Required Environment Variables

### Security & Authentication

- `SESSION_SECRET` (required): Secret key for session signing. Must be at least 32 characters.
- `JWT_SECRET` (required): Secret key for JWT token signing.

### OAuth Configuration (optional)

- `GOOGLE_CLIENT_ID`: Google OAuth client ID
- `GOOGLE_CLIENT_SECRET`: Google OAuth client secret
- `MICROSOFT_CLIENT_ID`: Microsoft OAuth client ID
- `MICROSOFT_CLIENT_SECRET`: Microsoft OAuth client secret

### Key Rotation

Sessions should be rotated regularly. Update secrets:

1. Set new `SESSION_SECRET_NEW` environment variable
2. Run migration to re-sign sessions
3. Update `SESSION_SECRET` to new value
4. Remove `SESSION_SECRET_NEW`
```

#### Checklist

- [ ] Add `AuthConfig` to `src/config.rs`
- [ ] Add `auth` field to `AppConfig` struct
- [ ] Create `.env.example` with all required variables
- [ ] Implement secrets loading from environment variables
- [ ] Validate that secrets are never logged or exposed
- [ ] Add validation for minimum secret length (32+ chars)
- [ ] Document key rotation mechanism in docs/CONFIGURATION.md
- [ ] Add config validation tests for auth section
- [ ] Ensure secrets are not committed to git (.gitignore check)

**Estimated Effort:** 1-2 days

---

## 5. 🟢 LOW: Code Quality & Maintainability

### Current State

- ✅ Good development documentation (DEVELOPMENT.md)
- ⚠️ 9 compiler warnings (unused imports, variables)
- ⚠️ Some dead code (`setup_global_functions`, `RouteRegisterFn`)
- ⚠️ Unused struct fields in `ThreatDetector`

### Current Warnings

```
warning: unused import: `std::collections::HashMap`
 --> src/security/secure_globals.rs:3:5

warning: unused variable: `secure_ops_asset`
   --> src/security/secure_globals.rs:497:13

warning: unused variable: `secure_ops_query`
   --> src/security/secure_globals.rs:645:13

warning: unused variable: `secure_ops_mutation`
   --> src/security/secure_globals.rs:757:13

warning: unused variable: `secure_ops_subscription`
   --> src/security/secure_globals.rs:869:13

warning: unused variable: `secure_ops_register`
   --> src/security/secure_globals.rs:992:13

warning: function `setup_global_functions` is never used
  --> src/js_engine.rs:92:4

warning: type alias `RouteRegisterFn` is never used
  --> src/security/secure_globals.rs:12:6

warning: fields `geo_anomalies` and `rate_limit_violations` are never read
  --> src/security/threat_detection.rs:19:5
```

### Required Actions

#### Fix All Compiler Warnings

```bash
# Auto-fix what can be fixed automatically
cargo fix --lib -p aiwebengine

# Run clippy for additional issues
cargo clippy --all-targets -- -D warnings

# Format code
cargo fmt --all
```

#### Manual Fixes Needed

```rust
// src/js_engine.rs:92 - DELETE THIS ENTIRE FUNCTION
// It's marked as LEGACY and unused
fn setup_global_functions(...) {
    // DELETE - use setup_secure_global_functions instead
}

// src/security/secure_globals.rs:12 - DELETE OR USE
type RouteRegisterFn = dyn Fn(&str, &str, Option<&str>) -> Result<(), rquickjs::Error>;

// src/security/secure_globals.rs - Prefix unused vars with underscore
let _secure_ops_asset = secure_ops.clone();
let _secure_ops_query = secure_ops.clone();
// ... etc

// src/security/threat_detection.rs - Either use or remove
pub struct ThreatDetector {
    // Either implement functionality using these or remove them
    geo_anomalies: Arc<RwLock<HashMap<String, Vec<GeoLocation>>>>,
    rate_limit_violations: Arc<RwLock<HashMap<String, VecDeque<DateTime<Utc>>>>>,
}
```

#### Checklist

- [ ] Run `cargo fix --lib -p aiwebengine` and commit results
- [ ] Run `cargo clippy --all-targets -- -D warnings` and fix all issues
- [ ] Run `cargo fmt --all -- --check` and fix formatting
- [ ] Remove `setup_global_functions` from `src/js_engine.rs`
- [ ] Fix or remove `RouteRegisterFn` in `src/security/secure_globals.rs`
- [ ] Prefix all intentionally unused variables with underscore
- [ ] Implement or remove unused `ThreatDetector` fields
- [ ] Add pre-commit hooks to prevent warnings from being committed
- [ ] Set up CI to fail on warnings
- [ ] Document code quality standards in DEVELOPMENT.md

**Estimated Effort:** 0.5-1 day

---

## 6. 🟢 LOW: Development Workflow Improvements

### Current State

- ✅ Clear documentation structure (docs/, examples, README)
- ✅ Script organization (feature_scripts, test_scripts, example_scripts)
- ❌ **No automatic reload** for development
- ❌ **No Docker setup** for consistent dev environments
- ❌ **No Makefile** for common commands

### Required Actions

#### Install Development Tools

```bash
# Auto-reload on file changes
cargo install cargo-watch

# Better test output
cargo install cargo-nextest

# Code coverage
cargo install cargo-llvm-cov
```

#### Create Makefile

```makefile
# Makefile - CREATE NEW FILE

.PHONY: help test dev build clean lint format coverage

help:
	@echo "Available commands:"
	@echo "  make dev       - Run development server with auto-reload"
	@echo "  make test      - Run all tests"
	@echo "  make lint      - Run clippy linter"
	@echo "  make format    - Format code with rustfmt"
	@echo "  make coverage  - Generate test coverage report"
	@echo "  make build     - Build release binary"
	@echo "  make clean     - Clean build artifacts"

dev:
	cargo watch -x 'run --bin server'

test:
	cargo nextest run --all-features

test-simple:
	cargo test --all-features

lint:
	cargo clippy --all-targets -- -D warnings

format:
	cargo fmt --all

format-check:
	cargo fmt --all -- --check

coverage:
	cargo llvm-cov --all-features --html
	@echo "Coverage report: target/llvm-cov/html/index.html"

build:
	cargo build --release

clean:
	cargo clean

# Pre-commit checks
check: format-check lint test
	@echo "All checks passed!"

# CI pipeline
ci: format-check lint test coverage
	@echo "CI pipeline completed!"
```

#### Create Docker Setup

```dockerfile
# Dockerfile - CREATE NEW FILE

FROM rust:1.75 as builder

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY scripts ./scripts

RUN cargo build --release

FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/server /usr/local/bin/aiwebengine

EXPOSE 8080

CMD ["aiwebengine"]
```

```yaml
# docker-compose.yml - CREATE NEW FILE

version: "3.8"

services:
  aiwebengine:
    build: .
    ports:
      - "8080:8080"
    environment:
      - SESSION_SECRET=${SESSION_SECRET}
      - JWT_SECRET=${JWT_SECRET}
      - LOG_LEVEL=debug
      - ENVIRONMENT=development
    volumes:
      - ./config.local.toml:/app/config.toml:ro
    depends_on:
      - redis

  redis:
    image: redis:7-alpine
    ports:
      - "6379:6379"
    volumes:
      - redis_data:/data

volumes:
  redis_data:
```

#### Update Documentation

````markdown
# docs/local-development.md - UPDATE

## Quick Start

### Using Make (Recommended)

```bash
# Run development server with auto-reload
make dev

# Run tests
make test

# Run all checks (format, lint, test)
make check
```
````

### Using Docker

```bash
# Build and run with Docker Compose
docker-compose up

# Run tests in Docker
docker-compose run aiwebengine cargo test
```

### Manual Setup

```bash
# Install development tools
make install-tools

# Or manually:
cargo install cargo-watch cargo-nextest cargo-llvm-cov

# Run development server
cargo run --bin server

# Run tests with auto-reload
cargo watch -x test
```

````

#### Checklist
- [ ] Install `cargo-watch`, `cargo-nextest`, `cargo-llvm-cov`
- [ ] Create `Makefile` with common development commands
- [ ] Create `Dockerfile` for containerized deployment
- [ ] Create `docker-compose.yml` for local development with Redis
- [ ] Update `docs/local-development.md` with new workflow
- [ ] Add `.dockerignore` file
- [ ] Test Docker build and deployment
- [ ] Document CI/CD pipeline setup
- [ ] Add pre-commit hook configuration

**Estimated Effort:** 1 day

---

## 7. 🔵 ARCHITECTURE: Session Storage Foundation

### Current State
- ✅ In-memory storage for scripts/logs/assets (using `OnceLock<Mutex<HashMap>>`)
- ❌ **No persistent storage** implementation yet
- ❌ **Session storage strategy** not defined

### Impact on Authentication
Sessions stored only in memory will be lost on restart, forcing all users to re-authenticate.

### Design Decision Required

#### Option A: In-Memory First (Recommended for Initial Development)
```rust
// src/session.rs - CREATE NEW FILE

use chrono::{DateTime, Utc};
use std::collections::{HashMap, HashSet};
use std::sync::{Mutex, OnceLock};
use uuid::Uuid;
use crate::security::Capability;

#[derive(Debug, Clone)]
pub struct SessionData {
    pub session_id: String,
    pub user_id: String,
    pub username: String,
    pub email: Option<String>,
    pub capabilities: HashSet<Capability>,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub last_activity: DateTime<Utc>,
}

static SESSIONS: OnceLock<Mutex<HashMap<String, SessionData>>> = OnceLock::new();

pub struct SessionStore;

impl SessionStore {
    pub fn new() -> Self {
        Self
    }

    fn get_store() -> &'static Mutex<HashMap<String, SessionData>> {
        SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
    }

    pub fn create_session(&self, user_id: String, username: String, capabilities: HashSet<Capability>) -> Result<SessionData, SessionError> {
        let session_id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let expires_at = now + chrono::Duration::hours(24);

        let session = SessionData {
            session_id: session_id.clone(),
            user_id,
            username,
            email: None,
            capabilities,
            created_at: now,
            expires_at,
            last_activity: now,
        };

        let mut store = Self::get_store().lock()
            .map_err(|_| SessionError::LockError)?;
        store.insert(session_id.clone(), session.clone());

        Ok(session)
    }

    pub fn get_session(&self, session_id: &str) -> Result<Option<SessionData>, SessionError> {
        let mut store = Self::get_store().lock()
            .map_err(|_| SessionError::LockError)?;

        if let Some(session) = store.get_mut(session_id) {
            // Check expiration
            if session.expires_at < Utc::now() {
                store.remove(session_id);
                return Ok(None);
            }

            // Update last activity
            session.last_activity = Utc::now();
            return Ok(Some(session.clone()));
        }

        Ok(None)
    }

    pub fn delete_session(&self, session_id: &str) -> Result<(), SessionError> {
        let mut store = Self::get_store().lock()
            .map_err(|_| SessionError::LockError)?;
        store.remove(session_id);
        Ok(())
    }

    pub fn cleanup_expired(&self) -> Result<usize, SessionError> {
        let mut store = Self::get_store().lock()
            .map_err(|_| SessionError::LockError)?;
        let now = Utc::now();
        let before_count = store.len();

        store.retain(|_, session| session.expires_at > now);

        Ok(before_count - store.len())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("Session lock error")]
    LockError,
    #[error("Session not found")]
    NotFound,
    #[error("Session expired")]
    Expired,
}
````

**Pros:**

- ✅ Fast, simple to implement
- ✅ No external dependencies
- ✅ Good for initial development and testing
- ✅ Can migrate to persistent storage later

**Cons:**

- ❌ Lost on server restart
- ❌ Cannot scale horizontally (single server only)
- ❌ Not suitable for production long-term

#### Option B: Redis-Based (Future Migration Path)

```rust
// src/session.rs - FUTURE IMPLEMENTATION

use redis::{AsyncCommands, Client};

pub struct RedisSessionStore {
    client: Client,
}

impl RedisSessionStore {
    pub async fn new(redis_url: &str) -> Result<Self, SessionError> {
        let client = Client::open(redis_url)
            .map_err(|e| SessionError::ConnectionError(e.to_string()))?;
        Ok(Self { client })
    }

    pub async fn create_session(&self, user_id: String, username: String, capabilities: HashSet<Capability>) -> Result<SessionData, SessionError> {
        let session_id = Uuid::new_v4().to_string();
        let session = SessionData {
            session_id: session_id.clone(),
            user_id,
            username,
            // ... other fields
        };

        let mut conn = self.client.get_async_connection().await
            .map_err(|e| SessionError::ConnectionError(e.to_string()))?;

        let session_json = serde_json::to_string(&session)
            .map_err(|e| SessionError::SerializationError(e.to_string()))?;

        // Set with expiration
        conn.set_ex(&session_id, session_json, 86400).await
            .map_err(|e| SessionError::StorageError(e.to_string()))?;

        Ok(session)
    }

    // ... other methods
}
```

**Pros:**

- ✅ Persists across restarts
- ✅ Can scale horizontally
- ✅ Built-in expiration support
- ✅ Production-ready

**Cons:**

- ❌ Requires Redis infrastructure
- ❌ More complex setup
- ❌ Network latency for each session check

### Recommended Approach

**Phase 1 (Now):** Implement Option A (In-Memory)

- Fast to implement
- Sufficient for development and testing
- Easy to understand and debug

**Phase 2 (Later):** Add trait abstraction and Redis backend

```rust
#[async_trait]
pub trait SessionStore: Send + Sync {
    async fn create_session(&self, user_id: String, username: String, capabilities: HashSet<Capability>) -> Result<SessionData, SessionError>;
    async fn get_session(&self, session_id: &str) -> Result<Option<SessionData>, SessionError>;
    async fn delete_session(&self, session_id: &str) -> Result<(), SessionError>;
    async fn cleanup_expired(&self) -> Result<usize, SessionError>;
}

// Can swap implementations without changing authentication code
let store: Box<dyn SessionStore> = if config.auth.session_storage == "redis" {
    Box::new(RedisSessionStore::new(&config.auth.redis_url).await?)
} else {
    Box::new(InMemorySessionStore::new())
};
```

### Required Actions

#### Implement In-Memory Session Store

```rust
// src/session.rs - CREATE THIS FILE
// See Option A implementation above
```

#### Add Session Cleanup Task

```rust
// src/lib.rs or src/bin/server.rs - ADD BACKGROUND TASK

use tokio::time::{interval, Duration};

async fn start_session_cleanup_task(store: Arc<SessionStore>) {
    let mut cleanup_interval = interval(Duration::from_secs(300)); // Every 5 minutes

    loop {
        cleanup_interval.tick().await;

        match store.cleanup_expired() {
            Ok(count) if count > 0 => {
                info!("Cleaned up {} expired sessions", count);
            }
            Err(e) => {
                error!("Session cleanup failed: {}", e);
            }
            _ => {}
        }
    }
}

// In app startup:
let session_store = Arc::new(SessionStore::new());
tokio::spawn(start_session_cleanup_task(session_store.clone()));
```

#### Add Session Tests

```rust
// src/session.rs - ADD TESTS MODULE

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_session() {
        let store = SessionStore::new();
        let capabilities = HashSet::from([Capability::ReadScripts]);

        let result = store.create_session(
            "user123".to_string(),
            "testuser".to_string(),
            capabilities
        );

        assert!(result.is_ok());
        let session = result.unwrap();
        assert_eq!(session.user_id, "user123");
    }

    #[test]
    fn test_get_session() {
        let store = SessionStore::new();
        let session = store.create_session(
            "user123".to_string(),
            "testuser".to_string(),
            HashSet::new()
        ).unwrap();

        let result = store.get_session(&session.session_id);
        assert!(result.is_ok());
        assert!(result.unwrap().is_some());
    }

    #[test]
    fn test_session_expiration() {
        // Test that expired sessions are not returned
    }

    #[test]
    fn test_cleanup_expired() {
        // Test automatic cleanup
    }

    #[test]
    fn test_delete_session() {
        // Test manual deletion
    }
}
```

#### Checklist

- [ ] Create `src/session.rs` with in-memory session store
- [ ] Add `SessionData` struct with all required fields
- [ ] Implement session CRUD operations (create, get, delete)
- [ ] Add session expiration checking
- [ ] Implement automatic cleanup mechanism
- [ ] Design session ID generation using `uuid` crate v4
- [ ] Add background task for periodic cleanup
- [ ] Add comprehensive tests for session operations
- [ ] Document session storage architecture decision
- [ ] Add session storage to `AppConfig`
- [ ] Plan future migration path to Redis/persistent storage

**Estimated Effort:** 1-2 days

---

## 📋 Implementation Roadmap

### Week 1: Stability & Security Foundation (Days 1-5)

#### Days 1-2: Error Handling Cleanup

**Goal:** Eliminate all panic-able code paths

- [ ] **Day 1 Morning:** Audit and list all `unwrap()` calls
- [ ] **Day 1 Afternoon:** Fix `src/lib.rs` unwrap calls (10+ instances)
- [ ] **Day 2 Morning:** Fix `src/js_engine.rs` unwrap calls (6+ instances)
- [ ] **Day 2 Afternoon:** Fix failing test `test_register_web_stream_invalid_path`
- [ ] **Day 2 Evening:** Add timeout enforcement for JS execution

**Deliverable:** Zero `unwrap()` calls in production paths, 100% test pass rate

#### Days 3-4: Security Integration

**Goal:** Connect security framework to execution paths

- [ ] **Day 3 Morning:** Remove `setup_global_functions()` dead code
- [ ] **Day 3 Afternoon:** Integrate `SecureOperations` with repository layer (5 TODOs)
- [ ] **Day 4 Morning:** Complete async handlers in `secure_globals.rs` (8 TODOs)
- [ ] **Day 4 Afternoon:** Connect audit logging to security operations (5 TODOs)
- [ ] **Day 4 Evening:** Add security integration tests

**Deliverable:** All 18 security TODOs completed or documented, security actively enforced

#### Day 5: Testing Infrastructure

**Goal:** Achieve 100% test pass rate with comprehensive coverage

- [ ] **Morning:** Create `tests/security_integration.rs` with 10+ tests
- [ ] **Afternoon:** Add missing tests for `graphql.rs` module
- [ ] **Evening:** Run coverage report, identify gaps, add tests for <80% areas

**Deliverable:** 100% test pass rate, security enforcement validated

---

### Week 2: Configuration & Preparation (Days 6-10)

#### Days 6-7: Configuration Enhancement

**Goal:** Support authentication requirements

- [ ] **Day 6 Morning:** Add `AuthConfig` struct to `src/config.rs`
- [ ] **Day 6 Afternoon:** Create `.env.example` with all required variables
- [ ] **Day 6 Evening:** Implement secrets validation (length, presence)
- [ ] **Day 7 Morning:** Add environment-specific auth configs (dev/staging/prod)
- [ ] **Day 7 Afternoon:** Update `docs/CONFIGURATION.md` with auth config
- [ ] **Day 7 Evening:** Add config validation tests

**Deliverable:** Complete auth configuration support with secrets management

#### Day 8: Code Quality

**Goal:** Zero compiler warnings, clean codebase

- [ ] **Morning:** Run `cargo fix` and `cargo fmt`
- [ ] **Afternoon:** Run `cargo clippy` and fix all warnings
- [ ] **Evening:** Set up pre-commit hooks and CI checks

**Deliverable:** Clean build with zero warnings

#### Days 9-10: Session Storage Foundation

**Goal:** Session CRUD operations working

- [ ] **Day 9 Morning:** Create `src/session.rs` with structs
- [ ] **Day 9 Afternoon:** Implement in-memory session store
- [ ] **Day 9 Evening:** Add session tests
- [ ] **Day 10 Morning:** Implement cleanup background task
- [ ] **Day 10 Afternoon:** Integration testing with session store
- [ ] **Day 10 Evening:** Document session architecture

**Deliverable:** Working session storage with automatic cleanup

---

### Week 3: Polish & Preparation (Days 11-12)

#### Day 11: Development Workflow

**Goal:** Improved developer experience

- [ ] **Morning:** Install dev tools (cargo-watch, nextest, llvm-cov)
- [ ] **Afternoon:** Create `Makefile` and test all commands
- [ ] **Evening:** Create Docker setup and test

**Deliverable:** Complete development workflow tools

#### Day 12: Final Verification

**Goal:** Ready for authentication development

- [ ] **Morning:** Run full test suite, verify 100% pass
- [ ] **Afternoon:** Verify all success metrics (see below)
- [ ] **Evening:** Review and update documentation

**Deliverable:** ✅ READY FOR AUTHENTICATION DEVELOPMENT

---

## 📊 Success Metrics (Gate for Authentication Development)

Before starting authentication implementation, verify ALL of these:

### Code Quality

- [ ] **0 compiler warnings** in `cargo build --release`
- [ ] **0 clippy warnings** with `cargo clippy -- -D warnings`
- [ ] **0 `unwrap()` calls** in production code paths
- [ ] **0 dead code warnings**

### Testing

- [ ] **100% test pass rate** (currently 125/126 = 99.2%)
- [ ] **Security integration tests** passing (minimum 10 tests)
- [ ] **Coverage report** generated and >80% for critical modules
- [ ] **All error paths** tested

### Security

- [ ] **All 18 security TODOs** resolved or documented as deferred
- [ ] **SecureOperations** integrated with all execution paths
- [ ] **UserContext** validated on every request
- [ ] **Audit logging** active for security events
- [ ] **Legacy security code** removed

### Configuration

- [ ] **AuthConfig** added to config system
- [ ] **Secrets management** implemented (from env vars)
- [ ] **.env.example** created and documented
- [ ] **Environment-specific** configs tested
- [ ] **Validation** prevents missing/weak secrets

### Session Storage

- [ ] **Session CRUD** operations working
- [ ] **Session expiration** implemented and tested
- [ ] **Automatic cleanup** background task running
- [ ] **Session tests** comprehensive and passing
- [ ] **Migration path** to Redis documented

### Documentation

- [ ] **CONFIGURATION.md** updated with auth requirements
- [ ] **local-development.md** updated with new workflow
- [ ] **Architecture decisions** documented
- [ ] **Security model** documented

### Development Workflow

- [ ] **Makefile** with common commands
- [ ] **Docker setup** tested and working
- [ ] **Auto-reload** working with cargo-watch
- [ ] **Pre-commit hooks** installed and active

---

## 🎓 Key Recommendations

### 1. **Don't Skip Error Handling**

This is THE most critical item. One panic during authentication = session loss for all users.

**Verification:**

```bash
# Must return zero results:
grep -r "unwrap()" src/ | grep -v "test" | grep -v "expect_test"
grep -r "expect(" src/ | grep -v "test" | grep -v "Valid regex"
```

### 2. **Security First**

Your security framework exists but isn't fully integrated. Connect it before adding auth.

**Verification:**

```bash
# All these should have implementations:
grep -r "TODO" src/security/
# Should return 0 or only deferred items
```

### 3. **Test Everything**

Authentication is complex. You need bulletproof tests to catch edge cases.

**Verification:**

```bash
cargo test
# Must show: test result: ok. 126 passed; 0 failed
```

### 4. **Start Simple**

Use in-memory sessions first, add persistence later once auth flow works.

### 5. **Document Decisions**

As you build, document why you chose certain approaches.

---

## 🚨 Blocking Issues

The following MUST be resolved before authentication development:

1. **Fix failing test** `test_register_web_stream_invalid_path`
2. **Remove all `unwrap()` calls** from production paths
3. **Integrate `SecureOperations`** with actual execution
4. **Implement session storage** with CRUD operations
5. **Add secrets management** from environment variables

---

## 📞 Getting Help

If stuck on any item:

1. Review the detailed implementation in this document
2. Check existing tests for similar patterns
3. Review DEVELOPMENT.md for coding standards
4. Consult SECURITY_TODO.md for security-specific guidance

---

## 🎯 Next Steps After Completion

Once all success metrics are met:

1. Review AUTH_TODO.md for authentication implementation plan
2. Start with Phase 1: Core Infrastructure (authentication module structure)
3. Implement OAuth2 provider integration
4. Build authentication middleware
5. Add JavaScript session context APIs

---

**Document Version:** 1.0  
**Last Updated:** October 11, 2025  
**Status:** ACTIVE - Must complete before authentication development
