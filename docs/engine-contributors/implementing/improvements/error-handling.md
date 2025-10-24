# Error Handling & Stability Improvements

**Priority:** 🔴 Critical (Blocking v1.0)  
**Status:** 🚧 In Progress  
**Effort:** 2-3 days  
**Owner:** Unassigned

This guide addresses critical error handling improvements needed before v1.0 release.

---

## 1. Current State Assessment

### What Works

- ✅ Error types defined (`src/error.rs`)
- ✅ Some functions use `Result<T, E>` properly
- ✅ Error middleware for HTTP responses

### What's Problematic

- ❌ **20+ `unwrap()` calls in production code** - Can cause panics and crash the server
- ❌ **1 failing test** - `test_register_web_stream_invalid_path`
- ❌ **No JavaScript execution timeouts** - Infinite loops can hang the server
- ❌ **Incomplete mutex poisoning recovery** - Poisoned mutexes not always recovered
- ⚠️ **Some error messages unclear** - Hard to debug issues

### Impact

**User Impact:**

- Server crashes from panics lose all in-memory sessions
- No graceful degradation when components fail
- Poor error messages make troubleshooting difficult

**Developer Impact:**

- Hard to debug issues
- Fear of making changes (might cause panics)
- Unclear error handling patterns

---

## 2. Problems to Solve

### Problem 1: Panic-Causing `unwrap()` Calls

**Locations (20+ instances):**

- `src/lib.rs`: Lines 75, 308, 319, 379, 441-442, 448-449, 529-530, 558-559, 567-568, 851
- `src/js_engine.rs`: Lines 994, 1250, 1447, 1466, 1486, 1764

**Example:**

```rust
// CURRENT (Dangerous):
let result_value = result.as_string().unwrap().to_string()?;

// SHOULD BE:
let result_value = result.as_string()
    .ok_or_else(|| JsError::InvalidReturnType("Expected string".into()))?
    .to_string()?;
```

**Why it's critical:**

A single panic crashes the entire server, losing all active sessions and connections.

### Problem 2: No Execution Timeouts

**Current state:**

JavaScript can run indefinitely with no timeout mechanism.

**Consequences:**

- Infinite loops hang the server
- Resource exhaustion attacks possible
- No way to limit execution time

**Needed:**

```rust
pub async fn execute_script_with_timeout(
    script: &str,
    timeout: Duration
) -> Result<ScriptResult, ScriptError> {
    tokio::time::timeout(timeout, execute_script_internal(script))
        .await
        .map_err(|_| ScriptError::Timeout {
            timeout_ms: timeout.as_millis() as u64
        })??
}
```

### Problem 3: Failing Test

**Test:** `test_register_web_stream_invalid_path`  
**Status:** Failing (blocks 100% pass rate)  
**Impact:** Can't claim code quality until fixed

### Problem 4: Mutex Poisoning Recovery

**Current state:**

Some mutexes have recovery, some don't.

**Example of good pattern:**

```rust
let store = Self::get_store().lock()
    .map_err(|_| RepositoryError::LockError)?;
```

**Needed everywhere:**

All mutex accesses should recover from poisoning gracefully.

---

## 3. Implementation Tasks

### Phase 1: Remove All `unwrap()` Calls (Day 1-2)

**src/lib.rs Tasks:**

- [ ] Line 75: `schema.lock().unwrap()` → Use proper error handling
- [ ] Lines 308, 319: Template interpolation unwraps → Return errors
- [ ] Line 379: JSON parsing unwrap → Handle parse errors
- [ ] Lines 441-442, 448-449: Cookie handling unwraps → Return errors
- [ ] Lines 529-530, 558-559, 567-568: Request parsing unwraps → Handle errors
- [ ] Line 851: Stream lock unwrap → Handle poisoning

**src/js_engine.js Tasks:**

- [ ] Line 994: Context unwrap → Handle error
- [ ] Line 1250: String conversion unwrap → Handle error
- [ ] Lines 1447, 1466, 1486: JSON unwraps → Handle parse errors
- [ ] Line 1764: Function call unwrap → Handle error

**Success Criteria:**

```bash
# Must return zero results:
grep -r "\.unwrap()" src/ | grep -v "test" | grep -v "\/\/"
```

### Phase 2: Add Execution Timeouts (Day 2)

**Tasks:**

- [ ] Add `execution_timeout` field to `JsEngine` config
- [ ] Wrap script execution in `tokio::time::timeout`
- [ ] Add `ScriptError::Timeout` variant
- [ ] Update all script execution call sites
- [ ] Add tests for timeout scenarios

**Configuration:**

```rust
// Add to config.rs
pub struct JsEngineConfig {
    pub execution_timeout_ms: u64, // Default: 5000 (5 seconds)
    pub max_memory_mb: usize,       // Default: 256
}
```

### Phase 3: Fix Failing Test (Day 2)

**Test:** `test_register_web_stream_invalid_path`

**Tasks:**

- [ ] Run test with `--nocapture` to see actual failure
- [ ] Identify root cause
- [ ] Fix the issue
- [ ] Verify fix doesn't break other tests
- [ ] Add regression test if needed

**Debug Command:**

```bash
cargo test test_register_web_stream_invalid_path -- --nocapture
```

### Phase 4: Standardize Error Patterns (Day 3)

**Create error handling helpers:**

```rust
// src/error_helpers.rs (NEW FILE)

/// Safe mutex lock that recovers from poisoning
pub fn safe_lock<T>(
    mutex: &Mutex<T>
) -> Result<MutexGuard<T>, LockError> {
    mutex.lock().map_err(|_| LockError::Poisoned)
}

/// Safe JSON parse with context
pub fn safe_parse_json<T: DeserializeOwned>(
    json_str: &str,
    context: &str
) -> Result<T, ParseError> {
    serde_json::from_str(json_str)
        .map_err(|e| ParseError::JsonParse {
            context: context.to_string(),
            error: e.to_string(),
        })
}
```

**Tasks:**

- [ ] Create `src/error_helpers.rs`
- [ ] Implement helper functions
- [ ] Refactor existing code to use helpers
- [ ] Add tests for helpers

---

## 4. Success Metrics

### Must Achieve

- [ ] **Zero `unwrap()` calls** in production code paths
- [ ] **100% test pass rate** (126/126 tests passing)
- [ ] **All error paths return `Result<T, E>`** with proper error types
- [ ] **Execution timeouts** implemented and tested
- [ ] **Mutex poisoning recovery** in all lock call sites

### Quality Indicators

- [ ] Error messages are clear and actionable
- [ ] All errors include context (what was being done)
- [ ] No panics in normal operation or error scenarios
- [ ] Graceful degradation when components fail

### Test Coverage

- [ ] All error paths have tests
- [ ] Timeout scenarios tested
- [ ] Mutex poisoning scenarios tested
- [ ] Integration tests verify error propagation

---

## 5. Testing Strategy

### Unit Tests

**For each unwrap() removal:**

```rust
#[test]
fn test_handles_error_case() {
    let result = function_that_used_to_unwrap(invalid_input);
    assert!(result.is_err());
    match result.unwrap_err() {
        ExpectedError::SpecificVariant { .. } => (),
        other => panic!("Unexpected error: {:?}", other),
    }
}
```

### Integration Tests

**Test timeout enforcement:**

```rust
#[tokio::test]
async fn test_script_execution_timeout() {
    let infinite_loop = "while(true) {}";
    let timeout = Duration::from_millis(100);
    
    let result = execute_script_with_timeout(infinite_loop, timeout).await;
    
    assert!(matches!(result, Err(ScriptError::Timeout { .. })));
}
```

**Test mutex recovery:**

```rust
#[test]
fn test_mutex_poisoning_recovery() {
    // Poison the mutex
    let mutex = Arc::new(Mutex::new(42));
    let mutex_clone = mutex.clone();
    
    let _ = std::panic::catch_unwind(|| {
        let _guard = mutex_clone.lock().unwrap();
        panic!("Intentional panic to poison mutex");
    });
    
    // Verify recovery
    let result = safe_lock(&mutex);
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), LockError::Poisoned));
}
```

### Regression Tests

- [ ] Add test for each fixed panic scenario
- [ ] Ensure tests fail before fix, pass after
- [ ] Document the scenario being prevented

---

## 6. Implementation Checklist

### Before Starting

- [ ] Read [DEVELOPMENT.md](../DEVELOPMENT.md) error handling section
- [ ] Review existing error types in `src/error.rs`
- [ ] Set up test coverage reporting

### During Implementation

- [ ] Fix errors incrementally (file by file)
- [ ] Run tests after each change
- [ ] Update error types as needed
- [ ] Add tests for each fix

### Before Committing

- [ ] Run `cargo test` - all tests pass
- [ ] Run `cargo clippy` - no warnings
- [ ] Run grep for unwrap() - returns nothing
- [ ] Review all error messages for clarity
- [ ] Update documentation

---

## 7. Code Examples

### Pattern 1: Replace unwrap() with proper error handling

**Before:**

```rust
pub fn get_script(&self, id: &str) -> Script {
    let store = SCRIPTS.get().unwrap().lock().unwrap();
    store.get(id).unwrap().clone()
}
```

**After:**

```rust
pub fn get_script(&self, id: &str) -> Result<Script, RepositoryError> {
    let store = SCRIPTS.get()
        .ok_or(RepositoryError::NotInitialized)?
        .lock()
        .map_err(|_| RepositoryError::LockError)?;
    
    store.get(id)
        .cloned()
        .ok_or_else(|| RepositoryError::NotFound {
            entity_type: "script",
            id: id.to_string()
        })
}
```

### Pattern 2: Add timeout wrapper

**Before:**

```rust
pub async fn execute_script(script: &str) -> Result<Value, ScriptError> {
    // No timeout - can run forever
    execute_internal(script).await
}
```

**After:**

```rust
pub async fn execute_script(
    script: &str,
    timeout: Duration
) -> Result<Value, ScriptError> {
    tokio::time::timeout(timeout, execute_internal(script))
        .await
        .map_err(|_| ScriptError::Timeout {
            timeout_ms: timeout.as_millis() as u64
        })?
}
```

### Pattern 3: Add context to errors

**Before:**

```rust
let value: MyType = serde_json::from_str(json_str)?;
```

**After:**

```rust
let value: MyType = serde_json::from_str(json_str)
    .map_err(|e| ParseError::JsonParse {
        context: "parsing user configuration".to_string(),
        source: e,
    })?;
```

---

## 8. Related Resources

- [DEVELOPMENT.md](../DEVELOPMENT.md) - Error handling guidelines
- [guides/error-handling-patterns.md](../guides/error-handling-patterns.md) - Detailed patterns
- [Rust Error Handling Book](https://doc.rust-lang.org/book/ch09-00-error-handling.html)
- [thiserror crate docs](https://docs.rs/thiserror/)

---

## 9. FAQ

**Q: Can I ever use unwrap()?**

A: Only in tests or when you have a compile-time guarantee (like constants).

**Q: What about expect()?**

A: Same as unwrap() - avoid in production code.

**Q: How do I handle multiple error types?**

A: Use `map_err()` to convert to your error type, or use `thiserror` derive macros.

**Q: Should every function return Result?**

A: If it can fail, yes. If it truly can't fail, document why with a comment.

---

_This improvement is **CRITICAL** and must be completed before authentication work begins._
