//! QuickJS bytecode compilation cache.
//!
//! Each request executes in a fresh QuickJS runtime, so an owner script was
//! previously re-parsed from source on every invocation. Parsing the ~43KB
//! `core.js` costs ~1ms; executing pre-compiled bytecode costs ~0.2ms.
//!
//! This module compiles a *global* script to QuickJS bytecode once, caches the
//! serialized bytes keyed by (uri, source hash), and executes the bytecode on
//! subsequent runs. Bytecode is position-independent and carries its atom
//! table, so bytes produced in one runtime can be read into any other runtime
//! backed by the same QuickJS build. Global-script semantics are preserved:
//! `function foo(){}` declarations remain callable via `globalThis.foo`, which
//! the engine relies on for handler dispatch.
//!
//! # Safety
//!
//! rquickjs 0.11 exposes bytecode serialization only for ES modules, whose
//! scope semantics differ, so this uses the raw `qjs` FFI. The unsafe blocks
//! are confined here and audited for QuickJS's ownership rules:
//! `JS_WriteObject` borrows its value; `JS_EvalFunction` consumes it; the
//! buffer returned by `JS_WriteObject` is freed with `js_free`.

use moka::sync::Cache;
use rquickjs::{Ctx, qjs};
use sha2::{Digest, Sha256};
use std::sync::{Arc, OnceLock};
use tracing::debug;

#[derive(Clone)]
struct CachedBytecode {
    source_hash: String,
    /// Shared rather than owned: a hit hands the bytes straight to
    /// [`eval_bytecode`], and this runs on every request that executes a
    /// script. Cloning a `Vec<u8>` out of the cache copied the whole program's
    /// bytecode to read it once.
    bytecode: Arc<[u8]>,
}

/// Ceiling on the compiled bytecode held at once, in bytes.
///
/// Keys are script URIs and test-module paths, so the map this replaced grew
/// with the content of the database rather than without limit — but nothing
/// bounded it, and an engine hosting many scripts kept every program it had
/// ever run. Bytes rather than entries, because entries differ by two orders
/// of magnitude between a small handler and a bundled application.
const MAX_BYTECODE_BYTES: u64 = 32 * 1024 * 1024;

static BYTECODE_CACHE: OnceLock<Cache<String, CachedBytecode>> = OnceLock::new();

fn cache() -> &'static Cache<String, CachedBytecode> {
    BYTECODE_CACHE.get_or_init(|| {
        Cache::builder()
            .name("bytecode")
            .max_capacity(MAX_BYTECODE_BYTES)
            .weigher(|_key: &String, cached: &CachedBytecode| {
                cached.bytecode.len().try_into().unwrap_or(u32::MAX)
            })
            .build()
    })
}

fn hash_source(code: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(code.as_bytes());
    hex::encode(hasher.finalize())
}

/// Remove any cached bytecode for a script (call when its source changes).
pub fn invalidate(uri: &str) {
    cache().invalidate(uri);
}

/// Clear the entire bytecode cache.
///
/// Key by key rather than `invalidate_all`, which is applied lazily against
/// the time it was called: an entry stored in the same instant would be
/// ambiguous, and every caller here expects the cache to be cold on return.
pub fn clear() {
    let cache = cache();
    for (key, _) in cache.iter() {
        cache.invalidate(key.as_str());
    }
}

/// Compile global-script `code` and execute it in `ctx`, caching the compiled
/// bytecode under `cache_key` so later calls with identical source skip parsing.
///
/// Behaves like `ctx.eval::<(), _>(code)`: the program runs for its side
/// effects (defining global functions). On a JavaScript exception this returns
/// `Err(rquickjs::Error::Exception)` with the exception left pending on the
/// context, so existing `Ctx::catch`-based error extraction works unchanged.
pub fn eval_program(ctx: &Ctx<'_>, cache_key: &str, code: &str) -> Result<(), rquickjs::Error> {
    let source_hash = hash_source(code);

    let cached = cache()
        .get(cache_key)
        .filter(|entry| entry.source_hash == source_hash);

    if let Some(entry) = cached {
        debug!(uri = cache_key, "Bytecode cache hit");
        return eval_bytecode(ctx, &entry.bytecode);
    }

    debug!(uri = cache_key, "Bytecode cache miss; compiling");
    let bytecode: Arc<[u8]> = Arc::from(compile_to_bytecode(ctx, code, cache_key)?);

    cache().insert(
        cache_key.to_string(),
        CachedBytecode {
            source_hash,
            bytecode: Arc::clone(&bytecode),
        },
    );

    eval_bytecode(ctx, &bytecode)
}

/// Compile global-script source to serialized QuickJS bytecode without running
/// it. Returns `Err(Error::Exception)` on a compile error (left pending on the
/// context).
fn compile_to_bytecode(
    ctx: &Ctx<'_>,
    code: &str,
    filename: &str,
) -> Result<Vec<u8>, rquickjs::Error> {
    let raw = ctx.as_raw().as_ptr();
    // Filenames appear in stack traces only; a NUL is not worth failing on.
    let filename_c = std::ffi::CString::new(filename).unwrap_or_else(|_| c"script".to_owned());
    // Source may not contain interior NULs; QuickJS takes a length-prefixed
    // buffer but rquickjs uses CString, so match that constraint.
    let code_c = std::ffi::CString::new(code).map_err(|_| rquickjs::Error::Exception)?;

    unsafe {
        let flags = (qjs::JS_EVAL_TYPE_GLOBAL | qjs::JS_EVAL_FLAG_COMPILE_ONLY) as i32;
        let func = qjs::JS_Eval(
            raw,
            code_c.as_ptr(),
            code.len() as _,
            filename_c.as_ptr(),
            flags,
        );
        if qjs::JS_IsException(func) {
            // Exception is pending on the context for the caller to extract.
            return Err(rquickjs::Error::Exception);
        }

        let mut size: qjs::size_t = 0;
        let ptr = qjs::JS_WriteObject(
            raw,
            &mut size as *mut _,
            func,
            qjs::JS_WRITE_OBJ_BYTECODE as i32,
        );
        // JS_WriteObject borrows func; free it either way.
        if ptr.is_null() {
            qjs::JS_FreeValue(raw, func);
            return Err(rquickjs::Error::Exception);
        }
        let bytes = std::slice::from_raw_parts(ptr, size as usize).to_vec();
        qjs::js_free(raw, ptr as *mut std::ffi::c_void);
        qjs::JS_FreeValue(raw, func);
        Ok(bytes)
    }
}

/// Read serialized bytecode into `ctx` and execute it as global code.
fn eval_bytecode(ctx: &Ctx<'_>, bytecode: &[u8]) -> Result<(), rquickjs::Error> {
    let raw = ctx.as_raw().as_ptr();
    unsafe {
        let func = qjs::JS_ReadObject(
            raw,
            bytecode.as_ptr(),
            bytecode.len() as _,
            qjs::JS_READ_OBJ_BYTECODE as i32,
        );
        if qjs::JS_IsException(func) {
            return Err(rquickjs::Error::Exception);
        }

        // JS_EvalFunction consumes `func` (frees it), so it must not be freed
        // again here.
        let result = qjs::JS_EvalFunction(raw, func);
        if qjs::JS_IsException(result) {
            // Exception pending on the context; the sentinel carries no
            // refcount, so freeing it is a harmless no-op.
            qjs::JS_FreeValue(raw, result);
            return Err(rquickjs::Error::Exception);
        }
        qjs::JS_FreeValue(raw, result);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rquickjs::{Context, Function, Runtime};

    fn fresh() -> (Runtime, Context) {
        let rt = Runtime::new().unwrap();
        let ctx = Context::full(&rt).unwrap();
        (rt, ctx)
    }

    #[test]
    fn test_bytecode_defines_global_functions() {
        clear();
        let src = "function greet(n){ return 'hi ' + n; } var TOP = 42;";

        // Compile+cache in one runtime
        {
            let (_rt, ctx) = fresh();
            ctx.with(|ctx| {
                eval_program(&ctx, "s1", src).expect("first eval");
                let f: Function = ctx.globals().get("greet").expect("greet defined");
                let out: String = f.call(("world",)).unwrap();
                assert_eq!(out, "hi world");
            });
        }

        // A fresh runtime reads the cached bytecode and still exposes globals
        {
            let (_rt, ctx) = fresh();
            ctx.with(|ctx| {
                eval_program(&ctx, "s1", src).expect("cached eval");
                let top: i32 = ctx.globals().get("TOP").unwrap();
                assert_eq!(top, 42);
            });
        }
    }

    #[test]
    fn test_source_change_recompiles() {
        clear();
        let (_rt, ctx) = fresh();
        ctx.with(|ctx| {
            eval_program(&ctx, "s2", "var V = 1;").unwrap();
            let v: i32 = ctx.globals().get("V").unwrap();
            assert_eq!(v, 1);
        });
        // Same key, changed source: must not serve stale bytecode
        let (_rt2, ctx2) = fresh();
        ctx2.with(|ctx| {
            eval_program(&ctx, "s2", "var V = 2;").unwrap();
            let v: i32 = ctx.globals().get("V").unwrap();
            assert_eq!(v, 2);
        });
    }

    #[test]
    fn test_compile_error_surfaces() {
        clear();
        let (_rt, ctx) = fresh();
        ctx.with(|ctx| {
            let result = eval_program(&ctx, "s3", "function (");
            assert!(result.is_err(), "syntax error must be reported");
        });
    }

    #[test]
    fn test_runtime_exception_surfaces_and_is_catchable() {
        clear();
        let (_rt, ctx) = fresh();
        ctx.with(|ctx| {
            let result = eval_program(&ctx, "s4", "throw new Error('boom');");
            assert!(result.is_err());
            // The pending exception must be retrievable via catch()
            let caught = ctx.catch();
            assert!(
                caught.as_exception().is_some() || caught.is_object(),
                "exception should be pending on the context"
            );
        });
    }
}
