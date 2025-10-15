# Test Failures Analysis

**Date**: October 14, 2025  
**Test Run**: `cargo nextest run --no-fail-fast`  
**Total Failures**: 6 tests

## Executive Summary

All 6 failing tests **can be fixed** based on existing REQUIREMENTS.md documentation. The issues stem from **implementation bugs and incomplete feature implementation**, NOT from missing requirements. The requirements are sufficiently clear and comprehensive.

---

## Test Failure Analysis

### 1. Security Test Failure

#### Test: `test_anonymous_user_has_minimal_capabilities`

**File**: `tests/security_integration.rs:291`  
**Status**: ❌ FAILS - Implementation Bug

**Error**:

```
assertion failed: anon_user.require_capability(&Capability::WriteScripts).is_err()
```

**Root Cause**:
The `UserContext::anonymous()` implementation in `src/security/capabilities.rs:36-46` grants **WriteScripts** capability to anonymous users, which directly violates security requirements.

**Current Implementation** (INCORRECT):

```rust
fn anonymous_capabilities() -> HashSet<Capability> {
    // Anonymous users can read and write (for development/testing)
    // In production, these should be restricted to authenticated users only
    [
        Capability::ViewLogs,
        Capability::ReadScripts,
        Capability::WriteScripts,  // ❌ SECURITY VIOLATION
        Capability::ReadAssets,
    ]
    .into_iter()
    .collect()
}
```

**Requirement Violated**: REQ-AUTH-006 (Anonymous User Restrictions)

> Anonymous users SHOULD have minimal capabilities:
>
> - Read public content ONLY
> - NO write operations
> - NO script management
> - NO GraphQL mutations

**Required Fix**:
Remove `WriteScripts` from anonymous capabilities:

```rust
fn anonymous_capabilities() -> HashSet<Capability> {
    [
        Capability::ReadScripts,  // Read public scripts only
        Capability::ReadAssets,   // Read public assets only
    ]
    .into_iter()
    .collect()
}
```

**Can Fix**: ✅ YES - Clear requirement exists, simple code change needed

---

### 2. Script Management HTTP API Failures

#### Tests:

- `test_delete_script_endpoint` (line 314)
- `test_script_lifecycle_via_http_api` (line 469)
- `test_read_script_endpoint` (line 582)

**Status**: ❌ FAILS - Implementation Bug

**Error Pattern**:

```
assertion `left == right` failed: Expected 404 for deleted/nonexistent script
  left: 200
  right: 404
```

**Root Cause**:
The JavaScript global functions `getScript()` and `deleteScript()` return **string messages** instead of proper **null/boolean values** that the HTTP handlers expect.

**Current Implementation Issues**:

1. **getScript() returns string on failure** (src/security/secure_globals.rs:327):

```rust
match repository::fetch_script(&script_name) {
    Some(content) => Ok(content),
    None => Ok(format!("Script '{}' not found", script_name)),  // ❌ Returns string, not null
}
```

2. **deleteScript() returns string on failure** (src/security/secure_globals.rs:422):

```rust
match repository::delete_script(&script_name) {
    true => Ok(format!("Script '{}' deleted successfully", script_name)),
    false => Ok(format!("Script '{}' not found", script_name)),  // ❌ Returns string, not boolean
}
```

3. **JavaScript handler checks falsy value** (scripts/feature_scripts/core.js:273):

```javascript
const content = getScript(uri);

if (content) {
  // ❌ String "Script 'x' not found" is truthy!
  return { status: 200, body: content, contentType: "application/javascript" };
} else {
  return { status: 404, body: JSON.stringify({ error: "Script not found" }) };
}
```

**Requirement Coverage**: REQ-JS-006 (Script Management API)

> - `getScript(name)` - Retrieve script content by name
>   - Returns script content string on success
>   - Returns null/undefined on failure
> - `deleteScript(name)` - Delete a script
>   - Returns true on success
>   - Returns false if script not found

**Required Fixes**:

1. **Fix getScript() to return null on failure**:

```rust
match repository::fetch_script(&script_name) {
    Some(content) => Ok(content),
    None => Err(rquickjs::Error::new_from_js("value", "null")), // Return JS null
}
```

OR use `Option<String>` return type:

```rust
move |_ctx: rquickjs::Ctx<'_>, script_name: String| -> JsResult<Option<String>> {
    // ...
    Ok(repository::fetch_script(&script_name))
}
```

2. **Fix deleteScript() to return boolean**:

```rust
move |_ctx: rquickjs::Ctx<'_>, script_name: String| -> JsResult<bool> {
    // ... capability checks ...
    Ok(repository::delete_script(&script_name))
}
```

**Can Fix**: ✅ YES - Requirements clearly specify return types, implementation just needs correction

---

### 3. Script Streaming Integration Failures

#### Tests:

- `test_script_update_streaming_integration` (line 157)
- `test_script_update_message_format` (line 321)

**Status**: ❌ FAILS - Test Infrastructure Issue

**Error**:

```
Timeout waiting for insert/inserted message
```

**Root Cause**:
Tests are trying to use streaming functionality **without a running HTTP server**. The tests directly call `execute_script()` which doesn't trigger the stream broadcast mechanism that requires an active server.

**Test Code Issue** (tests/script_streaming_integration.rs:133-142):

```rust
// This registers the stream but doesn't start a server
let result = aiwebengine::js_engine::execute_script("test_streaming_core.js", core_script_content);
assert!(result.success);

// This creates a receiver but no server is broadcasting
let (receiver, connection_id) = GLOBAL_STREAM_REGISTRY
    .create_connection("/script_updates_test1")
    .expect("Failed to create connection");

// This executes JS but doesn't go through HTTP layer to trigger broadcasts
let insert_result = aiwebengine::js_engine::execute_script_for_request(...);

// Waiting for message that will never come because no server is running
match tokio::time::timeout(Duration::from_secs(2), receiver.recv()).await {
    // ❌ Times out because broadcast never happens
}
```

**Requirement Coverage**: REQ-TEST-007 (Test Infrastructure)

> The project MUST implement robust test infrastructure:
>
> - **Test server lifecycle management** - No resource leaks
> - **Test isolation and cleanup** requirements
> - **Parallel test execution** support

**Required Fix**:
Refactor streaming tests to use `TestContext` pattern like other integration tests:

```rust
#[tokio::test]
async fn test_script_update_streaming_integration() {
    let context = common::TestContext::new();
    let port = context.start_server().await.expect("Server failed to start");

    common::wait_for_server(port, 40).await.expect("Server not ready");

    // Now use HTTP client to trigger upsert which will broadcast
    let client = reqwest::Client::new();

    // Subscribe to SSE stream
    let stream_url = format!("http://127.0.0.1:{}/script_updates_test1", port);
    let mut event_source = reqwest::get(&stream_url).await.unwrap();

    // Trigger upsert via HTTP
    client.post(format!("http://127.0.0.1:{}/upsert_script", port))
        .form(&[("uri", "test.js"), ("content", "console.log('test');")])
        .send()
        .await
        .unwrap();

    // Now messages will arrive via SSE
    // ... receive and validate messages ...

    context.cleanup().await.expect("Failed to cleanup");
}
```

**Can Fix**: ✅ YES - Requirement REQ-TEST-007 specifies proper test infrastructure, tests need to follow the pattern

---

## Requirements Assessment

### Requirements Quality: ✅ GOOD

The REQUIREMENTS.md document is **comprehensive and sufficient** for fixing all failures:

1. **Security Requirements** (REQ-AUTH-006): ✅ Clear
   - Explicitly states anonymous users should NOT have write capabilities
   - Clearly defines capability model

2. **JavaScript API Requirements** (REQ-JS-006): ✅ Clear
   - Explicitly defines return types for `getScript()` (null on failure)
   - Explicitly defines return types for `deleteScript()` (boolean)
   - Provides clear API contract

3. **Test Infrastructure Requirements** (REQ-TEST-007): ✅ Clear
   - Requires proper server lifecycle management
   - Requires test isolation and cleanup
   - Provides guidance on test structure

4. **Streaming Requirements** (REQ-STREAM-004): ✅ Clear
   - Defines SSE protocol requirements
   - Specifies message format standards
   - Defines connection lifecycle

### Requirements Gaps: NONE IDENTIFIED

All failing tests have corresponding requirements that, if followed correctly, would make the tests pass.

---

## Recommendations

### Immediate Actions Required:

1. **Fix Anonymous User Capabilities** (CRITICAL)
   - File: `src/security/capabilities.rs`
   - Remove `WriteScripts` from anonymous capabilities
   - Impact: 1 test will pass
   - Security: CRITICAL FIX

2. **Fix getScript() Return Type** (HIGH)
   - File: `src/security/secure_globals.rs` AND `src/security/secure_globals_simple.rs`
   - Return `Option<String>` or JS null instead of error strings
   - Impact: 3 tests will pass
   - Breaking Change: YES - JavaScript code may need updates

3. **Fix deleteScript() Return Type** (HIGH)
   - File: `src/security/secure_globals.rs` AND `src/security/secure_globals_simple.rs`
   - Return `bool` instead of string messages
   - Impact: 3 tests will pass
   - Breaking Change: YES - JavaScript code may need updates

4. **Refactor Streaming Tests** (MEDIUM)
   - File: `tests/script_streaming_integration.rs`
   - Use `TestContext` pattern with actual HTTP server
   - Follow REQ-TEST-007 infrastructure requirements
   - Impact: 2 tests will pass

### Code Quality Improvements:

1. **Enforce REQ-DEV-006 Standards**:
   - Add pre-commit hooks to prevent security violations
   - Add linting rules to catch incorrect return types
   - Improve type safety in JS bindings

2. **Improve Test Documentation**:
   - Add comments linking tests to specific requirements
   - Document test patterns and infrastructure usage
   - Create test template files

3. **API Contract Validation**:
   - Add runtime validation that JS global functions follow contracts
   - Add integration tests that verify return types
   - Consider using TypeScript definitions for JS API

---

## Conclusion

**Answer to User Question**:

> "can these be fixed based on REQUIREMENTS.md and other documentation related to testing. or do we need better requirements?"

✅ **YES, these can ALL be fixed based on existing requirements.**

❌ **NO, we do NOT need better requirements.**

The REQUIREMENTS.md document is comprehensive and well-structured. The failures are due to:

1. **Implementation bugs** (security, return types)
2. **Test infrastructure issues** (not following established patterns)
3. **Incomplete implementation** (not following API contracts)

All necessary guidance exists in the requirements. The development team needs to:

1. Follow the security requirements strictly
2. Implement API contracts as specified
3. Use proper test infrastructure patterns
4. Review and enforce requirement compliance

### Next Steps:

1. Apply the 4 fixes listed above
2. Run tests again: `cargo nextest run --no-fail-fast`
3. All 6 tests should pass
4. Consider adding requirement traceability to prevent future deviations
