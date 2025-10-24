# HTTP Fetch Test Mock Server Implementation

## Problem

The HTTP fetch tests were experiencing failures and flakiness due to reliance on external services (httpbin.org):

```
FLAKY 3/3 [  21.979s] aiwebengine::http_fetch test_fetch_post_with_json
FLAKY 3/3 [  20.850s] aiwebengine::http_fetch test_fetch_response_headers
FLAKY 3/3 [  20.108s] aiwebengine::http_fetch test_fetch_secret_template_syntax
TRY 3 FAIL [  13.157s] aiwebengine::http_fetch test_fetch_404_not_found
TRY 3 FAIL [  11.408s] aiwebengine::http_fetch test_fetch_custom_headers
TRY 3 FAIL [  10.589s] aiwebengine::http_fetch test_fetch_different_methods
TRY 3 FAIL [  10.818s] aiwebengine::http_fetch test_fetch_get_request
```

Issues with external service dependency:

- Network unreliability
- Rate limiting
- Service outages
- Slow response times
- Non-deterministic behavior

## Solution

Implemented a local mock HTTP server using Axum to replace the dependency on httpbin.org.

### Components Created

#### 1. Mock Server (`tests/mock_server.rs`)

A lightweight HTTP server that mimics httpbin.org's behavior:

**Features:**

- Random port selection (avoids port conflicts)
- Graceful shutdown
- Echo endpoints for testing HTTP methods
- Header reflection
- Status code simulation
- Query parameter handling

**Endpoints:**

- `GET /get` - Echo GET request with headers and query params
- `POST /post` - Echo POST request with body and JSON parsing
- `PUT /put` - Echo PUT request
- `DELETE /delete` - Echo DELETE request
- `PATCH /patch` - Echo PATCH request
- `GET /headers` - Return request headers
- `GET /response-headers?key=value` - Return custom response headers
- `GET /status/{code}` - Return specific HTTP status code

**API:**

```rust
let mock = MockServer::start().await?;
let url = mock.url("/get");
// ... make requests ...
mock.shutdown().await;
```

#### 2. HttpClient Test Mode

Added `new_for_tests()` constructor that:

- Allows connections to localhost/127.0.0.1
- Allows connections to private IP addresses
- Accepts self-signed certificates
- Maintains all other security checks (scheme validation, etc.)

**Changes to `src/http_client.rs`:**

```rust
pub struct HttpClient {
    // ...
    allow_private: bool,  // New field
}

impl HttpClient {
    // Existing production constructor
    pub fn new() -> Result<Self, HttpError> { /* ... */ }

    // New test-only constructor
    #[doc(hidden)]
    pub fn new_for_tests() -> Result<Self, HttpError> { /* ... */ }
}
```

**URL Validation:**

- Production: `validate_url()` - Blocks localhost and private IPs
- Testing: `validate_url_test()` - Allows localhost and private IPs

#### 3. Updated Tests

All tests that previously used httpbin.org now use the mock server:

- Wrapped blocking HTTP calls in `tokio::task::spawn_blocking`
- Use `HttpClient::new_for_tests()` for mock server tests
- Use `HttpClient::new()` for security validation tests

## Results

### Before

- 8 failing/flaky tests
- ~20 seconds per test (network latency)
- Non-deterministic failures

### After

- **All 17 tests passing consistently**
- ~9 seconds for full test suite
- Deterministic, reliable execution
- 5 consecutive runs: 100% pass rate

```
Run 1: 17 passed; 0 failed (9.03s)
Run 2: 17 passed; 0 failed (8.83s)
Run 3: 17 passed; 0 failed (9.18s)
Run 4: 17 passed; 0 failed (9.22s)
Run 5: 17 passed; 0 failed (9.16s)
```

## Benefits

1. **Reliability**: No external dependencies means no network failures
2. **Speed**: Local server is much faster than internet requests
3. **Determinism**: Predictable behavior makes debugging easier
4. **Isolation**: Tests don't affect or get affected by external services
5. **Cost**: No concerns about API rate limits or quotas
6. **Offline**: Tests work without internet connection
7. **Security**: Test mode properly isolated from production code

## Testing Security

The solution maintains security by:

- Test mode is explicitly marked `#[doc(hidden)]`
- Production code still uses strict validation
- Security validation tests still use `HttpClient::new()`
- `allow_private` flag clearly separates concerns
- No feature flags that could accidentally leak into production

## Usage

### Running Tests

```bash
# Run all HTTP fetch tests
cargo test --test http_fetch

# Run specific test
cargo test --test http_fetch test_fetch_get_request

# Run with output
cargo test --test http_fetch -- --nocapture
```

### Adding New Tests

```rust
#[tokio::test]
async fn test_new_feature() {
    let mock = MockServer::start().await.expect("Failed to start mock server");
    let url = mock.url("/endpoint");

    let result = tokio::task::spawn_blocking(move || {
        let client = HttpClient::new_for_tests()
            .expect("Failed to create client");
        client.fetch(url, FetchOptions::default())
    })
    .await
    .expect("Task panicked");

    assert!(result.is_ok());
    mock.shutdown().await;
}
```

## Future Improvements

Potential enhancements:

1. Add request recording/verification for more complex assertions
2. Support for streaming responses
3. Configurable delays to simulate slow networks
4. Support for testing timeout behavior
5. Mock HTTPS endpoints with self-signed certificates
6. Request count assertions
7. Concurrent request handling tests

## Files Modified

- `tests/mock_server.rs` (new) - Mock server implementation
- `tests/http_fetch.rs` - Updated to use mock server
- `src/http_client.rs` - Added test mode support

## Conclusion

The mock server implementation successfully eliminated all flaky tests and reduced test execution time by ~50%. The solution maintains security best practices while providing a reliable testing environment.
