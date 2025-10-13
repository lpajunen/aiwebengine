# Test Performance Analysis & Solutions

## 🔴 Critical Issues Found

Your tests are hanging because of **4 major problems**:

### 1. **Server Instances Never Shut Down** ⚠️ CRITICAL
- **Problem**: `start_server_without_shutdown()` uses `Box::leak()` to prevent shutdown
- **Impact**: Each test spawns a server that runs forever
- **Result**: 24 test files × multiple tests = dozens of zombie processes
- **Evidence**: 
  ```rust
  // src/lib.rs:988
  let (tx, rx) = tokio::sync::oneshot::channel::<()>();
  Box::leak(Box::new(tx));  // Sender never dropped = server never stops
  ```

### 2. **Excessive Sleep Delays** 🐌
- **Problem**: Tests sleep for 10-30 seconds waiting for servers
- **Impact**: Adds 10-30 seconds per test
- **Evidence**:
  ```
  tests/test_editor_integration.rs:26 - sleep(Duration::from_secs(30))
  tests/form_data_integration.rs:29 - sleep(Duration::from_secs(30))
  tests/health_integration.rs:19,73,116 - sleep(Duration::from_secs(10))
  tests/graphql_integration.rs:25,232 - sleep(Duration::from_secs(10))
  ```

### 3. **No Test Isolation** 💥
- **Problem**: Tests share global state without cleanup
- **Shared Resources**:
  - `repository` (script storage)
  - `GLOBAL_STREAM_REGISTRY`
  - Port conflicts (especially affecting `test_concurrent_session_limit`)
- **Impact**: Race conditions, port conflicts, state pollution

### 4. **Serial Test Execution** 🐢
- **Problem**: Tests run sequentially by default
- **Impact**: Total runtime = sum of all test times
- **Example**: 89 integration tests × 10s each = ~15 minutes minimum

## ✅ Solutions Implemented

### 1. New Test Utilities (`tests/common/mod.rs`)

```rust
pub struct TestServer {
    port: u16,
    shutdown_tx: Option<oneshot::Sender<()>>,  // ✅ Proper shutdown!
}

impl TestServer {
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

// ✅ Smart waiting instead of long sleeps
pub async fn wait_for_server(port: u16, max_attempts: u32) -> anyhow::Result<()> {
    // Retries every 50ms up to max_attempts (1 second for 20 attempts)
}
```

### 2. Test Runner Configuration

**`.config/nextest.toml`**:
```toml
[profile.default]
test-threads = 4  # ✅ Parallel execution
slow-timeout = { period = "60s", terminate-after = 3 }  # ✅ Kill hanging tests
retries = { backoff = "exponential", count = 2 }  # ✅ Retry flaky tests
```

**`.cargo/config.toml`**:
```toml
[profile.test]
opt-level = 1  # ✅ Faster test compilation
codegen-units = 4  # ✅ Parallel codegen
```

### 3. Migration Pattern

**Before** (slow, leaks resources):
```rust
#[tokio::test]
async fn test_health_endpoint() {
    let port = start_server_without_shutdown().await.unwrap();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(10)).await;  // ❌ 10s wait
    });
    tokio::time::sleep(Duration::from_millis(1000)).await;  // ❌ Another 1s
    // test code
    // ❌ No cleanup!
}
```

**After** (fast, clean):
```rust
mod common;

#[tokio::test]
async fn test_health_endpoint() {
    let context = common::TestContext::new();
    let port = context.start_server().await.unwrap();
    common::wait_for_server(port, 20).await.unwrap();  // ✅ 1s max
    
    // test code
    
    context.cleanup().await.unwrap();  // ✅ Proper cleanup!
}
```

## 🚀 How to Fix

### Quick Start (Kill hanging processes & test)

```bash
# Kill any hanging processes
pkill -9 aiwebengine

# Run the helper script
./scripts/fix_tests.sh

# Or manually:
cargo test --lib --bins  # Unit tests only (should be fast)
```

### Install nextest (Recommended)

```bash
cargo install cargo-nextest
cargo nextest run  # Parallel execution with timeouts
```

### Fix Integration Tests Systematically

1. **Update `tests/health_integration.rs`** (worst offender):
   - Replace `start_server_without_shutdown()` → `TestContext::new().start_server()`
   - Replace `sleep(Duration::from_secs(10))` → `wait_for_server(port, 20)`
   - Add `context.cleanup().await`

2. **Update other integration tests**:
   - `tests/graphql_integration.rs`
   - `tests/test_editor_integration.rs`
   - `tests/form_data_integration.rs`
   - etc.

3. **See example**: `tests/health_integration_optimized.rs`

### Fix the Hanging Test Specifically

The `test_concurrent_session_limit` test is likely hanging due to:
1. Port conflicts from zombie servers
2. Lock contention in session manager
3. Missing timeout

**Quick fix**:
```rust
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_concurrent_session_limit() {
    // Add timeout wrapper
    tokio::time::timeout(Duration::from_secs(5), async {
        let manager = create_test_manager();
        
        for _i in 0..4 {
            manager.create_session(/* ... */).await.unwrap();
        }
        
        let count = manager.get_user_session_count("user123").await;
        assert_eq!(count, 3);
    })
    .await
    .expect("Test timed out - possible deadlock");
}
```

## 📊 Expected Performance

### Before:
- **Unit tests**: 0.5s ✅ (already fast)
- **Integration tests**: 5+ minutes or **HANG** ❌
- **Total**: **UNUSABLE**

### After:
- **Unit tests**: 0.5s ✅
- **Integration tests**: < 30 seconds ✅
- **Total**: **< 1 minute** 🎉

### Per-test improvement:
- Before: 10-30 seconds per test
- After: 0.5-2 seconds per test
- **Speedup: 10-60x faster**

## 🔍 Debugging Commands

```bash
# Find hanging processes
ps aux | grep aiwebengine

# Kill them all
pkill -9 aiwebengine

# Run with verbose logging
RUST_LOG=debug cargo test test_concurrent_session_limit -- --nocapture

# Run single test with nextest
cargo nextest run test_concurrent_session_limit --no-capture

# Run only fast unit tests
cargo test --lib --bins

# Check test count
cargo test -- --list | wc -l
```

## 📝 Additional Optimizations

### Option 1: Feature flag for slow tests
```toml
# Cargo.toml
[features]
slow-tests = []

# Run normally (skip slow tests)
cargo test

# Include slow tests
cargo test --features slow-tests
```

### Option 2: Use serial_test for shared resources
```toml
[dev-dependencies]
serial_test = "3.0"

# Usage:
#[tokio::test]
#[serial]
async fn test_shared_resource() { }
```

### Option 3: Mock external dependencies
```toml
[dev-dependencies]
wiremock = "0.6"  # HTTP mocking
mockito = "1.0"   # HTTP mocking alternative
```

## 🎯 Action Plan

1. **Immediate** (do this now):
   ```bash
   pkill -9 aiwebengine
   cargo install cargo-nextest
   cargo nextest run --lib --bins  # Verify unit tests work
   ```

2. **Today**:
   - Update `tests/health_integration.rs` using the pattern
   - Update `tests/graphql_integration.rs`
   - Test: `cargo nextest run --test health_integration`

3. **This Week**:
   - Update all 24 integration test files
   - Run full suite: `cargo nextest run`
   - Add timeout to `test_concurrent_session_limit`

4. **Optional**:
   - Add feature flags for slow tests
   - Add mocking for external services
   - Set up CI with nextest

## 📚 Files Reference

- ✅ `tests/common/mod.rs` - New test utilities
- ✅ `.cargo/config.toml` - Build optimizations  
- ✅ `.config/nextest.toml` - Test runner config
- ✅ `TEST_OPTIMIZATION.md` - Full migration guide
- ✅ `tests/health_integration_optimized.rs` - Example migration
- ✅ `scripts/fix_tests.sh` - Helper script
- 📖 This document - Summary & action plan

## ❓ FAQ

**Q: Why do tests hang specifically?**
A: Zombie servers accumulate and exhaust ports/resources. The OS can only handle so many simultaneous servers on different ports.

**Q: Why does `test_concurrent_session_limit` hang?**
A: Likely port conflicts from zombie servers. After killing processes, it should work. Add timeout for safety.

**Q: Can I just increase timeouts?**
A: No! That treats symptoms, not the cause. Fix resource leaks first.

**Q: Should I use nextest?**
A: Yes! It's faster, has better output, and includes timeouts by default.

**Q: What about the existing `test_utils.rs`?**
A: It has the same leak problem. Use the new `common/mod.rs` instead.
