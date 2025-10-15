# Script Initialization Function Implementation Plan

**Requirement**: REQ-JS-010: Script Initialization Function  
**Priority**: HIGH  
**Status**: PLANNED  
**Created**: October 15, 2025

---

## Overview

Implement support for an optional `init()` function in JavaScript scripts that is called:

1. When a script is registered or updated via `upsert_script`
2. When the server starts for all registered scripts

This allows scripts to perform initialization tasks such as:

- Registering HTTP route handlers
- Setting up subscriptions or background tasks
- Initializing script-level state
- Loading configuration
- Registering GraphQL resolvers

---

## Requirements Summary

From REQ-JS-010:

- ✅ Call `init()` function when script is upserted (if it exists)
- ✅ Call `init()` function for all scripts on server startup
- ✅ Make `init()` function optional
- ✅ Handle errors gracefully without stopping other scripts
- ✅ Provide context to `init()` function
- ✅ Respect script timeout limits
- ✅ Re-run `init()` on script updates

---

## Implementation Phases

### Phase 1: Core Infrastructure (Week 1)

#### 1.1 Add Script Metadata Tracking

**File**: `src/repository.rs`

- Add `initialized` field to script metadata
- Add `init_error` field to track initialization failures
- Add `last_init_time` timestamp

```rust
pub struct ScriptMetadata {
    pub name: String,
    pub code: String,
    pub created_at: std::time::SystemTime,
    pub updated_at: std::time::SystemTime,
    pub initialized: bool,           // NEW
    pub init_error: Option<String>,  // NEW
    pub last_init_time: Option<std::time::SystemTime>, // NEW
}
```

#### 1.2 Create Initialization Module

**New File**: `src/script_init.rs`

Create a new module to handle script initialization:

```rust
pub struct ScriptInitializer {
    js_engine: Arc<JsEngine>,
    repository: Arc<dyn ScriptRepository>,
}

impl ScriptInitializer {
    pub async fn initialize_script(&self, script_name: &str) -> Result<(), Error>;
    pub async fn initialize_all_scripts(&self) -> Result<Vec<InitResult>, Error>;
    fn call_init_function(&self, ctx: &Context, script_name: &str) -> Result<(), Error>;
}

pub struct InitResult {
    pub script_name: String,
    pub success: bool,
    pub error: Option<String>,
    pub duration_ms: u64,
}
```

**Tasks**:

- [x] Create module structure
- [ ] Implement `initialize_script` method
- [ ] Implement `initialize_all_scripts` method
- [ ] Implement `call_init_function` helper
- [ ] Add error handling and logging
- [ ] Add timeout enforcement

#### 1.3 Modify JsEngine for Init Context

**File**: `src/js_engine.rs`

Add method to check for and call `init()` function:

```rust
impl JsEngine {
    /// Check if script has an init function and call it
    pub fn call_init_if_exists(
        &self,
        script_name: &str,
        script_code: &str,
    ) -> Result<bool, Error> {
        // Returns true if init was called, false if no init function exists
    }

    /// Prepare init context with metadata
    fn prepare_init_context(&self, script_name: &str) -> InitContext {
        // Provide script metadata to init function
    }
}

pub struct InitContext {
    pub script_name: String,
    pub timestamp: std::time::SystemTime,
    // Future: config, server state, etc.
}
```

**Tasks**:

- [ ] Implement `call_init_if_exists` method
- [ ] Add function existence check (check for `init` in global scope)
- [ ] Create execution context for init
- [ ] Pass context object to init function
- [ ] Handle init function return values
- [ ] Catch and wrap errors appropriately

---

### Phase 2: Integration with Script Management (Week 2)

#### 2.1 Update Script Upsert Flow

**File**: `src/repository.rs` and handlers

Modify the upsert operation to call init:

```rust
pub async fn upsert_script_with_init(
    &self,
    name: String,
    code: String,
    initializer: &ScriptInitializer,
) -> Result<(), Error> {
    // 1. Store/update script
    self.upsert_script(name.clone(), code)?;

    // 2. Call init function
    match initializer.initialize_script(&name).await {
        Ok(_) => {
            // Mark as initialized
            self.mark_script_initialized(&name, true, None)?;
        }
        Err(e) => {
            // Mark as failed but keep script
            self.mark_script_initialized(&name, false, Some(e.to_string()))?;
            // Log but don't fail the upsert
            warn!("Script {} init failed: {}", name, e);
        }
    }

    Ok(())
}
```

**Tasks**:

- [ ] Update GraphQL mutation `upsertScript` to call init
- [ ] Update any other script registration endpoints
- [ ] Add init status to upsert response
- [ ] Add configuration option to disable auto-init on upsert
- [ ] Log initialization attempts and results

#### 2.2 Server Startup Script Initialization

**File**: `src/main.rs`

Add initialization phase during server startup:

```rust
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ... existing setup ...

    // Initialize all scripts
    info!("Initializing registered scripts...");
    let initializer = ScriptInitializer::new(js_engine.clone(), repository.clone());
    let init_results = initializer.initialize_all_scripts().await?;

    // Log results
    for result in init_results {
        if result.success {
            info!("✓ Script '{}' initialized in {}ms",
                  result.script_name, result.duration_ms);
        } else {
            warn!("✗ Script '{}' init failed: {}",
                  result.script_name, result.error.unwrap_or_default());
        }
    }

    // Continue with server startup...
}
```

**Tasks**:

- [ ] Add initialization phase before server starts
- [ ] Make initialization order deterministic (alphabetical or by dependency)
- [ ] Add configuration for initialization timeout
- [ ] Add option to fail startup if critical scripts fail init
- [ ] Add metrics/monitoring for init phase duration
- [ ] Consider parallel initialization for independent scripts

---

### Phase 3: JavaScript API Enhancements (Week 2-3)

#### 3.1 Expose Init Context to JavaScript

**File**: `src/js_engine.rs`

Make script metadata available in the init function:

```javascript
// What the init function receives
function init(context) {
  console.log("Initializing script:", context.scriptName);
  console.log("Init timestamp:", context.timestamp);
  // Future: context.config, context.serverInfo, etc.
}
```

**Tasks**:

- [ ] Create `InitContext` object in JavaScript global scope during init
- [ ] Add `scriptName` property
- [ ] Add `timestamp` property
- [ ] Add `isStartup` flag (true on server start, false on upsert)
- [ ] Document context object structure

#### 3.2 Add Helper Functions for Init

**File**: `src/safe_helpers.rs` and `src/js_engine.rs`

Consider adding init-specific utilities:

```javascript
// Example helpers that might be useful
function init(context) {
  // Check if we're initializing at startup or on update
  if (context.isStartup) {
    // Different behavior on startup vs. update
  }

  // Register handlers
  registerHandler("GET", "/api/users", handleUsers);
  registerGraphQLQuery("users", schema, resolveUsers);
}
```

**Tasks**:

- [ ] Document best practices for init functions
- [ ] Consider adding `isReInit()` helper
- [ ] Add validation utilities for init
- [ ] Document what globals are available in init context

---

### Phase 4: Error Handling & Recovery (Week 3)

#### 4.1 Graceful Error Handling

Implement comprehensive error handling:

```rust
pub enum InitError {
    ScriptNotFound(String),
    NoInitFunction,  // Not really an error
    InitTimeout(String),
    InitException { script: String, error: String },
    EngineError(String),
}

impl ScriptInitializer {
    async fn initialize_script(&self, script_name: &str) -> Result<InitResult, Error> {
        // Set timeout
        let timeout_duration = Duration::from_millis(
            self.config.script_timeout_ms
        );

        match timeout(timeout_duration, self.call_init_internal(script_name)).await {
            Ok(Ok(result)) => Ok(result),
            Ok(Err(e)) => {
                // Init function threw error
                warn!("Init failed for {}: {}", script_name, e);
                Ok(InitResult::failed(script_name, e.to_string()))
            }
            Err(_) => {
                // Timeout
                error!("Init timeout for {}", script_name);
                Ok(InitResult::failed(script_name, "Init timeout".to_string()))
            }
        }
    }
}
```

**Tasks**:

- [ ] Implement timeout handling
- [ ] Catch JavaScript exceptions
- [ ] Log all init errors with context
- [ ] Don't fail other scripts if one fails
- [ ] Store error details for inspection
- [ ] Add retry logic for transient failures (optional)

#### 4.2 Init Status Inspection

Add GraphQL queries to inspect init status:

```graphql
type ScriptInitStatus {
  scriptName: String!
  initialized: Boolean!
  lastInitTime: String
  initError: String
  initDurationMs: Int
}

type Query {
  scriptInitStatus(name: String!): ScriptInitStatus
  allScriptsInitStatus: [ScriptInitStatus!]!
}
```

**Tasks**:

- [ ] Add GraphQL query for init status
- [ ] Add REST endpoint for init status (optional)
- [ ] Include init status in script list responses
- [ ] Add init status to health check endpoint

---

### Phase 5: Testing (Week 3-4)

#### 5.1 Unit Tests

**New File**: `tests/script_init_test.rs`

Test cases:

- [ ] Script with `init()` function is called on upsert
- [ ] Script without `init()` function works normally
- [ ] Init function receives correct context
- [ ] Init errors don't prevent script storage
- [ ] Init timeout is enforced
- [ ] All scripts initialized on server startup
- [ ] Init is re-run on script update
- [ ] Multiple upserts call init multiple times
- [ ] Init errors are logged and stored
- [ ] Concurrent script inits work correctly

```rust
#[tokio::test]
async fn test_init_function_called_on_upsert() {
    let script = r#"
        let initCalled = false;

        function init(context) {
            initCalled = true;
            writeLog("Init called for: " + context.scriptName);
        }

        function testHandler(request) {
            return {
                status: 200,
                body: JSON.stringify({ initCalled: initCalled })
            };
        }
    "#;

    // Upsert script
    upsert_script("test", script).await.unwrap();

    // Verify init was called
    let status = get_script_init_status("test").await.unwrap();
    assert!(status.initialized);
}
```

#### 5.2 Integration Tests

Test real-world scenarios:

- [ ] Init registers HTTP handlers that work
- [ ] Init sets up GraphQL subscriptions
- [ ] Init errors are recoverable
- [ ] Server startup initializes all scripts
- [ ] Hot reload re-runs init
- [ ] Init has access to all expected globals

```rust
#[tokio::test]
async fn test_init_registers_handler() {
    let script = r#"
        function init(context) {
            register('/dynamic', 'handleDynamic', 'GET');
        }

        function handleDynamic(request) {
            return { status: 200, body: "Registered in init!" };
        }
    "#;

    upsert_script("dynamic", script).await.unwrap();

    // Test that the handler works
    let response = test_get("/dynamic").await;
    assert_eq!(response.status, 200);
    assert_eq!(response.body, "Registered in init!");
}
```

#### 5.3 Error Handling Tests

Test error scenarios:

- [ ] Init function that throws error
- [ ] Init function that times out
- [ ] Init function with infinite loop
- [ ] Missing dependencies in init
- [ ] Init that uses unavailable APIs

---

### Phase 6: Documentation (Week 4)

#### 6.1 Update User Documentation

**File**: `docs/javascript-apis.md`

Add section on script initialization:

```markdown
## Script Initialization

Scripts can optionally define an `init()` function that is called:

- When the script is first registered or updated
- When the server starts

### Usage

\`\`\`javascript
function init(context) {
// context.scriptName - Name of this script
// context.timestamp - When init was called
// context.isStartup - true if server startup, false if upsert

    // Register HTTP handlers
    register('/api/users', 'handleUsers', 'GET');

    // Register GraphQL resolvers
    registerGraphQLQuery('users', schema, resolveUsers);

    // Set up initial state
    globalState.initialized = true;

    writeLog("Script initialized: " + context.scriptName);

}
\`\`\`

### Best Practices

1. Keep init functions fast and simple
2. Init must complete within script timeout
3. Don't rely on external services in init
4. Handle init errors gracefully
5. Use init for registration, not heavy computation
   ...
```

**Tasks**:

- [ ] Add init() documentation to API reference
- [ ] Add examples of common init patterns
- [ ] Document init context object
- [ ] Add troubleshooting section for init errors
- [ ] Update APP_DEVELOPMENT.md with init patterns

#### 6.2 Create Example Scripts

**New File**: `scripts/example_scripts/init_example.js`

Create comprehensive examples:

```javascript
// Example 1: Basic init with handler registration
function init(context) {
  writeLog("Initializing " + context.scriptName);

  // Register multiple handlers
  register("/api/hello", "handleHello", "GET");
  register("/api/data", "handleData", "POST");
}

function handleHello(request) {
  return { status: 200, body: "Hello from initialized script!" };
}

function handleData(request) {
  return { status: 200, body: JSON.stringify(request.body) };
}
```

**Tasks**:

- [ ] Create basic init example
- [ ] Create GraphQL init example
- [ ] Create state initialization example
- [ ] Create error handling example
- [ ] Add examples to docs/examples.md

#### 6.3 Update Requirements Documentation

**File**: `REQUIREMENTS.md`

- [x] REQ-JS-010 already added
- [ ] Add init() to JavaScript API listing
- [ ] Update examples in various requirement sections

---

## Configuration

Add new configuration options:

```toml
[javascript]
# Existing options...
enable_init_functions = true  # Enable/disable init functions
init_timeout_ms = 5000        # Timeout for init functions (default: same as script_timeout)
fail_startup_on_init_error = false  # Fail server startup if any init fails
init_retry_attempts = 0       # Number of retry attempts for failed inits
init_retry_delay_ms = 1000   # Delay between retry attempts
```

---

## Migration Strategy

### For Existing Deployments

1. **Backward Compatibility**: Scripts without `init()` work exactly as before
2. **Gradual Adoption**: Scripts can add `init()` incrementally
3. **No Breaking Changes**: All existing functionality preserved

### For New Features

Scripts that currently use patterns like this:

```javascript
// Old pattern - runs on every request
if (!globalState.initialized) {
  register("/api/users", "handleUsers", "GET");
  globalState.initialized = true;
}
```

Can be refactored to:

```javascript
// New pattern - runs once on init
function init(context) {
  register("/api/users", "handleUsers", "GET");
}
```

---

## Success Criteria

- [ ] Init function is called on script upsert
- [ ] Init function is called for all scripts on server startup
- [ ] Scripts without init() work normally
- [ ] Init errors don't break script storage
- [ ] Init timeout is enforced
- [ ] Init status is queryable
- [ ] All tests pass
- [ ] Documentation is complete
- [ ] Example scripts demonstrate usage
- [ ] Zero breaking changes to existing scripts

---

## Timeline

| Week | Phase   | Deliverables                                  |
| ---- | ------- | --------------------------------------------- |
| 1    | Phase 1 | Core infrastructure, ScriptInitializer module |
| 2    | Phase 2 | Integration with upsert and startup           |
| 2-3  | Phase 3 | JavaScript API enhancements                   |
| 3    | Phase 4 | Error handling and recovery                   |
| 3-4  | Phase 5 | Comprehensive testing                         |
| 4    | Phase 6 | Documentation and examples                    |

**Total Estimated Time**: 4 weeks

---

## Risks and Mitigations

| Risk                             | Impact | Mitigation                                           |
| -------------------------------- | ------ | ---------------------------------------------------- |
| Init functions hang indefinitely | High   | Enforce strict timeouts                              |
| Init errors break server startup | High   | Graceful error handling, continue with other scripts |
| Race conditions in parallel init | Medium | Deterministic initialization order                   |
| Breaking existing scripts        | High   | Thorough backward compatibility testing              |
| Performance impact on startup    | Medium | Parallel initialization, monitoring                  |

---

## Future Enhancements

After initial implementation, consider:

1. **Init Dependencies**: Allow scripts to declare dependencies on other scripts
2. **Init Phases**: Multiple init phases (pre-init, init, post-init)
3. **Hot Reload Detection**: Different behavior on hot reload vs. cold start
4. **Init Hooks**: Allow plugins to hook into init process
5. **Init Validation**: Pre-flight validation before calling init
6. **Async Init**: Support for async/await in init functions
7. **Init Context Extensions**: More metadata in context object

---

## References

- **Requirement**: REQ-JS-010 in REQUIREMENTS.md
- **Related Requirements**:
  - REQ-JS-007: Script Management
  - REQ-JS-003: Execution Timeout
  - REQ-JS-006: Error Handling
  - REQ-TEST-007: Test Infrastructure
- **Related Files**:
  - `src/repository.rs` - Script storage
  - `src/js_engine.rs` - JavaScript execution
  - `src/graphql.rs` - GraphQL mutations
  - `src/main.rs` - Server startup

---

## Implementation Checklist

### Week 1: Core Infrastructure

- [ ] Create `src/script_init.rs` module
- [ ] Add metadata fields to ScriptMetadata
- [ ] Implement ScriptInitializer struct
- [ ] Implement initialize_script method
- [ ] Implement initialize_all_scripts method
- [ ] Add JsEngine::call_init_if_exists method
- [ ] Add timeout enforcement
- [ ] Add basic error handling

### Week 2: Integration

- [ ] Update upsert_script to call init
- [ ] Add server startup initialization
- [ ] Add init status tracking
- [ ] Add configuration options
- [ ] Add logging for init operations
- [ ] Update GraphQL mutations

### Week 3: Polish

- [ ] Add InitContext to JavaScript
- [ ] Implement comprehensive error handling
- [ ] Add retry logic (optional)
- [ ] Add init status GraphQL queries
- [ ] Write unit tests
- [ ] Write integration tests

### Week 4: Documentation

- [ ] Update API documentation
- [ ] Create example scripts
- [ ] Update user guides
- [ ] Add troubleshooting guide
- [ ] Review and final testing
- [ ] Release notes preparation
