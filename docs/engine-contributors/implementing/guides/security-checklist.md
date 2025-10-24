# Security Checklist

**Last Updated:** October 24, 2025

Security review checklist for all contributions to aiwebengine.

---

## Overview

Every contribution to aiwebengine must pass security review. This checklist helps identify and prevent common security vulnerabilities.

**Who should use this:**

- Contributors before submitting PRs
- Code reviewers during review
- Security auditors

**When to use:**

- Before submitting any PR
- During code review
- When implementing security-sensitive features

---

## Quick Security Checklist

Before submitting your PR, verify:

- [ ] All user inputs are validated
- [ ] No SQL injection vulnerabilities (when DB added)
- [ ] No XSS vulnerabilities in outputs
- [ ] Authentication properly enforced
- [ ] Authorization checks in place
- [ ] Secrets not hardcoded or logged
- [ ] Error messages don't leak sensitive info
- [ ] Rate limiting on user-facing endpoints
- [ ] CSRF protection where needed
- [ ] Input size limits enforced

---

## Input Validation

### Rule: Never Trust User Input

**✅ DO:**

```rust
pub fn register_script(path: &str, content: &str) -> Result<(), SecurityError> {
    // Validate path
    if path.contains("..") {
        return Err(SecurityError::PathTraversal);
    }
    
    if !path.starts_with('/') {
        return Err(SecurityError::InvalidPath);
    }
    
    if path.len() > MAX_PATH_LENGTH {
        return Err(SecurityError::PathTooLong);
    }
    
    // Validate content
    if content.len() > MAX_SCRIPT_SIZE {
        return Err(SecurityError::ScriptTooLarge);
    }
    
    // Check for dangerous patterns
    validate_script_content(content)?;
    
    Ok(())
}
```

**❌ DON'T:**

```rust
pub fn register_script(path: &str, content: &str) -> Result<(), Error> {
    // No validation - vulnerable!
    storage.save(path, content)
}
```

### Validation Checklist

- [ ] **Path Validation**
  - [ ] No path traversal (`../`, `..\\`)
  - [ ] No absolute paths outside allowed directories
  - [ ] Length limits enforced
  - [ ] Only allowed characters

- [ ] **String Input Validation**
  - [ ] Length limits checked
  - [ ] Character set restrictions
  - [ ] No dangerous patterns (eval, __proto__)
  - [ ] Proper encoding/escaping

- [ ] **Numeric Input Validation**
  - [ ] Range checks
  - [ ] Type validation
  - [ ] Overflow prevention

- [ ] **Email/URL Validation**
  - [ ] Format validation
  - [ ] Domain validation if needed
  - [ ] Length limits

---

## Authentication & Authorization

### Authentication Enforcement

**✅ DO:**

```rust
pub async fn protected_handler(
    State(auth): State<Arc<AuthMiddleware>>,
    Extension(user): Extension<UserContext>,
    request: Request<Body>,
) -> Result<Response, Error> {
    // User context automatically injected by middleware
    // Only authenticated users reach this handler
    
    process_authenticated_request(user, request).await
}
```

**❌ DON'T:**

```rust
pub async fn handler(request: Request<Body>) -> Result<Response, Error> {
    // No authentication check - anyone can access!
    process_request(request).await
}
```

### Authorization Checks

**✅ DO:**

```rust
pub fn delete_script(user: &UserContext, script_id: &str) -> Result<(), Error> {
    // Check user has permission
    if !user.has_capability(Capability::WriteScripts) {
        return Err(Error::PermissionDenied);
    }
    
    // Additional check: user owns the script
    let script = repository.get_script(script_id)?;
    if script.owner_id != user.id && !user.is_admin() {
        return Err(Error::PermissionDenied);
    }
    
    repository.delete_script(script_id)
}
```

**❌ DON'T:**

```rust
pub fn delete_script(script_id: &str) -> Result<(), Error> {
    // No permission check - anyone can delete!
    repository.delete_script(script_id)
}
```

### Authentication Checklist

- [ ] **Authentication Required**
  - [ ] Protected endpoints enforce authentication
  - [ ] Session validation is secure
  - [ ] Token expiration is checked
  - [ ] Invalid tokens are rejected

- [ ] **Authorization Enforced**
  - [ ] User permissions checked before operations
  - [ ] Resource ownership verified
  - [ ] Role-based access control implemented
  - [ ] Principle of least privilege followed

- [ ] **Session Security**
  - [ ] Session tokens are cryptographically secure
  - [ ] Sessions have timeout
  - [ ] Session fixation prevented
  - [ ] Concurrent session limits enforced

---

## Secrets Management

### Rule: Never Hardcode Secrets

**✅ DO:**

```rust
pub fn load_config() -> Result<AppConfig, ConfigError> {
    let jwt_secret = std::env::var("JWT_SECRET")
        .map_err(|_| ConfigError::MissingSecret("JWT_SECRET"))?;
    
    if jwt_secret.len() < 32 {
        return Err(ConfigError::WeakSecret("JWT_SECRET must be at least 32 characters"));
    }
    
    Ok(AppConfig {
        jwt_secret,
        // ...
    })
}
```

**❌ DON'T:**

```rust
pub const JWT_SECRET: &str = "my-secret-key"; // Never do this!

pub fn load_config() -> AppConfig {
    AppConfig {
        jwt_secret: JWT_SECRET.to_string(),
        // ...
    }
}
```

### Secrets Checklist

- [ ] **No Hardcoded Secrets**
  - [ ] No API keys in code
  - [ ] No passwords in code
  - [ ] No tokens in code
  - [ ] No encryption keys in code

- [ ] **Environment Variables**
  - [ ] Secrets loaded from environment
  - [ ] Fallbacks don't contain real secrets
  - [ ] Validation of secret format/length

- [ ] **Logging Safety**
  - [ ] Secrets never logged
  - [ ] Secrets not in error messages
  - [ ] Secrets not in debug output
  - [ ] Secrets not in stack traces

- [ ] **Storage**
  - [ ] Secrets encrypted at rest
  - [ ] Secrets not in version control
  - [ ] Secrets not in backups (or encrypted)

---

## Output Encoding / XSS Prevention

### HTML Output

**✅ DO:**

```rust
pub fn generate_html(user_input: &str) -> String {
    // Escape HTML special characters
    let escaped = html_escape::encode_text(user_input);
    format!("<div>{}</div>", escaped)
}
```

**❌ DON'T:**

```rust
pub fn generate_html(user_input: &str) -> String {
    // Vulnerable to XSS!
    format!("<div>{}</div>", user_input)
}
```

### JSON Output

**✅ DO:**

```rust
pub fn api_response(user_data: &str) -> String {
    // Use serde_json for proper escaping
    let response = json!({
        "data": user_data
    });
    serde_json::to_string(&response).unwrap()
}
```

### JavaScript Output

**✅ DO:**

```rust
pub fn generate_js(user_input: &str) -> String {
    // Proper JavaScript escaping
    let escaped = user_input
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!("var data = \"{}\";", escaped)
}
```

### Output Encoding Checklist

- [ ] **HTML Escaping**
  - [ ] User input in HTML is escaped
  - [ ] Attributes are quoted
  - [ ] Context-appropriate escaping

- [ ] **JavaScript Escaping**
  - [ ] User input in JS strings is escaped
  - [ ] No eval() of user input
  - [ ] JSON encoding for data

- [ ] **URL Encoding**
  - [ ] User input in URLs is encoded
  - [ ] Query parameters escaped
  - [ ] Redirect URLs validated

---

## SQL Injection Prevention (Future)

When database integration is added:

**✅ DO:**

```rust
pub async fn find_user(email: &str) -> Result<User, DbError> {
    // Use parameterized queries
    sqlx::query_as!(
        User,
        "SELECT * FROM users WHERE email = $1",
        email
    )
    .fetch_one(&pool)
    .await
}
```

**❌ DON'T:**

```rust
pub async fn find_user(email: &str) -> Result<User, DbError> {
    // Never concatenate SQL!
    let query = format!("SELECT * FROM users WHERE email = '{}'", email);
    sqlx::query(&query).fetch_one(&pool).await
}
```

### SQL Injection Checklist

- [ ] Parameterized queries used
- [ ] No string concatenation in queries
- [ ] ORM/query builder used correctly
- [ ] Input validation still performed

---

## CSRF Protection

### For State-Changing Operations

**✅ DO:**

```rust
pub async fn delete_script_handler(
    Extension(user): Extension<UserContext>,
    Path(script_id): Path<String>,
    headers: HeaderMap,
) -> Result<Response, Error> {
    // Verify CSRF token
    let csrf_token = headers
        .get("X-CSRF-Token")
        .and_then(|v| v.to_str().ok())
        .ok_or(Error::MissingCsrfToken)?;
    
    csrf_protection.validate_token(&csrf_token, &user.session_id)?;
    
    // Proceed with deletion
    delete_script(&user, &script_id)
}
```

### CSRF Checklist

- [ ] **POST/PUT/DELETE Protected**
  - [ ] State-changing operations require CSRF token
  - [ ] Token tied to user session
  - [ ] Token validated before action

- [ ] **GET Requests Safe**
  - [ ] GET requests don't change state
  - [ ] No side effects in GET handlers

---

## Rate Limiting

### Prevent Abuse

**✅ DO:**

```rust
pub async fn login_handler(
    State(rate_limiter): State<Arc<RateLimiter>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    body: Json<LoginRequest>,
) -> Result<Response, Error> {
    // Check rate limit
    if !rate_limiter.check_rate_limit(&addr.ip().to_string()).await {
        return Err(Error::RateLimitExceeded);
    }
    
    // Process login
    authenticate(body.username, body.password).await
}
```

### Rate Limiting Checklist

- [ ] **Authentication Endpoints**
  - [ ] Login rate limited per IP
  - [ ] Failed attempts tracked
  - [ ] Progressive delays or lockout

- [ ] **API Endpoints**
  - [ ] Per-user rate limits
  - [ ] Per-IP rate limits
  - [ ] Burst protection

- [ ] **Resource-Intensive Operations**
  - [ ] Script execution rate limited
  - [ ] File uploads rate limited
  - [ ] Large queries rate limited

---

## Error Handling Security

### Don't Leak Information

**✅ DO:**

```rust
pub fn authenticate(username: &str, password: &str) -> Result<User, AuthError> {
    let user = repository.find_user_by_username(username)
        .map_err(|_| AuthError::InvalidCredentials)?; // Generic error
    
    if !verify_password(password, &user.password_hash) {
        return Err(AuthError::InvalidCredentials); // Same error
    }
    
    Ok(user)
}

impl Display for AuthError {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            Self::InvalidCredentials => write!(f, "Invalid username or password"),
            // Don't reveal which field was wrong!
        }
    }
}
```

**❌ DON'T:**

```rust
pub fn authenticate(username: &str, password: &str) -> Result<User, AuthError> {
    let user = repository.find_user_by_username(username)
        .ok_or(AuthError::UserNotFound)?; // Leaks existence!
    
    if !verify_password(password, &user.password_hash) {
        return Err(AuthError::WrongPassword); // Different error!
    }
    
    Ok(user)
}
```

### Error Handling Checklist

- [ ] **Generic Error Messages**
  - [ ] Authentication errors are generic
  - [ ] Don't reveal system internals
  - [ ] Don't leak user existence

- [ ] **Logging vs User Messages**
  - [ ] Detailed errors logged internally
  - [ ] Generic errors shown to users
  - [ ] Stack traces not in responses

- [ ] **No Information Disclosure**
  - [ ] File paths not revealed
  - [ ] Database errors sanitized
  - [ ] Configuration not exposed

---

## Cryptography

### Use Standard Libraries

**✅ DO:**

```rust
use argon2::{Argon2, PasswordHasher};
use rand::RngCore;

pub fn hash_password(password: &str) -> Result<String, CryptoError> {
    let mut salt = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut salt);
    
    let argon2 = Argon2::default();
    let hash = argon2
        .hash_password(password.as_bytes(), &salt)
        .map_err(|e| CryptoError::HashingFailed(e.to_string()))?;
    
    Ok(hash.to_string())
}
```

**❌ DON'T:**

```rust
pub fn hash_password(password: &str) -> String {
    // Never roll your own crypto!
    let mut hasher = Sha256::new();
    hasher.update(password);
    format!("{:x}", hasher.finalize())
}
```

### Cryptography Checklist

- [ ] **Use Standard Libraries**
  - [ ] No custom crypto implementations
  - [ ] Use argon2/bcrypt for passwords
  - [ ] Use proper random number generation

- [ ] **Secure Defaults**
  - [ ] Strong algorithms (AES-256, etc.)
  - [ ] Proper key lengths
  - [ ] Secure random for IVs/salts

- [ ] **Key Management**
  - [ ] Keys not hardcoded
  - [ ] Keys rotated regularly
  - [ ] Keys stored securely

---

## Dependency Security

### Keep Dependencies Updated

```bash
# Check for vulnerabilities
cargo audit

# Update dependencies
cargo update
```

### Dependency Checklist

- [ ] **Regular Updates**
  - [ ] Dependencies reviewed monthly
  - [ ] Security advisories monitored
  - [ ] Critical updates applied quickly

- [ ] **Minimal Dependencies**
  - [ ] Only necessary dependencies
  - [ ] Trusted sources
  - [ ] Licenses reviewed

- [ ] **Vulnerability Scanning**
  - [ ] cargo-audit in CI/CD
  - [ ] Known vulnerabilities addressed
  - [ ] Dependabot or similar enabled

---

## Security Testing

### Test Security Controls

```rust
#[test]
fn test_path_traversal_prevention() {
    let malicious_paths = vec![
        "../etc/passwd",
        "../../config",
        "./../../secrets",
    ];
    
    for path in malicious_paths {
        let result = validate_path(path);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), SecurityError::PathTraversal));
    }
}

#[test]
fn test_xss_prevention() {
    let xss_payloads = vec![
        "<script>alert('xss')</script>",
        "<img src=x onerror=alert('xss')>",
        "javascript:alert('xss')",
    ];
    
    for payload in xss_payloads {
        let output = generate_html_output(payload);
        assert!(!output.contains("<script"));
        assert!(!output.contains("onerror="));
        assert!(!output.contains("javascript:"));
    }
}

#[tokio::test]
async fn test_authentication_required() {
    let app = create_test_app().await;
    
    // Without auth
    let response = app.get("/protected").send().await;
    assert_eq!(response.status(), 401);
    
    // With auth
    let token = create_valid_token();
    let response = app
        .get("/protected")
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await;
    assert_eq!(response.status(), 200);
}
```

### Security Testing Checklist

- [ ] **Input Validation Tests**
  - [ ] Path traversal tests
  - [ ] SQL injection tests (when DB added)
  - [ ] XSS payload tests
  - [ ] Oversized input tests

- [ ] **Authentication Tests**
  - [ ] Missing auth tests
  - [ ] Invalid token tests
  - [ ] Expired token tests

- [ ] **Authorization Tests**
  - [ ] Permission denial tests
  - [ ] Privilege escalation tests
  - [ ] Resource access tests

---

## Complete Security Review Checklist

Use this for final review before merge:

### Input Security

- [ ] All user inputs validated
- [ ] Path traversal prevented
- [ ] File upload restrictions enforced
- [ ] Size limits on all inputs
- [ ] Dangerous patterns detected

### Authentication & Authorization

- [ ] Protected endpoints require auth
- [ ] Session management secure
- [ ] Tokens cryptographically secure
- [ ] Authorization checked before operations
- [ ] Least privilege enforced

### Data Protection

- [ ] Secrets in environment variables
- [ ] Passwords hashed with strong algorithm
- [ ] Sensitive data encrypted at rest
- [ ] Secure transmission (HTTPS enforced)
- [ ] No secrets in logs

### Output Security

- [ ] HTML output escaped
- [ ] JSON properly encoded
- [ ] Error messages generic
- [ ] No information disclosure
- [ ] Headers properly set

### Application Security

- [ ] CSRF protection on state changes
- [ ] Rate limiting on sensitive endpoints
- [ ] Dependencies scanned for vulnerabilities
- [ ] Security headers configured
- [ ] Timeouts on operations

### Testing

- [ ] Security tests written
- [ ] Attack scenarios tested
- [ ] Fuzzing considered
- [ ] Penetration testing done

---

## Resources

- [OWASP Top 10](https://owasp.org/www-project-top-ten/)
- [Rust Security Best Practices](https://anssi-fr.github.io/rust-guide/)
- [DEVELOPMENT.md](../DEVELOPMENT.md) - Security section
- [improvements/security-hardening.md](../improvements/security-hardening.md)

---

_Security is everyone's responsibility. When in doubt, ask for a security review._
