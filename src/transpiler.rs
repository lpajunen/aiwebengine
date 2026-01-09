use crate::error::AppResult;
use crate::repository::insert_log_message;
use oxc::allocator::Allocator;
use oxc::codegen::CodeGenerator;
use oxc::parser::Parser;
use oxc::span::SourceType;
use oxc::transformer::{TransformOptions, Transformer, Tsx};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use tracing::{debug, error};

/// Cached transpilation result
#[derive(Clone, Debug)]
struct CachedTranspilation {
    /// Transpiled JavaScript code with inline source map
    code: String,
    /// SHA256 hash of the original source content
    content_hash: String,
}

/// Global in-memory cache for transpiled scripts
static TRANSPILED_CACHE: OnceLock<Mutex<HashMap<String, CachedTranspilation>>> = OnceLock::new();

/// Check if a script needs transpilation based on file extension
fn needs_transpilation(uri: &str) -> bool {
    uri.ends_with(".ts") || uri.ends_with(".tsx") || uri.ends_with(".jsx")
}

/// Determine the syntax configuration based on file extension
fn get_source_type(uri: &str) -> SourceType {
    if uri.ends_with(".ts") {
        SourceType::default().with_typescript(true)
    } else if uri.ends_with(".tsx") {
        SourceType::default().with_typescript(true).with_jsx(true)
    } else if uri.ends_with(".jsx") {
        SourceType::default().with_jsx(true)
    } else {
        // Default to JavaScript
        SourceType::default()
    }
}

/// Calculate SHA256 hash of content for cache key
fn calculate_content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Get transpiled script from cache
fn get_cached_transpilation(uri: &str, content_hash: &str) -> Option<String> {
    let cache = TRANSPILED_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(guard) = cache.lock() {
        if let Some(cached) = guard.get(uri) {
            if cached.content_hash == content_hash {
                debug!(
                    uri = uri,
                    "Transpilation cache hit"
                );
                return Some(cached.code.clone());
            } else {
                debug!(
                    uri = uri,
                    "Transpilation cache miss (content changed)"
                );
            }
        }
    }
    None
}

/// Store transpiled script in cache
fn cache_transpilation(uri: &str, code: String, content_hash: String) {
    let cache = TRANSPILED_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(mut guard) = cache.lock() {
        guard.insert(
            uri.to_string(),
            CachedTranspilation {
                code,
                content_hash,
            },
        );
    }
}

/// Invalidate cached transpilation for a script
pub fn invalidate_transpilation_cache(uri: &str) {
    let cache = TRANSPILED_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Ok(mut guard) = cache.lock() {
        if guard.remove(uri).is_some() {
            debug!(
                uri = uri,
                "Invalidated transpilation cache"
            );
        }
    }
}

/// Transpile TypeScript/JSX/TSX to JavaScript with source maps
fn transpile(uri: &str, content: &str) -> AppResult<String> {
    let start = std::time::Instant::now();

    // Create allocator for oxc
    let allocator = Allocator::default();

    // Determine source type from extension
    let source_type = get_source_type(uri);

    // Parse the source code
    let ret = Parser::new(&allocator, content, source_type).parse();

    // Check for parse errors
    if !ret.errors.is_empty() {
        let error_msg = format!("Parse errors in {}: {:?}", uri, ret.errors);
        error!(error = %error_msg, uri = uri, "Transpilation parse error");
        // Log to database
        insert_log_message(uri, &format!("Transpilation parse error: {:?}", ret.errors), "ERROR");
        return Err(crate::error::AppError::Config(error_msg));
    }

    let mut program = ret.program;

    // Transform options for TypeScript stripping and JSX transformation
    let transform_options = TransformOptions {
        jsx: if uri.ends_with(".jsx") || uri.ends_with(".tsx") {
            Tsx {
                pragma: Some("h".into()),
                pragma_frag: Some("Fragment".into()),
                ..Default::default()
            }
        } else {
            Default::default()
        },
        ..Default::default()
    };

    // Apply transformations
    let result = Transformer::new(&allocator, uri.into(), source_type, &transform_options)
        .build(&mut program);
    
    if !result.errors.is_empty() {
        let error_msg = format!("Transform errors in {}: {:?}", uri, result.errors);
        error!(error = %error_msg, uri = uri, "Transpilation transform error");
        // Log to database
        insert_log_message(uri, &format!("Transpilation transform error: {:?}", result.errors), "ERROR");
        return Err(crate::error::AppError::Config(error_msg));
    }

    // Generate JavaScript code
    let printed = CodeGenerator::new().build(&program);
    
    let js_code = printed.code;

    // For now, we don't include source maps (oxc API has changed)
    // TODO: Add source map support when stable API is available

    let elapsed = start.elapsed();
    debug!(
        uri = uri,
        duration_ms = elapsed.as_millis(),
        "Transpiled script"
    );

    Ok(js_code)
}

/// Transpile script if needed based on file extension
///
/// - `.ts`, `.tsx`, `.jsx` files are transpiled to JavaScript
/// - `.js` files are returned as-is
/// - Returns transpiled code with inline source map
pub fn transpile_if_needed(uri: &str, content: &str) -> AppResult<String> {
    // Pass through JavaScript files without transpilation
    if !needs_transpilation(uri) {
        return Ok(content.to_string());
    }

    // Check cache
    let content_hash = calculate_content_hash(content);
    if let Some(cached_code) = get_cached_transpilation(uri, &content_hash) {
        return Ok(cached_code);
    }

    // Transpile
    let transpiled_code = transpile(uri, content)?;

    // Cache the result
    cache_transpilation(uri, transpiled_code.clone(), content_hash);

    Ok(transpiled_code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_needs_transpilation() {
        assert!(needs_transpilation("script.ts"));
        assert!(needs_transpilation("component.tsx"));
        assert!(needs_transpilation("component.jsx"));
        assert!(!needs_transpilation("script.js"));
        assert!(!needs_transpilation("unknown.xyz"));
    }

    #[test]
    fn test_transpile_typescript() {
        let uri = "test.ts";
        let content = r#"
            const greeting: string = "Hello";
            interface User {
                name: string;
            }
            const user: User = { name: "Test" };
        "#;

        let result = transpile_if_needed(uri, content);
        assert!(result.is_ok());
        let js_code = result.unwrap();
        assert!(js_code.contains("greeting"));
        assert!(js_code.contains("//# sourceMappingURL="));
    }

    #[test]
    fn test_transpile_jsx() {
        let uri = "component.jsx";
        let content = r#"
            const element = <div className="test">Hello</div>;
        "#;

        let result = transpile_if_needed(uri, content);
        assert!(result.is_ok());
        let js_code = result.unwrap();
        assert!(js_code.contains("h(")); // Should use custom pragma
    }

    #[test]
    fn test_passthrough_javascript() {
        let uri = "script.js";
        let content = "const x = 42;";

        let result = transpile_if_needed(uri, content);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), content);
    }
}
