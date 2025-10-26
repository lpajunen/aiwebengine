# Testing Guidelines

**Last Updated:** October 24, 2025

Comprehensive testing guidelines for aiwebengine contributors.

---

## Overview

Testing is **mandatory** for all contributions to aiwebengine. This guide covers what to test, how to test, and how much testing is enough.

**Target Audience:** All contributors  
**Prerequisites:** Basic Rust and testing knowledge

---

## Testing Philosophy

### Core Principles

1. **Test First** - Write tests before or during implementation
2. **Comprehensive Coverage** - Test happy paths AND error cases
3. **Fast Feedback** - Tests should run quickly
4. **Reliable** - Tests should not be flaky
5. **Readable** - Tests are documentation

### Why We Test

- ✅ Catch bugs before they reach production
- ✅ Enable confident refactoring
- ✅ Document expected behavior
- ✅ Prevent regressions
- ✅ Improve code design

---

## Testing Pyramid

We follow the testing pyramid approach:

```
       /\
      /  \     E2E Tests (5%)
     /----\    Slow, Expensive, High-Level
    /      \
   /--------\  Integration Tests (25%)
  /          \ Medium Speed, Module Interactions
 /------------\
/              \ Unit Tests (70%)
\--------------/ Fast, Cheap, Isolated
```

### Unit Tests (70%)

**Focus:** Individual functions and small components

**Characteristics:**

- Fast (<1ms per test)
- Isolated (no external dependencies)
- Numerous (hundreds or thousands)

**Example:**

```rust
#[test]
fn test_request_id_generation_creates_unique_ids() {
    let id1 = generate_request_id();
    let id2 = generate_request_id();

    assert_ne!(id1, id2);
    assert!(id1.starts_with("req_"));
}
```

### Integration Tests (25%)

**Focus:** Module interactions and workflows

**Characteristics:**

- Medium speed (10-100ms per test)
- Multiple components working together
- Realistic scenarios

**Example:**

```rust
#[tokio::test]
async fn test_authentication_flow_with_valid_credentials() {
    let app = create_test_app().await;

    // Login
    let login_response = app
        .post("/auth/login")
        .json(&json!({"username": "test", "password": "pass"}))
        .send()
        .await;

    assert_eq!(login_response.status(), 200);

    // Extract session token
    let token = login_response.cookie("session_token").unwrap();

    // Access protected resource
    let protected_response = app
        .get("/protected")
        .cookie(token)
        .send()
        .await;

    assert_eq!(protected_response.status(), 200);
}
```

### End-to-End Tests (5%)

**Focus:** Complete system behavior

**Characteristics:**

- Slow (100ms-1s+ per test)
- Full system with all components
- Critical user journeys

**Example:**

```rust
#[tokio::test]
async fn test_complete_user_journey_from_registration_to_api_call() {
    // Start server, register user, authenticate, make API call
    // Full system integration
}
```

---

## Coverage Requirements

### Minimum Coverage

**For new code:**

- > 90% line coverage
- > 95% function coverage
- 100% critical path coverage

**For overall codebase:**

- > 80% line coverage
- > 85% function coverage

### How to Measure

```bash
# Install coverage tool
cargo install cargo-llvm-cov

# Generate coverage report
cargo llvm-cov --all-features --html

# Open report
open target/llvm-cov/html/index.html
```

### What Must Be Tested

✅ **Always test:**

- Public APIs
- Error handling paths
- Edge cases and boundaries
- Security-critical code
- Data transformations
- State management

⚠️ **Can skip:**

- Generated code
- Third-party dependencies
- Trivial getters/setters
- Test utilities themselves

---

## Unit Testing Best Practices

### Test Naming

Use descriptive names that explain the scenario:

```rust
// Pattern: test_{function}_{scenario}_{expected_outcome}

#[test]
fn test_validate_email_with_valid_email_returns_ok() { }

#[test]
fn test_validate_email_with_invalid_format_returns_error() { }

#[test]
fn test_validate_email_with_empty_string_returns_error() { }
```

### Test Structure

Use the Arrange-Act-Assert pattern:

```rust
#[test]
fn test_session_manager_creates_valid_session() {
    // Arrange - Set up test data and state
    let config = SessionConfig {
        timeout_secs: 3600,
        secret: "test_secret".to_string(),
    };
    let manager = SessionManager::new(config);

    // Act - Execute the behavior being tested
    let result = manager.create_session("user123");

    // Assert - Verify the outcome
    assert!(result.is_ok());
    let session = result.unwrap();
    assert_eq!(session.user_id, "user123");
    assert!(session.expires_at > Utc::now());
    assert!(!session.token.is_empty());
}
```

### Testing Error Cases

**Every error variant must be tested:**

```rust
#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("Empty input")]
    EmptyInput,

    #[error("Too long: {length}")]
    TooLong { length: usize },

    #[error("Invalid format")]
    InvalidFormat,
}

// Test EACH error variant
#[test]
fn test_validate_with_empty_input_returns_empty_input_error() {
    let result = validate("");
    assert!(matches!(result, Err(ValidationError::EmptyInput)));
}

#[test]
fn test_validate_with_long_input_returns_too_long_error() {
    let long_input = "x".repeat(1001);
    let result = validate(&long_input);

    match result {
        Err(ValidationError::TooLong { length }) => {
            assert_eq!(length, 1001);
        }
        _ => panic!("Expected TooLong error"),
    }
}

#[test]
fn test_validate_with_invalid_format_returns_invalid_format_error() {
    let result = validate("not-valid-format");
    assert!(matches!(result, Err(ValidationError::InvalidFormat)));
}
```

### Testing Async Functions

```rust
#[tokio::test]
async fn test_async_function() {
    let result = async_function().await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_async_timeout() {
    let result = tokio::time::timeout(
        Duration::from_millis(100),
        slow_async_function()
    ).await;

    assert!(result.is_err()); // Should timeout
}
```

### Parameterized Tests

Test multiple scenarios efficiently:

```rust
#[test]
fn test_email_validation_with_various_formats() {
    let test_cases = vec![
        ("user@example.com", true),
        ("invalid.email", false),
        ("@example.com", false),
        ("user@", false),
        ("", false),
        ("user+tag@example.com", true),
    ];

    for (email, should_be_valid) in test_cases {
        let result = validate_email(email);
        assert_eq!(
            result.is_ok(),
            should_be_valid,
            "Failed for email: {}", email
        );
    }
}
```

---

## Integration Testing

### Test Organization

Place integration tests in `tests/` directory:

```
tests/
├── common/
│   ├── mod.rs       // Shared test utilities
│   └── fixtures.rs  // Test data
├── auth_tests.rs    // Authentication integration tests
├── graphql_tests.rs // GraphQL integration tests
└── api_tests.rs     // API integration tests
```

### Test Helpers

Create reusable test utilities:

```rust
// tests/common/mod.rs
pub async fn create_test_app() -> TestApp {
    let config = test_config();
    let app = create_app(config).await;
    TestApp::new(app)
}

pub fn test_config() -> AppConfig {
    AppConfig {
        server: ServerConfig {
            port: 0, // Random port
            host: "127.0.0.1".to_string(),
        },
        auth: Some(AuthConfig {
            jwt_secret: "test_secret".to_string(),
            session_timeout_secs: 3600,
        }),
    }
}

pub struct TestApp {
    app: Router,
    client: TestClient,
}

impl TestApp {
    pub fn request(&self) -> RequestBuilder {
        self.client.request()
    }
}
```

### Database Testing

If using a database:

```rust
// Use a test database or in-memory database
#[tokio::test]
async fn test_user_repository_crud() {
    let db = create_test_database().await;
    let repo = UserRepository::new(db);

    // Create
    let user = repo.create_user("test@example.com").await.unwrap();

    // Read
    let found = repo.find_user(&user.id).await.unwrap();
    assert_eq!(found.email, "test@example.com");

    // Update
    repo.update_user(&user.id, "new@example.com").await.unwrap();
    let updated = repo.find_user(&user.id).await.unwrap();
    assert_eq!(updated.email, "new@example.com");

    // Delete
    repo.delete_user(&user.id).await.unwrap();
    let deleted = repo.find_user(&user.id).await;
    assert!(deleted.is_none());

    cleanup_test_database(db).await;
}
```

### Mocking External Dependencies

```rust
// Use mockall or manual mocks
use mockall::predicate::*;
use mockall::*;

#[automock]
#[async_trait]
pub trait HttpClient: Send + Sync {
    async fn get(&self, url: &str) -> Result<Response, HttpError>;
}

#[tokio::test]
async fn test_service_with_mocked_http_client() {
    let mut mock_client = MockHttpClient::new();

    mock_client
        .expect_get()
        .with(eq("https://api.example.com/data"))
        .times(1)
        .returning(|_| Ok(Response {
            status: 200,
            body: "test data".to_string(),
        }));

    let service = MyService::new(Box::new(mock_client));
    let result = service.fetch_data().await;

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "test data");
}
```

---

## Security Testing

### Input Validation

Test all inputs are validated:

```rust
#[test]
fn test_script_registration_rejects_path_traversal() {
    let malicious_paths = vec![
        "../etc/passwd",
        "../../secret",
        "./../../config",
    ];

    for path in malicious_paths {
        let result = register_script(path, "content");
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), Error::SecurityViolation));
    }
}

#[test]
fn test_script_content_rejects_dangerous_patterns() {
    let dangerous_patterns = vec![
        "eval('malicious')",
        "__proto__.pollute = true",
        "require('fs')",
    ];

    for pattern in dangerous_patterns {
        let result = validate_script_content(pattern);
        assert!(result.is_err());
    }
}
```

### Authentication Testing

```rust
#[tokio::test]
async fn test_protected_endpoint_requires_authentication() {
    let app = create_test_app().await;

    // Without auth
    let response = app.get("/protected").send().await;
    assert_eq!(response.status(), 401);

    // With invalid token
    let response = app
        .get("/protected")
        .header("Authorization", "Bearer invalid")
        .send()
        .await;
    assert_eq!(response.status(), 401);

    // With valid token
    let token = create_valid_token();
    let response = app
        .get("/protected")
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await;
    assert_eq!(response.status(), 200);
}
```

---

## Performance Testing

### Benchmark Tests

```rust
#[cfg(test)]
mod benchmarks {
    use super::*;
    use std::time::Instant;

    #[test]
    fn benchmark_script_execution() {
        let script = "function test() { return 42; }";
        let iterations = 1000;

        let start = Instant::now();
        for _ in 0..iterations {
            let _ = execute_script(script);
        }
        let duration = start.elapsed();

        let avg_ms = duration.as_millis() / iterations;
        println!("Average execution time: {}ms", avg_ms);

        // Assert performance requirement
        assert!(avg_ms < 10, "Script execution too slow: {}ms", avg_ms);
    }
}
```

### Load Testing

```rust
#[tokio::test]
async fn test_concurrent_requests() {
    let app = create_test_app().await;
    let concurrent_requests = 100;

    let mut handles = vec![];
    for i in 0..concurrent_requests {
        let app_clone = app.clone();
        let handle = tokio::spawn(async move {
            app_clone
                .get(&format!("/api/test?id={}", i))
                .send()
                .await
        });
        handles.push(handle);
    }

    // Wait for all requests
    let results = futures::future::join_all(handles).await;

    // All should succeed
    for result in results {
        let response = result.unwrap();
        assert_eq!(response.status(), 200);
    }
}
```

---

## Test Data Management

### Fixtures

```rust
// tests/common/fixtures.rs
pub struct TestData {
    pub users: Vec<User>,
    pub sessions: Vec<Session>,
}

impl TestData {
    pub fn new() -> Self {
        Self {
            users: vec![
                User {
                    id: "user1".to_string(),
                    email: "user1@example.com".to_string(),
                    name: "Test User 1".to_string(),
                },
                User {
                    id: "user2".to_string(),
                    email: "user2@example.com".to_string(),
                    name: "Test User 2".to_string(),
                },
            ],
            sessions: vec![],
        }
    }

    pub fn user_by_email(&self, email: &str) -> Option<&User> {
        self.users.iter().find(|u| u.email == email)
    }
}
```

### Builders

```rust
pub struct UserBuilder {
    id: String,
    email: String,
    name: String,
    roles: Vec<String>,
}

impl UserBuilder {
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            email: "test@example.com".to_string(),
            name: "Test User".to_string(),
            roles: vec!["user".to_string()],
        }
    }

    pub fn with_email(mut self, email: &str) -> Self {
        self.email = email.to_string();
        self
    }

    pub fn with_role(mut self, role: &str) -> Self {
        self.roles.push(role.to_string());
        self
    }

    pub fn build(self) -> User {
        User {
            id: self.id,
            email: self.email,
            name: self.name,
            roles: self.roles,
        }
    }
}

// Usage in tests
#[test]
fn test_with_admin_user() {
    let admin = UserBuilder::new()
        .with_email("admin@example.com")
        .with_role("admin")
        .build();

    assert!(admin.roles.contains(&"admin".to_string()));
}
```

---

## Common Testing Patterns

### Testing Mutex/Lock Usage

```rust
#[test]
fn test_concurrent_access_to_shared_state() {
    let state = Arc::new(Mutex::new(0));
    let mut handles = vec![];

    for _ in 0..10 {
        let state_clone = state.clone();
        let handle = std::thread::spawn(move || {
            let mut guard = state_clone.lock().unwrap();
            *guard += 1;
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().unwrap();
    }

    assert_eq!(*state.lock().unwrap(), 10);
}
```

### Testing Error Propagation

```rust
#[test]
fn test_error_propagation_through_layers() {
    // Service layer error should propagate to handler
    let error = DatabaseError::NotFound;
    let service_result = service_function(error);

    assert!(matches!(
        service_result,
        Err(ServiceError::Database(DatabaseError::NotFound))
    ));
}
```

### Testing Cleanup

```rust
#[test]
fn test_resource_cleanup_on_drop() {
    let temp_file = create_temp_file();
    let path = temp_file.path().to_path_buf();

    assert!(path.exists());

    drop(temp_file);

    assert!(!path.exists(), "File should be deleted on drop");
}
```

---

## Running Tests

### Basic Commands

```bash
# Run all tests
cargo test

# Run specific test
cargo test test_name

# Run tests in specific file
cargo test --test integration_tests

# Run with output
cargo test -- --nocapture

# Run with specific number of threads
cargo test -- --test-threads=1
```

### With Coverage

```bash
# Generate HTML coverage report
cargo llvm-cov --all-features --html

# Generate coverage and open report
cargo llvm-cov --all-features --html --open

# Generate coverage for specific package
cargo llvm-cov -p aiwebengine --html
```

### In CI/CD

```yaml
# .github/workflows/test.yml
name: Tests

on: [push, pull_request]

jobs:
  test:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v2
      - uses: actions-rs/toolchain@v1
        with:
          toolchain: stable
      - name: Run tests
        run: cargo test --all-features
      - name: Generate coverage
        run: |
          cargo install cargo-llvm-cov
          cargo llvm-cov --all-features --lcov --output-path lcov.info
      - name: Upload coverage
        uses: codecov/codecov-action@v2
        with:
          files: lcov.info
```

---

## Debugging Failed Tests

### Add Debug Output

```rust
#[test]
fn test_with_debug_output() {
    let result = complex_function();

    // Debug output only shows with --nocapture
    println!("Result: {:?}", result);

    assert!(result.is_ok());
}
```

### Use Test Attributes

```rust
// Ignore flaky tests temporarily
#[test]
#[ignore]
fn test_sometimes_fails() {
    // Fix this test later
}

// Run ignored tests
// cargo test -- --ignored

// Test should panic
#[test]
#[should_panic(expected = "Invalid input")]
fn test_panics_on_invalid_input() {
    validate_input("invalid");
}
```

---

## Testing Checklist

Before submitting code:

### Unit Tests

- [ ] All public functions tested
- [ ] All error paths tested
- [ ] Edge cases covered
- [ ] > 90% coverage achieved

### Integration Tests

- [ ] Main workflows tested
- [ ] Module interactions validated
- [ ] External dependencies mocked

### Quality

- [ ] All tests pass
- [ ] Tests are not flaky
- [ ] Tests run quickly (<5 min total)
- [ ] Test names are descriptive

### Documentation

- [ ] Complex test scenarios explained
- [ ] Test data sources documented
- [ ] Setup/teardown requirements noted

---

## Related Resources

- [DEVELOPMENT.md](../DEVELOPMENT.md) - Development guidelines
- [adding-new-features.md](./adding-new-features.md) - Feature implementation
- [security-checklist.md](./security-checklist.md) - Security testing
- [Rust Testing Book](https://doc.rust-lang.org/book/ch11-00-testing.html)

---

_Testing is not optional. It's how we ensure aiwebengine works reliably for everyone._
