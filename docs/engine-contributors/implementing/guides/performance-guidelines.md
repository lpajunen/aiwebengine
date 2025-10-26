# Performance Guidelines

**Last Updated:** October 24, 2025

Writing efficient code for aiwebengine.

---

## Overview

This guide covers performance best practices for aiwebengine development. While correctness and security are paramount, performance matters for user experience and resource efficiency.

**Guiding Principles:**

1. **Correctness First** - Don't sacrifice correctness for speed
2. **Measure, Don't Guess** - Profile before optimizing
3. **Optimize Smartly** - Focus on hot paths and bottlenecks
4. **Readable Performance** - Keep code maintainable

---

## Performance Targets

### Response Time Goals

| Endpoint Type       | Target | Maximum |
| ------------------- | ------ | ------- |
| Health checks       | <10ms  | 50ms    |
| Static files        | <50ms  | 200ms   |
| Script registration | <100ms | 500ms   |
| Script execution    | <200ms | 2s      |
| GraphQL queries     | <100ms | 1s      |

### Resource Limits

- **Memory**: Process should stay under 512MB under normal load
- **CPU**: Single script execution should use <50% CPU
- **Concurrency**: Support 100 concurrent requests
- **Throughput**: 1000 req/s for simple operations

---

## Async Programming Best Practices

### Use Async Appropriately

**✅ GOOD: Async for I/O:**

```rust
pub async fn load_script(path: &Path) -> Result<String, IoError> {
    // I/O-bound: use async
    tokio::fs::read_to_string(path).await
}

pub async fn fetch_remote_config(url: &str) -> Result<Config, HttpError> {
    // Network I/O: use async
    reqwest::get(url).await?.json().await
}
```

**❌ BAD: Async for CPU-bound:**

```rust
pub async fn calculate_hash(data: &[u8]) -> String {
    // CPU-bound: doesn't benefit from async
    // This blocks the async runtime
    let hash = sha256::digest(data);
    hash
}
```

**✅ BETTER: Spawn blocking for CPU-bound:**

```rust
pub async fn calculate_hash(data: Vec<u8>) -> Result<String, TaskError> {
    tokio::task::spawn_blocking(move || {
        sha256::digest(&data)
    })
    .await
    .map_err(|e| TaskError::JoinFailed(e))
}
```

### Avoid Blocking the Runtime

**❌ BAD:**

```rust
pub async fn process_request() -> Response {
    // NEVER do this - blocks async runtime!
    std::thread::sleep(Duration::from_secs(1));

    // Also bad - synchronous I/O
    let data = std::fs::read_to_string("file.txt").unwrap();

    Response::ok()
}
```

**✅ GOOD:**

```rust
pub async fn process_request() -> Response {
    // Use async sleep
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Use async I/O
    let data = tokio::fs::read_to_string("file.txt").await.unwrap();

    Response::ok()
}
```

### Concurrent Operations

**✅ GOOD: Concurrent execution:**

```rust
pub async fn load_all_configs() -> Result<Vec<Config>, Error> {
    let config_paths = vec![
        "config1.toml",
        "config2.toml",
        "config3.toml",
    ];

    // Load concurrently
    let futures: Vec<_> = config_paths
        .iter()
        .map(|path| load_config(path))
        .collect();

    // Wait for all
    let results = futures::future::try_join_all(futures).await?;
    Ok(results)
}
```

**❌ BAD: Sequential execution:**

```rust
pub async fn load_all_configs() -> Result<Vec<Config>, Error> {
    let mut configs = Vec::new();

    // Loads one at a time - slow!
    configs.push(load_config("config1.toml").await?);
    configs.push(load_config("config2.toml").await?);
    configs.push(load_config("config3.toml").await?);

    Ok(configs)
}
```

---

## Memory Management

### Avoid Unnecessary Allocations

**✅ GOOD: Reuse allocations:**

```rust
pub fn filter_scripts(scripts: &[Script], filter: &str) -> Vec<&Script> {
    // Returns references - no cloning
    scripts.iter()
        .filter(|s| s.name.contains(filter))
        .collect()
}

pub fn process_items(items: &mut Vec<Item>) {
    // Modify in place
    items.retain(|item| item.is_valid());
    items.sort_by_key(|item| item.priority);
}
```

**❌ BAD: Unnecessary allocations:**

```rust
pub fn filter_scripts(scripts: &[Script], filter: &str) -> Vec<Script> {
    // Clones every script!
    scripts.iter()
        .filter(|s| s.name.contains(filter))
        .cloned()
        .collect()
}

pub fn process_items(items: Vec<Item>) -> Vec<Item> {
    // Creates new vector
    let valid: Vec<Item> = items.into_iter()
        .filter(|item| item.is_valid())
        .collect();

    // Creates another new vector
    let mut sorted = valid.clone();
    sorted.sort_by_key(|item| item.priority);
    sorted
}
```

### Use Cow for Conditional Cloning

**✅ GOOD:**

```rust
use std::borrow::Cow;

pub fn normalize_path(path: &str) -> Cow<str> {
    if path.starts_with('/') {
        // No allocation needed
        Cow::Borrowed(path)
    } else {
        // Only allocate when necessary
        Cow::Owned(format!("/{}", path))
    }
}
```

### String Building

**✅ GOOD: Pre-allocate capacity:**

```rust
pub fn build_large_string(items: &[Item]) -> String {
    let capacity = items.len() * 50; // Estimate
    let mut result = String::with_capacity(capacity);

    for item in items {
        result.push_str(&item.name);
        result.push(',');
    }

    result
}
```

**❌ BAD: Multiple reallocations:**

```rust
pub fn build_large_string(items: &[Item]) -> String {
    let mut result = String::new(); // Starts empty, reallocates repeatedly

    for item in items {
        result.push_str(&item.name);
        result.push(',');
    }

    result
}
```

---

## Data Structures

### Choose Appropriate Collections

**Hash Maps vs BTreeMap:**

```rust
// Use HashMap for most cases (O(1) average)
use std::collections::HashMap;
let mut cache: HashMap<String, Value> = HashMap::new();

// Use BTreeMap when you need ordering or range queries
use std::collections::BTreeMap;
let mut ordered_data: BTreeMap<i32, Value> = BTreeMap::new();
```

**Vec vs VecDeque:**

```rust
// Use Vec for most cases
let mut items: Vec<Item> = Vec::new();
items.push(item); // Efficient

// Use VecDeque for frequent push/pop from both ends
use std::collections::VecDeque;
let mut queue: VecDeque<Task> = VecDeque::new();
queue.push_front(task); // Efficient
queue.pop_back(); // Efficient
```

**HashSet vs Vec:**

```rust
// Use HashSet for membership testing
use std::collections::HashSet;
let mut seen: HashSet<String> = HashSet::new();
if seen.contains(&key) { /* O(1) */ }

// Don't use Vec for membership testing
let mut seen: Vec<String> = Vec::new();
if seen.contains(&key) { /* O(n) - slow! */ }
```

### Pre-allocate When Size is Known

**✅ GOOD:**

```rust
pub fn process_batch(count: usize) -> Vec<Result> {
    let mut results = Vec::with_capacity(count);

    for i in 0..count {
        results.push(process_item(i));
    }

    results
}
```

---

## Caching

### Cache Expensive Operations

**✅ GOOD:**

```rust
use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::HashMap;

pub struct ScriptCache {
    compiled: Arc<RwLock<HashMap<String, CompiledScript>>>,
}

impl ScriptCache {
    pub async fn get_or_compile(&self, path: &str) -> Result<CompiledScript, Error> {
        // Try read lock first (cheap)
        {
            let cache = self.compiled.read().await;
            if let Some(script) = cache.get(path) {
                return Ok(script.clone());
            }
        }

        // Compile (expensive)
        let script = compile_script(path).await?;

        // Write lock to store
        {
            let mut cache = self.compiled.write().await;
            cache.insert(path.to_string(), script.clone());
        }

        Ok(script)
    }
}
```

### Cache Invalidation

**✅ GOOD:**

```rust
pub struct ScriptCache {
    compiled: Arc<RwLock<HashMap<String, CachedScript>>>,
}

struct CachedScript {
    script: CompiledScript,
    modified_time: SystemTime,
}

impl ScriptCache {
    pub async fn get_or_compile(&self, path: &Path) -> Result<CompiledScript, Error> {
        let metadata = tokio::fs::metadata(path).await?;
        let modified = metadata.modified()?;

        // Check cache with freshness validation
        {
            let cache = self.compiled.read().await;
            if let Some(cached) = cache.get(path.to_str().unwrap()) {
                if cached.modified_time >= modified {
                    return Ok(cached.script.clone());
                }
            }
        }

        // Compile and cache
        let script = compile_script(path).await?;

        {
            let mut cache = self.compiled.write().await;
            cache.insert(
                path.to_str().unwrap().to_string(),
                CachedScript {
                    script: script.clone(),
                    modified_time: modified,
                },
            );
        }

        Ok(script)
    }
}
```

---

## Database Optimization (Future)

When database support is added:

### Use Prepared Statements

```rust
// Prepared once, reused many times
let stmt = client.prepare("SELECT * FROM scripts WHERE user_id = $1").await?;

for user_id in user_ids {
    let scripts = client.query(&stmt, &[&user_id]).await?;
    // ...
}
```

### Batch Operations

```rust
// Bad: N queries
for script in scripts {
    db.save_script(script).await?;
}

// Good: Bulk insert
db.save_scripts_batch(&scripts).await?;
```

### Use Indexes

```sql
-- Add indexes for frequently queried columns
CREATE INDEX idx_scripts_user_id ON scripts(user_id);
CREATE INDEX idx_scripts_path ON scripts(path);
```

### Limit Query Results

```rust
// Always use LIMIT for potentially large results
let scripts = sqlx::query_as!(
    Script,
    "SELECT * FROM scripts WHERE user_id = $1 LIMIT 100",
    user_id
)
.fetch_all(&pool)
.await?;
```

---

## HTTP Performance

### Connection Pooling

**✅ GOOD:**

```rust
use reqwest::Client;

// Create once, reuse
static CLIENT: Lazy<Client> = Lazy::new(|| {
    Client::builder()
        .pool_max_idle_per_host(10)
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap()
});

pub async fn fetch_data(url: &str) -> Result<Data, Error> {
    CLIENT.get(url).send().await?.json().await
}
```

**❌ BAD:**

```rust
// Creates new connection every time!
pub async fn fetch_data(url: &str) -> Result<Data, Error> {
    let client = reqwest::Client::new();
    client.get(url).send().await?.json().await
}
```

### Timeouts

**Always set timeouts:**

```rust
let response = CLIENT
    .get(url)
    .timeout(Duration::from_secs(10))
    .send()
    .await?;
```

---

## Iteration Optimization

### Use Iterators

**✅ GOOD:**

```rust
pub fn sum_values(items: &[Item]) -> i32 {
    items.iter()
        .filter(|item| item.is_active)
        .map(|item| item.value)
        .sum()
}
```

**❌ BAD:**

```rust
pub fn sum_values(items: &[Item]) -> i32 {
    let mut sum = 0;
    let mut active = Vec::new();

    for item in items {
        if item.is_active {
            active.push(item);
        }
    }

    for item in active {
        sum += item.value;
    }

    sum
}
```

### Early Exit

**✅ GOOD:**

```rust
pub fn find_invalid(items: &[Item]) -> Option<&Item> {
    items.iter().find(|item| !item.is_valid())
}

pub fn all_valid(items: &[Item]) -> bool {
    items.iter().all(|item| item.is_valid())
}
```

---

## Profiling & Measurement

### Benchmarking

**Use criterion for benchmarks:**

```rust
use criterion::{black_box, criterion_group, criterion_main, Criterion};

fn benchmark_script_compilation(c: &mut Criterion) {
    let script = load_test_script();

    c.bench_function("compile script", |b| {
        b.iter(|| compile_script(black_box(&script)))
    });
}

criterion_group!(benches, benchmark_script_compilation);
criterion_main!(benches);
```

### Flamegraphs

```bash
# Install cargo-flamegraph
cargo install flamegraph

# Generate flamegraph
cargo flamegraph --bin aiwebengine

# Open flamegraph.svg to see hot paths
```

### Memory Profiling

```bash
# Use valgrind/massif
valgrind --tool=massif target/release/aiwebengine

# Or use heaptrack
heaptrack target/release/aiwebengine
```

---

## Common Anti-Patterns

### ❌ Premature Optimization

```rust
// Don't do this without profiling first!
pub fn calculate_score(items: &[Item]) -> f64 {
    // Overly complex "optimization" that's hard to read
    unsafe {
        let ptr = items.as_ptr();
        let mut sum = 0.0;
        for i in 0..items.len() {
            sum += (*ptr.add(i)).score;
        }
        sum / items.len() as f64
    }
}

// Just write clear code
pub fn calculate_score(items: &[Item]) -> f64 {
    items.iter().map(|i| i.score).sum::<f64>() / items.len() as f64
}
```

### ❌ Excessive Cloning

```rust
// Bad: Clones everywhere
pub fn process(data: &Data) -> Result<Output, Error> {
    let d1 = data.clone();
    let d2 = d1.clone();
    let d3 = d2.clone();
    compute(d3)
}

// Good: Use references
pub fn process(data: &Data) -> Result<Output, Error> {
    compute(data)
}
```

### ❌ String Concatenation in Loops

```rust
// Bad: Quadratic time complexity
pub fn build_csv(items: &[Item]) -> String {
    let mut csv = String::new();
    for item in items {
        csv = csv + &item.to_csv() + "\n"; // Allocates new string each time!
    }
    csv
}

// Good: Linear time
pub fn build_csv(items: &[Item]) -> String {
    items.iter()
        .map(|item| item.to_csv())
        .collect::<Vec<_>>()
        .join("\n")
}
```

---

## Performance Checklist

Before merging performance-sensitive code:

- [ ] Async used for I/O, not CPU-bound work
- [ ] No blocking calls in async functions
- [ ] Collections pre-allocated when size known
- [ ] Appropriate data structures chosen
- [ ] Unnecessary cloning avoided
- [ ] Caching implemented for expensive operations
- [ ] Iterators used instead of manual loops
- [ ] Timeouts set on external calls
- [ ] Benchmarks added for critical paths
- [ ] Profiling done to verify improvements

---

## When to Optimize

**Optimize When:**

- Profiling shows a bottleneck
- User-facing latency is too high
- Resource usage exceeds targets
- Load testing reveals issues

**Don't Optimize When:**

- Code isn't working yet
- No measurements exist
- Code is not in hot path
- Optimization hurts readability significantly

---

## Resources

- [Rust Performance Book](https://nnethercote.github.io/perf-book/)
- [Tokio Performance Guide](https://tokio.rs/tokio/topics/performance)
- [Criterion Benchmarking](https://bheisler.github.io/criterion.rs/book/)

---

_"Premature optimization is the root of all evil" - Donald Knuth_

_But measured, profiled optimization is engineering._
