# aiwebengine Development Guidelines

This document outlines the development practices, coding standards, and quality guidelines for contributing to the aiwebengine project. Following these guidelines ensures consistent, maintainable, and production-ready code.

## Table of Contents

- [Development Philosophy](#development-philosophy)
- [Code Quality Standards](#code-quality-standards)
- [Testing Strategy](#testing-strategy)
- [Error Handling](#error-handling)
- [Performance Guidelines](#performance-guidelines)
- [Security Considerations](#security-considerations)
- [Documentation Standards](#documentation-standards)
- [Contribution Workflow](#contribution-workflow)
- [Architecture Guidelines](#architecture-guidelines)
- [Production Readiness Checklist](#production-readiness-checklist)

## Development Philosophy

### Core Principles

1. **Safety First**: Rust's ownership system and type safety are our primary defenses against runtime errors
2. **Fail Fast**: Use comprehensive error handling and validation to catch issues early
3. **Test-Driven Quality**: Every feature should have comprehensive tests before merging
4. **Performance by Design**: Consider performance implications in all design decisions
5. **Security by Default**: Implement secure coding practices from the start
6. **Maintainability**: Write code that future developers (including yourself) can easily understand and modify

### Design Goals

- **Simplicity**: Keep the API surface clean and intuitive
- **Performance**: Maintain low memory usage and fast response times
- **Reliability**: Handle edge cases gracefully and provide clear error messages
- **Extensibility**: Design for future enhancements without breaking changes

## Code Quality Standards

### Rust Best Practices

#### Error Handling

✅ **DO**: Use `Result<T, E>` for fallible operations
```rust
pub fn process_script(script: &str) -> Result<String, ScriptError> {
    // Process script with proper error propagation
    Ok(result)
}
```

❌ **DON'T**: Use `unwrap()` or `expect()` in production code
```rust
// Avoid this in production
let result = risky_operation().unwrap();
```

✅ **DO**: Use `?` operator for error propagation
```rust
pub fn complex_operation() -> Result<Value, ProcessingError> {
    let step1 = first_step()?;
    let step2 = second_step(step1)?;
    Ok(step2)
}
```

#### Resource Management

✅ **DO**: Use RAII and proper lifetimes
```rust
pub struct SafeRepository {
    connection: Arc<Mutex<Connection>>,
}

impl SafeRepository {
    pub async fn execute_safely<T>(&self, operation: impl FnOnce(&Connection) -> Result<T, Error>) -> Result<T, Error> {
        let conn = self.connection.lock().map_err(|_| Error::LockPoisoned)?;
        operation(&*conn)
    }
}
```

#### Type Safety

✅ **DO**: Use strong typing and avoid stringly-typed APIs
```rust
#[derive(Debug, Clone)]
pub struct RequestId(String);

pub trait HasRequestId {
    fn request_id(&self) -> &str;
}
```

#### Async/Await Patterns

✅ **DO**: Use structured concurrency patterns
```rust
pub async fn process_concurrent_requests(requests: Vec<Request>) -> Result<Vec<Response>, ProcessingError> {
    let futures = requests.into_iter().map(process_single_request);
    let results = futures::future::try_join_all(futures).await?;
    Ok(results)
}
```

### Code Organization

#### Module Structure

```
src/
├── lib.rs              // Public API exports
├── main.rs             // Binary entry point
├── config.rs           // Configuration management
├── error.rs            // Error types and handling
├── middleware.rs       // HTTP middleware
├── js_engine.rs        // JavaScript execution
├── graphql.rs          // GraphQL schema and resolvers
├── repository.rs       // Data storage
└── bin/
    ├── server.rs       // Server binary
    └── deployer.rs     // Deployment tools
```

#### Function Organization

- **Single Responsibility**: Each function should do one thing well
- **Pure Functions**: Prefer pure functions when possible
- **Small Functions**: Keep functions under 50 lines when practical
- **Clear Names**: Use descriptive, verb-based names for functions

```rust
// Good: Clear, focused function
pub fn generate_request_id() -> String {
    let counter = REQUEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let timestamp = current_timestamp_millis();
    format!("req_{}_{}", timestamp, counter)
}

// Good: Clear error handling
pub fn validate_script_content(content: &str) -> Result<(), ValidationError> {
    if content.is_empty() {
        return Err(ValidationError::EmptyContent);
    }
    if content.len() > MAX_SCRIPT_SIZE {
        return Err(ValidationError::ContentTooLarge(content.len()));
    }
    Ok(())
}
```

## Testing Strategy

### Testing Pyramid

Our testing strategy follows a comprehensive pyramid approach:

#### 1. Unit Tests (Foundation - 70% of tests)

✅ **Requirements**:
- **Coverage Target**: >80% line coverage, >90% function coverage
- **Test Structure**: Use descriptive test names that explain behavior
- **Isolation**: Each test should be independent and repeatable
- **Edge Cases**: Test boundary conditions, error scenarios, and edge cases

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_generate_request_id_creates_unique_ids() {
        let id1 = generate_request_id();
        let id2 = generate_request_id();
        
        assert_ne!(id1, id2);
        assert!(id1.starts_with("req_"));
        assert!(id2.starts_with("req_"));
    }

    #[test]
    fn test_generate_request_id_format_consistency() {
        let id = generate_request_id();
        let parts: Vec<&str> = id.split('_').collect();
        
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[0], "req");
        
        // Timestamp should be parseable
        let timestamp: u128 = parts[1].parse().expect("Invalid timestamp format");
        assert!(timestamp > 0);
        
        // Counter should be parseable
        let _counter: u64 = parts[2].parse().expect("Invalid counter format");
    }

    #[test]
    fn test_error_response_builder_with_context() {
        let error = ErrorResponseBuilder::new(ErrorCode::ValidationError, "Invalid input")
            .context("field", json!("username"))
            .context("value", json!(null))
            .build();

        assert_eq!(error.error.context.len(), 2);
        assert_eq!(error.error.context["field"], json!("username"));
        assert_eq!(error.error.context["value"], json!(null));
    }
}
```

#### 2. Integration Tests (25% of tests)

✅ **Focus Areas**:
- HTTP endpoint behavior
- Database interactions
- JavaScript engine integration
- Cross-module communication

```rust
#[tokio::test]
async fn test_graphql_endpoint_with_registered_query() {
    let app = create_test_app().await;
    
    // Register a GraphQL query via JavaScript
    let script = r#"
        registerGraphQLQuery('user', 'User', 'id: String!', function(args) {
            return { id: args.id, name: 'Test User' };
        });
    "#;
    
    register_script(&app, "test-query", script).await;
    
    // Execute GraphQL query
    let query = r#"{ user(id: "123") { id name } }"#;
    let response = app
        .request()
        .method("POST")
        .uri("/graphql")
        .json(&json!({"query": query}))
        .send()
        .await;
    
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await;
    assert_eq!(body["data"]["user"]["id"], "123");
    assert_eq!(body["data"]["user"]["name"], "Test User");
}
```

#### 3. End-to-End Tests (5% of tests)

✅ **Scenarios**:
- Complete user workflows
- Performance under load
- Error recovery scenarios
- Security boundary testing

### Test Quality Guidelines

#### Test Naming Convention

```rust
// Pattern: test_{function_name}_{scenario}_{expected_outcome}
#[test]
fn test_execute_script_with_syntax_error_returns_error() { }

#[test]
fn test_extract_request_id_with_invalid_header_generates_new_id() { }

#[test]
fn test_error_response_builder_with_empty_context_omits_context_field() { }
```

#### Test Data Management

```rust
// Good: Use builders for complex test data
struct TestScriptBuilder {
    content: String,
    method: String,
    path: String,
}

impl TestScriptBuilder {
    fn new() -> Self {
        Self {
            content: "function handler() { return 'default'; }".to_string(),
            method: "GET".to_string(),
            path: "/test".to_string(),
        }
    }
    
    fn with_content(mut self, content: &str) -> Self {
        self.content = content.to_string();
        self
    }
    
    fn with_method(mut self, method: &str) -> Self {
        self.method = method.to_string();
        self
    }
    
    fn build(self) -> Script {
        Script {
            content: self.content,
            method: self.method,
            path: self.path,
        }
    }
}

#[test]
fn test_script_execution_with_custom_method() {
    let script = TestScriptBuilder::new()
        .with_method("POST")
        .with_content("function handler() { return 'POST response'; }")
        .build();
        
    let result = execute_script(&script);
    assert!(result.is_ok());
}
```

#### Coverage Requirements

- **New Code**: Must have >90% test coverage
- **Bug Fixes**: Must include regression tests
- **Refactoring**: Maintain or improve existing coverage
- **Coverage Reporting**: Use `cargo llvm-cov` for accurate metrics

```bash
# Generate coverage report
cargo llvm-cov --html --open

# Coverage requirements
# - Lines: >80% overall, >90% for new code
# - Functions: >85% overall, >95% for new code
# - Branches: >75% overall
```

### Property-Based Testing

For complex algorithms and data structures, use property-based testing:

```rust
// Example: Property-based testing for request ID generation
#[cfg(test)]
mod property_tests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn test_request_id_always_unique(count in 1..1000usize) {
            let ids: Vec<String> = (0..count).map(|_| generate_request_id()).collect();
            let mut unique_ids = ids.clone();
            unique_ids.sort();
            unique_ids.dedup();
            prop_assert_eq!(unique_ids.len(), ids.len());
        }
    }
}
```

## Error Handling

### Error Types Hierarchy

```rust
// Application-level errors
#[derive(Debug, thiserror::Error)]
pub enum ApplicationError {
    #[error("Script execution failed: {0}")]
    ScriptExecution(#[from] ScriptError),
    
    #[error("Configuration error: {0}")]
    Configuration(#[from] ConfigError),
    
    #[error("Repository error: {0}")]
    Repository(#[from] RepositoryError),
}

// Domain-specific errors
#[derive(Debug, thiserror::Error)]
pub enum ScriptError {
    #[error("Script not found: {script_id}")]
    NotFound { script_id: String },
    
    #[error("Script execution timeout after {timeout_ms}ms")]
    Timeout { timeout_ms: u64 },
    
    #[error("JavaScript runtime error: {message}")]
    RuntimeError { message: String },
}
```

### Error Context and Propagation

```rust
// Good: Rich error context
pub fn execute_script_with_timeout(
    script_id: &str, 
    timeout: Duration
) -> Result<ScriptResult, ScriptError> {
    let script = repository.get_script(script_id)
        .map_err(|e| ScriptError::NotFound { 
            script_id: script_id.to_string() 
        })?;
    
    let result = tokio::time::timeout(timeout, execute_script_internal(&script))
        .await
        .map_err(|_| ScriptError::Timeout { 
            timeout_ms: timeout.as_millis() as u64 
        })??;
        
    Ok(result)
}
```

### Structured Error Responses

```rust
// Consistent error response format
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: ErrorDetails,
    pub status: u16,
}

#[derive(Debug, Serialize)]
pub struct ErrorDetails {
    pub code: ErrorCode,
    pub message: String,
    pub details: Option<String>,
    pub request_id: String,
    pub timestamp: String,
    pub path: String,
    pub method: String,
    pub context: HashMap<String, Value>,
}
```

## Performance Guidelines

### Memory Management

✅ **DO**: Use appropriate data structures
```rust
// Use Cow for potentially borrowed data
pub fn process_content(content: Cow<'_, str>) -> ProcessedContent {
    match content {
        Cow::Borrowed(s) => ProcessedContent::from_borrowed(s),
        Cow::Owned(s) => ProcessedContent::from_owned(s),
    }
}

// Use Arc for shared immutable data
pub struct SharedConfig {
    inner: Arc<Config>,
}
```

✅ **DO**: Implement efficient algorithms
```rust
// Good: Use appropriate collections
use std::collections::HashMap;
use indexmap::IndexMap; // When insertion order matters

// Good: Pre-allocate when size is known
pub fn process_batch(items: &[Item]) -> Vec<ProcessedItem> {
    let mut results = Vec::with_capacity(items.len());
    for item in items {
        results.push(process_item(item));
    }
    results
}
```

### Async Performance

```rust
// Good: Use structured concurrency
pub async fn process_requests_concurrently(
    requests: Vec<Request>
) -> Result<Vec<Response>, ProcessingError> {
    const MAX_CONCURRENT: usize = 10;
    
    let semaphore = Semaphore::new(MAX_CONCURRENT);
    let futures = requests.into_iter().map(|req| {
        let semaphore = &semaphore;
        async move {
            let _permit = semaphore.acquire().await?;
            process_request(req).await
        }
    });
    
    futures::future::try_join_all(futures).await
}
```

### Caching Strategies

```rust
// Good: Implement intelligent caching
use std::sync::Arc;
use tokio::sync::RwLock;

pub struct ScriptCache {
    compiled_scripts: Arc<RwLock<HashMap<String, CompiledScript>>>,
    max_size: usize,
}

impl ScriptCache {
    pub async fn get_or_compile(&self, script_id: &str, source: &str) -> Result<CompiledScript, CompilationError> {
        // Try to read from cache first
        {
            let cache = self.compiled_scripts.read().await;
            if let Some(compiled) = cache.get(script_id) {
                return Ok(compiled.clone());
            }
        }
        
        // Compile and cache
        let compiled = compile_script(source)?;
        let mut cache = self.compiled_scripts.write().await;
        
        // Implement LRU eviction if needed
        if cache.len() >= self.max_size {
            self.evict_lru_entry(&mut cache);
        }
        
        cache.insert(script_id.to_string(), compiled.clone());
        Ok(compiled)
    }
}
```

## Security Considerations

### Input Validation

```rust
// Always validate and sanitize inputs
pub fn validate_script_registration(
    path: &str,
    method: &str,
    content: &str
) -> Result<(), ValidationError> {
    // Path validation
    if !path.starts_with('/') {
        return Err(ValidationError::InvalidPath("Path must start with '/'".to_string()));
    }
    
    if path.len() > MAX_PATH_LENGTH {
        return Err(ValidationError::PathTooLong(path.len()));
    }
    
    // Method validation
    const ALLOWED_METHODS: &[&str] = &["GET", "POST", "PUT", "DELETE", "PATCH"];
    if !ALLOWED_METHODS.contains(&method.to_uppercase().as_str()) {
        return Err(ValidationError::InvalidMethod(method.to_string()));
    }
    
    // Content validation
    if content.len() > MAX_SCRIPT_SIZE {
        return Err(ValidationError::ScriptTooLarge(content.len()));
    }
    
    Ok(())
}
```

### JavaScript Sandbox Security

```rust
// Implement secure JavaScript execution
pub struct SecureJsEngine {
    runtime: Runtime,
    max_memory: usize,
    execution_timeout: Duration,
}

impl SecureJsEngine {
    pub fn new() -> Result<Self, JsEngineError> {
        let mut runtime = Runtime::new()?;
        
        // Set memory limits
        runtime.set_memory_limit(MAX_JS_MEMORY);
        
        // Disable dangerous APIs
        runtime.set_loader(SecureModuleLoader::new());
        
        Ok(Self {
            runtime,
            max_memory: MAX_JS_MEMORY,
            execution_timeout: DEFAULT_JS_TIMEOUT,
        })
    }
    
    pub async fn execute_safely(&mut self, script: &str) -> Result<Value, ExecutionError> {
        // Execute with timeout and resource limits
        tokio::time::timeout(
            self.execution_timeout,
            self.execute_internal(script)
        ).await?
    }
}
```

### Authentication and Authorization

```rust
// Implement JWT-based authentication
#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    sub: String,
    exp: usize,
    roles: Vec<String>,
}

pub async fn authenticate_request(
    headers: &HeaderMap,
    required_role: &str
) -> Result<Claims, AuthError> {
    let token = extract_bearer_token(headers)?;
    let claims = validate_jwt_token(&token)?;
    
    if !claims.roles.contains(&required_role.to_string()) {
        return Err(AuthError::InsufficientPermissions);
    }
    
    Ok(claims)
}
```

## Documentation Standards

### Code Documentation

```rust
/// Executes a JavaScript function with the given arguments and returns the result.
///
/// This function provides a safe execution environment for JavaScript code with
/// resource limits and timeout protection.
///
/// # Arguments
///
/// * `script_content` - The JavaScript code to execute
/// * `function_name` - The name of the function to call
/// * `args` - Arguments to pass to the function as JSON values
/// * `timeout` - Maximum execution time allowed
///
/// # Returns
///
/// Returns `Ok(ScriptExecutionResult)` on successful execution, or an error if:
/// - The script has syntax errors
/// - The function is not found
/// - Execution times out
/// - Runtime errors occur
///
/// # Examples
///
/// ```rust
/// use aiwebengine::js_engine::{execute_script, ScriptExecutionResult};
/// use serde_json::json;
/// use std::time::Duration;
///
/// let script = r#"
///     function add(a, b) {
///         return a + b;
///     }
/// "#;
///
/// let result = execute_script(
///     script,
///     "add",
///     &json!([5, 3]),
///     Duration::from_secs(1)
/// )?;
///
/// assert_eq!(result.value, json!(8));
/// ```
///
/// # Security Notes
///
/// This function executes untrusted JavaScript code in a sandboxed environment.
/// Resource limits are enforced to prevent DoS attacks, but callers should
/// still validate input appropriately.
pub fn execute_script(
    script_content: &str,
    function_name: &str,
    args: &Value,
    timeout: Duration,
) -> Result<ScriptExecutionResult, ScriptError> {
    // Implementation...
}
```

### API Documentation

- Use OpenAPI/Swagger for HTTP API documentation
- Include examples for all endpoints
- Document error responses and status codes
- Provide SDK examples in multiple languages

### Architecture Documentation

```rust
//! # aiwebengine Core Architecture
//!
//! This module provides the core functionality for the aiwebengine web framework.
//! The architecture is built around several key components:
//!
//! ## Components
//!
//! ### JavaScript Engine (`js_engine`)
//! - Executes user-provided JavaScript code in a sandboxed environment
//! - Provides APIs for HTTP request handling and data access
//! - Implements resource limits and security controls
//!
//! ### HTTP Server (`server`)
//! - Handles incoming HTTP requests
//! - Routes requests to appropriate JavaScript handlers
//! - Manages middleware pipeline
//!
//! ### Configuration (`config`)
//! - Manages application configuration from multiple sources
//! - Supports environment-specific settings
//! - Validates configuration values
//!
//! ## Data Flow
//!
//! ```text
//! HTTP Request → Middleware → Router → JavaScript Handler → Response
//!      ↓              ↓          ↓             ↓             ↑
//!   Logging      Auth Check   Script      Function       JSON/HTML
//!  Req ID Gen   Rate Limit   Lookup      Execution      Formatting
//! ```
//!
//! ## Security Model
//!
//! The framework implements defense in depth:
//! - Request validation and sanitization
//! - JavaScript sandbox with resource limits
//! - Authentication and authorization middleware
//! - Audit logging for security events
```

## Contribution Workflow

### Git Workflow

1. **Branch Naming**: Use descriptive prefixes
   ```bash
   feature/graphql-subscriptions
   fix/memory-leak-in-js-engine  
   refactor/error-handling-consolidation
   docs/api-documentation-update
   ```

2. **Commit Messages**: Follow conventional commits
   ```
   feat: add GraphQL subscription support with Server-Sent Events
   
   - Implement SSE endpoint for real-time GraphQL subscriptions
   - Add subscription registration API for JavaScript handlers
   - Include comprehensive tests for subscription lifecycle
   
   Closes #123
   ```

3. **Pull Request Process**:
   - Include thorough description of changes
   - Reference related issues
   - Ensure all tests pass
   - Meet code coverage requirements
   - Update documentation as needed

### Code Review Guidelines

#### For Authors

- **Self-Review**: Review your own code first
- **Testing**: Include comprehensive tests
- **Documentation**: Update relevant documentation
- **Breaking Changes**: Clearly document any breaking changes

#### For Reviewers

- **Focus on Logic**: Check for correctness and edge cases
- **Performance**: Look for performance implications
- **Security**: Verify security considerations
- **Maintainability**: Ensure code is readable and well-structured

### Quality Gates

Before merging, ensure:

- [ ] All tests pass (unit, integration, e2e)
- [ ] Code coverage meets requirements (>80% lines)
- [ ] No security vulnerabilities detected
- [ ] Performance benchmarks show no regressions
- [ ] Documentation is updated
- [ ] Breaking changes are documented

## Architecture Guidelines

### Module Dependencies

```rust
// Good: Clear dependency hierarchy
pub mod config;      // No internal dependencies
pub mod error;       // Depends on: serde
pub mod middleware;  // Depends on: error
pub mod js_engine;   // Depends on: error, config  
pub mod graphql;     // Depends on: js_engine, error
pub mod server;      // Depends on: all above
```

### API Design Principles

#### 1. Consistency

```rust
// Good: Consistent naming patterns
pub trait Handler {
    async fn handle_request(&self, request: Request) -> Result<Response, HandlerError>;
}

pub trait Validator {
    fn validate_input(&self, input: &Input) -> Result<(), ValidationError>;
}

pub trait Repository {
    async fn find_by_id(&self, id: &str) -> Result<Option<Entity>, RepositoryError>;
    async fn save(&self, entity: Entity) -> Result<Entity, RepositoryError>;
    async fn delete(&self, id: &str) -> Result<(), RepositoryError>;
}
```

#### 2. Composability

```rust
// Good: Composable middleware
pub fn create_middleware_stack() -> MiddlewareStack {
    MiddlewareStack::new()
        .layer(request_id_middleware())
        .layer(logging_middleware())
        .layer(auth_middleware())
        .layer(rate_limiting_middleware())
}
```

#### 3. Extensibility

```rust
// Good: Plugin architecture
pub trait Plugin: Send + Sync {
    fn name(&self) -> &'static str;
    fn initialize(&mut self, context: &PluginContext) -> Result<(), PluginError>;
    fn handle_request(&self, request: &Request) -> Result<Option<Response>, PluginError>;
}

pub struct PluginManager {
    plugins: Vec<Box<dyn Plugin>>,
}
```

## Production Readiness Checklist

### Before Release

#### Code Quality
- [ ] All code follows style guidelines
- [ ] No `unwrap()` or `expect()` in production paths
- [ ] Comprehensive error handling implemented
- [ ] Performance benchmarks meet requirements
- [ ] Security audit completed

#### Testing
- [ ] Unit test coverage >80%
- [ ] Integration tests cover major workflows  
- [ ] Load testing under expected traffic
- [ ] Security testing (penetration testing)
- [ ] Chaos engineering/failure testing

#### Documentation
- [ ] API documentation complete and accurate
- [ ] Deployment guides updated
- [ ] Troubleshooting guides available
- [ ] Architecture documentation current

#### Observability
- [ ] Structured logging implemented
- [ ] Metrics collection configured
- [ ] Health checks functional
- [ ] Alerting rules defined
- [ ] Dashboards created

#### Security
- [ ] Input validation comprehensive
- [ ] Authentication/authorization working
- [ ] Security headers configured
- [ ] Rate limiting active
- [ ] Audit logging enabled

#### Operations
- [ ] Configuration management ready
- [ ] Deployment automation tested
- [ ] Backup/recovery procedures defined
- [ ] Scaling procedures documented
- [ ] Incident response procedures ready

### Monitoring and Maintenance

#### Key Metrics
- Response time (p50, p95, p99)
- Error rate by endpoint
- JavaScript execution time
- Memory usage patterns
- Active connections

#### Alerting Rules
- Error rate >1% for 5 minutes
- Response time p95 >500ms for 5 minutes  
- Memory usage >80% for 10 minutes
- JavaScript execution timeouts >5% for 5 minutes

#### Regular Maintenance
- Security updates applied monthly
- Performance optimization quarterly
- Dependency updates with testing
- Log rotation and cleanup
- Database maintenance and optimization

---

## Getting Help

### Resources

- **Documentation**: `/docs` directory contains comprehensive guides
- **Examples**: `/scripts/example_scripts` for working code samples
- **Issues**: GitHub issues for bug reports and feature requests
- **Discussions**: GitHub discussions for questions and ideas

### Contact

For questions about these guidelines or architectural decisions:

1. Check existing documentation in `/docs`
2. Search GitHub issues and discussions
3. Create a new discussion for questions
4. Open an issue for bugs or specific problems

---

*This document is a living guide that evolves with the project. Contributions and improvements are welcome through pull requests.*