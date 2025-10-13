# Test Performance Optimization Guide

## 🐌 Problems Identified

### 1. **Leaked Server Instances**
- `start_server_without_shutdown()` uses `Box::leak()` to prevent shutdown
- Each test spawns a server that runs until process termination
- Result: dozens of zombie servers consuming resources

### 2. **Excessive Sleep Delays**
```rust
// Found in multiple test files:
tokio::time::sleep(Duration::from_secs(30)).await;  // 30 seconds!
tokio::time::sleep(Duration::from_secs(10)).await;  // 10 seconds!
```

### 3. **No Test Isolation**
- Shared global state (repository, stream registry)
- Port conflicts
- No cleanup between tests

### 4. **Serial Execution**
- Tests run sequentially by default
- Total runtime = sum of all delays

## ✅ Solutions Implemented

### 1. **Proper Server Lifecycle Management**

Created new `tests/common/mod.rs` with:
- `TestServer` with graceful shutdown support
- `TestContext` for managing multiple servers
- `wait_for_server()` with retry logic (replaces long sleeps)

### 2. **Faster Test Configuration**

**`.cargo/config.toml`**: Optimized build settings
**`.config/nextest.toml`**: Configure nextest for parallel execution with timeouts

### 3. **How to Use**

#### Install nextest (recommended):
```bash
cargo install cargo-nextest
```

#### Run tests faster:
```bash
# Using nextest (parallel, with timeouts)
cargo nextest run

# Traditional (still better than before)
cargo test

# Run specific test
cargo nextest run test_health_endpoint
```

## 🔧 Migration Steps

### Step 1: Update Integration Tests

Replace this pattern:
```rust
#[tokio::test]
async fn test_something() {
    let port = start_server_without_shutdown().await.unwrap();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
    });
    tokio::time::sleep(Duration::from_millis(1000)).await;
    // test code
}
```

With this:
```rust
mod common;

#[tokio::test]
async fn test_something() {
    with_test_server!(|port| async move {
        let client = reqwest::Client::new();
        // test code - server is ready
        Ok(())
    }).unwrap();
}
```

### Step 2: Update Sleep Durations

Change all long sleeps to shorter waits:
- `sleep(Duration::from_secs(30))` → `sleep(Duration::from_millis(100))`
- `sleep(Duration::from_secs(10))` → removed (use `wait_for_server()`)
- `sleep(Duration::from_secs(1))` → `sleep(Duration::from_millis(100))`

### Step 3: Run Quick Test
```bash
# Kill any hanging processes first
pkill -9 aiwebengine

# Run unit tests only (should be fast)
cargo test --lib --bins

# Run with nextest
cargo nextest run
```

## 📊 Expected Improvements

### Before:
- Integration tests: **5+ minutes** or hang indefinitely
- Each test: 10-30 seconds of sleep
- No parallelization
- Resource leaks

### After:
- Integration tests: **< 30 seconds** with nextest
- Each test: < 2 seconds
- Parallel execution (4 threads)
- Proper cleanup

## 🔍 Debugging Hanging Tests

If tests still hang:

```bash
# Find hanging processes
ps aux | grep aiwebengine

# Kill them
pkill -9 aiwebengine

# Run with verbose logging
RUST_LOG=debug cargo nextest run

# Run single test with timeout
cargo nextest run test_concurrent_session_limit --no-capture
```

## 🎯 Quick Fixes for Specific Issues

### `test_concurrent_session_limit` hangs
This test likely has a deadlock. Check:
1. Lock contention in session manager
2. Async runtime issues
3. Port conflicts

Add timeout:
```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_concurrent_session_limit() {
    tokio::time::timeout(
        Duration::from_secs(5),
        async {
            // test code
        }
    ).await.expect("Test timed out");
}
```

### General hanging
1. Check for `.await` without timeout
2. Look for blocking operations in async code
3. Verify no infinite loops or retries

## 📝 Additional Optimizations

### 1. Use test features in Cargo.toml
```toml
[dev-dependencies]
serial_test = "3.0"  # For tests that must run serially

# Usage:
use serial_test::serial;

#[tokio::test]
#[serial]
async fn test_shared_resource() {
    // This runs serially
}
```

### 2. Mock expensive operations
```toml
[dev-dependencies]
mockall = "0.13.1"  # Already added
wiremock = "0.6"    # For HTTP mocking
```

### 3. Feature flags for slow tests
```toml
[features]
slow-tests = []

# Run normally
cargo test

# Include slow tests
cargo test --features slow-tests
```

## 🚀 Next Steps

1. ✅ Update `tests/health_integration.rs` first (has 30s sleeps)
2. ✅ Update other integration tests systematically
3. ✅ Add cleanup in test teardown
4. ✅ Consider using test fixtures for common setup
5. ✅ Add CI configuration with nextest
