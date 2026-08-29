//! Ad-hoc benchmark for the prepared-program cache in `module_loader`.
//!
//! Not a correctness test — it measures the per-request cost of building an
//! asset-backed program with N imports, comparing the old behavior (rebuild the
//! bundle every request) against the new prepared-program cache. Run explicitly:
//!
//! ```bash
//! cargo nextest run --test prepare_cache_bench --no-capture -- --ignored
//! ```

mod common;

use common::setup_env;

use aiwebengine::js_engine::{RequestExecutionParams, execute_script_for_request_secure};
use aiwebengine::module_loader;
use aiwebengine::repository;
use aiwebengine::security::UserContext;
use std::collections::HashMap;
use std::time::Instant;

fn asset(script_uri: &str, uri: &str, content: String) -> repository::Asset {
    let now = std::time::SystemTime::now();
    repository::Asset {
        uri: uri.to_string(),
        name: Some(uri.to_string()),
        mimetype: "text/plain".to_string(),
        content: content.into_bytes(),
        created_at: now,
        updated_at: now,
        script_uri: script_uri.to_string(),
    }
}

/// A realistically-sized helper module (~1 KB of source) exporting one function.
fn helper_module(idx: usize) -> String {
    let filler =
        "// realistic helper body line kept around to reach a plausible file size\n".repeat(12);
    format!(
        "{filler}export function helper{idx}(target: string): string {{\n  \
         const parts = [\"{idx}\", target, String({idx} * 7)];\n  \
         return parts.join(\"-\");\n}}\n"
    )
}

fn root_script(n_imports: usize) -> String {
    let mut s = String::new();
    for i in 0..n_imports {
        s.push_str(&format!("import {{ helper{i} }} from \"./m{i}.ts\";\n"));
    }
    s.push_str("\nfunction benchHandler(context) {\n");
    s.push_str("  let acc = \"\";\n");
    for i in 0..n_imports {
        s.push_str(&format!("  acc += helper{i}(\"r\");\n"));
    }
    s.push_str("  return { status: 200, body: acc, contentType: \"text/plain\" };\n}\n");
    s
}

fn median(mut v: Vec<u128>) -> u128 {
    v.sort_unstable();
    v[v.len() / 2]
}

fn time_prepare(uri: &str, content: &str, iters: usize, clear_each: bool) -> u128 {
    let mut samples = Vec::with_capacity(iters);
    for _ in 0..iters {
        if clear_each {
            module_loader::clear(); // simulate old behavior: rebuild every request
        }
        let t = Instant::now();
        module_loader::prepare_executable_program(uri, content).expect("prepare");
        samples.push(t.elapsed().as_micros());
    }
    median(samples)
}

fn one_request(uri: &str) {
    let _ = execute_script_for_request_secure(RequestExecutionParams {
        script_uri: uri.to_string(),
        handler_name: "benchHandler".to_string(),
        path: "/bench".to_string(),
        method: "GET".to_string(),
        url: None,
        query_params: None,
        form_data: None,
        raw_body: None,
        headers: HashMap::new(),
        user_context: UserContext::authenticated("bench-user".to_string()),
        route_params: None,
        auth_context: None,
        uploaded_files: None,
        request_id: None,
        route_pattern: None,
    })
    .expect("request execution");
}

fn time_request(uri: &str, iters: usize, clear_prepare_each: bool) -> u128 {
    let mut samples = Vec::with_capacity(iters);
    for _ in 0..iters {
        if clear_prepare_each {
            module_loader::clear(); // old world: no prepared cache (bytecode/transpile stay warm)
        }
        let t = Instant::now();
        one_request(uri);
        samples.push(t.elapsed().as_micros());
    }
    median(samples)
}

/// Measure `fetch_script` (#2): the in-memory source-cache hit vs the old
/// per-request Postgres round-trip (forced by evicting the metadata entry).
fn time_fetch(uri: &str, iters: usize, evict_each: bool) -> u128 {
    let mut samples = Vec::with_capacity(iters);
    for _ in 0..iters {
        if evict_each && let Ok(mut guard) = repository::safe_lock_scripts() {
            guard.remove(uri); // old world: no source cache -> DB read
        }
        let t = Instant::now();
        let _ = repository::fetch_script(uri);
        samples.push(t.elapsed().as_micros());
    }
    median(samples)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "manual benchmark; needs Postgres"]
async fn bench_prepared_program_cache() {
    setup_env().await;

    println!("\n=== prepared-program cache benchmark (median µs) ===");
    println!(
        "{:>8} | {:>14} {:>14} {:>8} | {:>14} {:>14} {:>8}",
        "imports", "prepare_old", "prepare_new", "speedup", "request_old", "request_new", "speedup"
    );
    println!("{}", "-".repeat(96));

    for &n in &[0usize, 1, 5, 10, 20, 40] {
        let uri = format!("test://bench-imports-{n}");
        let content = root_script(n);

        // Seed root script + N asset modules.
        repository::upsert_script(&uri, &content).expect("store root script");
        for i in 0..n {
            repository::upsert_asset(asset(&uri, &format!("m{i}.ts"), helper_module(i)))
                .expect("store asset");
        }

        // Warm the transpiler + bytecode caches once (both existed before this
        // change) and populate the in-memory script-source cache the way a
        // route-index rebuild does in production, so the request measurements
        // isolate the prepared-program cache (#3) rather than the source
        // fetch (#2), which is measured separately below.
        module_loader::clear();
        let _ = repository::get_script_metadata(&uri);
        one_request(&uri);

        let prepare_old = time_prepare(&uri, &content, 200, true);
        let prepare_new = time_prepare(&uri, &content, 200, false);
        let request_old = time_request(&uri, 100, true);
        let request_new = time_request(&uri, 100, false);

        let ps = prepare_old as f64 / prepare_new.max(1) as f64;
        let rs = request_old as f64 / request_new.max(1) as f64;
        println!(
            "{:>8} | {:>14} {:>14} {:>7.1}x | {:>14} {:>14} {:>7.1}x",
            n, prepare_old, prepare_new, ps, request_old, request_new, rs
        );

        // Cleanup
        for i in 0..n {
            repository::delete_asset(&uri, &format!("m{i}.ts"));
        }
        let _ = repository::delete_script(&uri);
    }
    println!();

    // #2: script-source fetch — in-memory cache hit vs Postgres round-trip.
    let uri = "test://bench-source-fetch";
    repository::upsert_script(
        uri,
        "function h(context){ return { status:200, body:\"ok\" }; }",
    )
    .expect("store script");
    let _ = repository::get_script_metadata(uri); // populate source cache

    let fetch_db = time_fetch(uri, 500, true); // evict each iter -> DB read
    let _ = repository::get_script_metadata(uri); // repopulate (fetch_db left it evicted)
    let fetch_cache = time_fetch(uri, 500, false); // steady-state cache hit
    println!("=== script-source fetch (#2), median µs ===");
    println!(
        "fetch_from_db = {}  fetch_from_cache = {}  ({:.1}x)",
        fetch_db,
        fetch_cache,
        fetch_db as f64 / fetch_cache.max(1) as f64
    );
    let _ = repository::delete_script(uri);
    println!();
}
