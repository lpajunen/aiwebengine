//! Cached index over script route registrations.
//!
//! Route matching previously fetched every script's metadata (a full database
//! read including all script contents) twice per request. This module builds
//! the lookup table once and serves matching from memory; script changes
//! invalidate the index and the next request rebuilds it.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use tracing::debug;

use crate::repository::{self, Repository as _};

/// Result of a route lookup.
#[derive(Debug)]
pub enum RouteLookup {
    /// A handler matched.
    Handler {
        script_uri: String,
        handler_name: String,
        /// Parameters extracted from `:param` path segments
        params: HashMap<String, String>,
        /// True when a HEAD request was served by falling back to the path's
        /// GET handler because no HEAD handler was registered for it. The
        /// caller must run the handler as usual but drop the response body
        /// before returning it, per RFC 7231 §4.3.2.
        strip_body: bool,
    },
    /// The path is registered, but not for the requested method (HTTP 405).
    MethodNotAllowed,
    /// No registration matches the path (HTTP 404).
    NotFound,
}

#[derive(Debug, Clone)]
struct RouteTarget {
    script_uri: String,
    handler_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PatternKind {
    /// Contains `:param` segments; matched with [`match_route_pattern`]
    Param,
    /// Ends with `/*`; `pattern` holds the prefix up to and including the `/`
    Wildcard,
}

#[derive(Debug)]
struct PatternRoute {
    pattern: String,
    method: String,
    kind: PatternKind,
    specificity: i32,
    target: RouteTarget,
}

impl PatternRoute {
    fn matches(&self, path: &str) -> Option<HashMap<String, String>> {
        match self.kind {
            PatternKind::Param => match_route_pattern(&self.pattern, path),
            PatternKind::Wildcard => path.starts_with(&self.pattern).then(HashMap::new),
        }
    }
}

#[derive(Debug, Default)]
struct IndexInner {
    /// (path, method) -> target, for patterns without params or wildcards
    exact: HashMap<(String, String), RouteTarget>,
    /// Param and wildcard patterns, competing on specificity at lookup time
    patterns: Vec<PatternRoute>,
}

static INDEX: RwLock<Option<Arc<IndexInner>>> = RwLock::new(None);

/// Drops the cached index; the next lookup rebuilds it from script metadata.
/// Must be called whenever scripts or their route registrations change.
pub fn invalidate() {
    if let Ok(mut guard) = INDEX.write() {
        *guard = None;
    }
}

/// Returns the current index, rebuilding it from script metadata if a script
/// change invalidated it. Concurrent rebuilds are harmless (last write wins).
async fn current_index() -> Result<Arc<IndexInner>, String> {
    if let Ok(guard) = INDEX.read()
        && let Some(index) = guard.as_ref()
    {
        return Ok(Arc::clone(index));
    }

    let metadata = repository::get_repository()
        .get_all_script_metadata()
        .await
        .map_err(|e| format!("Failed to fetch script metadata: {}", e))?;

    let inner = build_index(&metadata);
    debug!(
        "Rebuilt route index: {} exact routes, {} pattern routes",
        inner.exact.len(),
        inner.patterns.len()
    );

    let index = Arc::new(inner);
    if let Ok(mut guard) = INDEX.write() {
        *guard = Some(Arc::clone(&index));
    }
    Ok(index)
}

fn build_index(metadata: &[repository::ScriptMetadata]) -> IndexInner {
    let mut inner = IndexInner::default();
    for script in metadata {
        if !script.initialized || script.registrations.is_empty() {
            continue;
        }
        for ((pattern, method), route_meta) in &script.registrations {
            let target = RouteTarget {
                script_uri: script.uri.clone(),
                handler_name: route_meta.handler_name.clone(),
            };
            if pattern.ends_with("/*") {
                inner.patterns.push(PatternRoute {
                    // Keep the trailing '/' so "/api/*" matches "/api/x" but
                    // not "/apix"
                    pattern: pattern[..pattern.len() - 1].to_string(),
                    method: method.clone(),
                    kind: PatternKind::Wildcard,
                    specificity: calculate_route_specificity(pattern),
                    target,
                });
            } else if pattern.split('/').any(|part| part.starts_with(':')) {
                inner.patterns.push(PatternRoute {
                    pattern: pattern.clone(),
                    method: method.clone(),
                    kind: PatternKind::Param,
                    specificity: calculate_route_specificity(pattern),
                    target,
                });
            } else {
                inner
                    .exact
                    .insert((pattern.clone(), method.clone()), target);
            }
        }
    }

    inner
}

/// Finds the handler for a path and method. Exact matches win; param and
/// wildcard patterns compete on specificity (exact segments outweigh params,
/// which outweigh wildcard depth — see [`calculate_route_specificity`]).
///
/// HEAD requests fall back to the path's GET handler when no HEAD handler is
/// registered (RFC 7231 §4.3.2): a script that explicitly registers HEAD
/// always wins, otherwise the GET handler runs and [`RouteLookup::Handler`]
/// is returned with `strip_body: true` so the caller drops the body.
pub async fn lookup(path: &str, method: &str) -> Result<RouteLookup, String> {
    let index = current_index().await?;
    Ok(resolve(&index, path, method))
}

fn resolve(index: &IndexInner, path: &str, method: &str) -> RouteLookup {
    let result = match_index(index, path, method);
    if method == "HEAD"
        && !matches!(result, RouteLookup::Handler { .. })
        && let RouteLookup::Handler {
            script_uri,
            handler_name,
            params,
            ..
        } = match_index(index, path, "GET")
    {
        return RouteLookup::Handler {
            script_uri,
            handler_name,
            params,
            strip_body: true,
        };
    }
    result
}

fn match_index(index: &IndexInner, path: &str, method: &str) -> RouteLookup {
    if let Some(target) = index.exact.get(&(path.to_string(), method.to_string())) {
        return RouteLookup::Handler {
            script_uri: target.script_uri.clone(),
            handler_name: target.handler_name.clone(),
            params: HashMap::new(),
            strip_body: false,
        };
    }

    let mut best: Option<(&PatternRoute, HashMap<String, String>)> = None;
    for route in &index.patterns {
        if route.method != method {
            continue;
        }
        if let Some(params) = route.matches(path)
            && best
                .as_ref()
                .map(|(b, _)| route.specificity > b.specificity)
                .unwrap_or(true)
        {
            best = Some((route, params));
        }
    }
    if let Some((route, params)) = best {
        return RouteLookup::Handler {
            script_uri: route.target.script_uri.clone(),
            handler_name: route.target.handler_name.clone(),
            params,
            strip_body: false,
        };
    }

    // No handler for this method; distinguish 405 (path registered under
    // another method) from 404
    let path_registered = index.exact.keys().any(|(p, _)| p == path)
        || index
            .patterns
            .iter()
            .any(|route| route.matches(path).is_some());
    if path_registered {
        RouteLookup::MethodNotAllowed
    } else {
        RouteLookup::NotFound
    }
}

/// Calculate specificity score for a route pattern
/// Higher score = more specific route
/// Score = (exact segments × 1000) + (param segments × 100) - (wildcard depth × 10)
pub fn calculate_route_specificity(pattern: &str) -> i32 {
    let parts: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let mut exact_count = 0i32;
    let mut param_count = 0i32;
    let mut wildcard_depth = 0i32;

    for (depth, part) in parts.iter().enumerate() {
        if part.starts_with(':') {
            param_count += 1;
        } else if *part == "*" {
            wildcard_depth = (parts.len() - depth) as i32;
        } else {
            exact_count += 1;
        }
    }

    (exact_count * 1000) + (param_count * 100) - (wildcard_depth * 10)
}

/// Match a route pattern with parameters against a path
/// Returns extracted parameters if the pattern matches
pub fn match_route_pattern(pattern: &str, path: &str) -> Option<HashMap<String, String>> {
    let pattern_parts: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let path_parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    if pattern_parts.len() != path_parts.len() {
        return None;
    }

    let mut params = HashMap::new();

    for (pattern_part, path_part) in pattern_parts.iter().zip(path_parts.iter()) {
        if let Some(param_name) = pattern_part.strip_prefix(':') {
            // This is a parameter
            params.insert(param_name.to_string(), path_part.to_string());
        } else if *pattern_part != *path_part {
            // Literal parts must match exactly
            return None;
        }
    }

    Some(params)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repository::{RouteMetadata, ScriptMetadata};

    fn script_with_routes(uri: &str, routes: &[(&str, &str, &str)]) -> ScriptMetadata {
        let mut metadata = ScriptMetadata::new(uri.to_string(), String::new());
        metadata.initialized = true;
        for (pattern, method, handler) in routes {
            metadata.registrations.insert(
                (pattern.to_string(), method.to_string()),
                RouteMetadata::simple(handler.to_string()),
            );
        }
        metadata
    }

    fn handler_of(lookup: RouteLookup) -> (String, HashMap<String, String>) {
        match lookup {
            RouteLookup::Handler {
                handler_name,
                params,
                ..
            } => (handler_name, params),
            other => panic!("Expected a handler, got {:?}", other),
        }
    }

    #[test]
    fn test_exact_match_wins_over_patterns() {
        let index = build_index(&[script_with_routes(
            "s1",
            &[
                ("/api/users/:id", "GET", "param_handler"),
                ("/api/users/*", "GET", "wildcard_handler"),
                ("/api/users/me", "GET", "exact_handler"),
            ],
        )]);

        let (handler, params) = handler_of(match_index(&index, "/api/users/me", "GET"));
        assert_eq!(handler, "exact_handler");
        assert!(params.is_empty());
    }

    #[test]
    fn test_param_match_extracts_params() {
        let index = build_index(&[script_with_routes(
            "s1",
            &[("/api/users/:id", "GET", "param_handler")],
        )]);

        let (handler, params) = handler_of(match_index(&index, "/api/users/42", "GET"));
        assert_eq!(handler, "param_handler");
        assert_eq!(params.get("id").map(String::as_str), Some("42"));
    }

    #[test]
    fn test_wildcard_prefix_matching() {
        let index = build_index(&[script_with_routes(
            "s1",
            &[("/files/*", "GET", "files_handler")],
        )]);

        let (handler, _) = handler_of(match_index(&index, "/files/a/b/c.txt", "GET"));
        assert_eq!(handler, "files_handler");
        // The prefix keeps its slash: /filesx must not match
        assert!(matches!(
            match_index(&index, "/filesx", "GET"),
            RouteLookup::NotFound
        ));
    }

    #[test]
    fn test_deep_wildcard_beats_sparse_param_pattern() {
        // Preserves the original scoring: a wildcard with more exact segments
        // outranks a param pattern with fewer
        let index = build_index(&[script_with_routes(
            "s1",
            &[
                ("/:a/:b/:c/d", "GET", "sparse_param"),
                ("/a/b/c/*", "GET", "deep_wildcard"),
            ],
        )]);

        let (handler, _) = handler_of(match_index(&index, "/a/b/c/d", "GET"));
        assert_eq!(handler, "deep_wildcard");
    }

    #[test]
    fn test_method_not_allowed_vs_not_found() {
        let index = build_index(&[script_with_routes(
            "s1",
            &[("/api/thing", "POST", "post_handler")],
        )]);

        assert!(matches!(
            match_index(&index, "/api/thing", "GET"),
            RouteLookup::MethodNotAllowed
        ));
        assert!(matches!(
            match_index(&index, "/api/other", "GET"),
            RouteLookup::NotFound
        ));
    }

    #[test]
    fn test_uninitialized_scripts_are_excluded() {
        let mut metadata = script_with_routes("s1", &[("/route", "GET", "handler")]);
        metadata.initialized = false;
        let index = build_index(&[metadata]);

        assert!(matches!(
            match_index(&index, "/route", "GET"),
            RouteLookup::NotFound
        ));
    }

    #[test]
    fn test_head_falls_back_to_get_and_strips_body() {
        let index = build_index(&[script_with_routes(
            "s1",
            &[("/api/users", "GET", "list_users")],
        )]);

        match resolve(&index, "/api/users", "HEAD") {
            RouteLookup::Handler {
                handler_name,
                strip_body,
                ..
            } => {
                assert_eq!(handler_name, "list_users");
                assert!(strip_body);
            }
            other => panic!("Expected a handler, got {:?}", other),
        }
    }

    #[test]
    fn test_explicit_head_registration_wins_over_get_fallback() {
        let index = build_index(&[script_with_routes(
            "s1",
            &[
                ("/api/users", "GET", "list_users"),
                ("/api/users", "HEAD", "head_users"),
            ],
        )]);

        match resolve(&index, "/api/users", "HEAD") {
            RouteLookup::Handler {
                handler_name,
                strip_body,
                ..
            } => {
                assert_eq!(handler_name, "head_users");
                assert!(!strip_body);
            }
            other => panic!("Expected a handler, got {:?}", other),
        }
    }

    #[test]
    fn test_head_still_405_when_path_only_registered_for_other_methods() {
        let index = build_index(&[script_with_routes(
            "s1",
            &[("/api/thing", "POST", "post_handler")],
        )]);

        assert!(matches!(
            resolve(&index, "/api/thing", "HEAD"),
            RouteLookup::MethodNotAllowed
        ));
    }

    #[test]
    fn test_head_fallback_matches_param_and_wildcard_routes() {
        let index = build_index(&[script_with_routes(
            "s1",
            &[
                ("/api/users/:id", "GET", "get_user"),
                ("/files/*", "GET", "get_file"),
            ],
        )]);

        let (handler, params) = handler_of(resolve(&index, "/api/users/42", "HEAD"));
        assert_eq!(handler, "get_user");
        assert_eq!(params.get("id").map(String::as_str), Some("42"));

        let (handler, _) = handler_of(resolve(&index, "/files/a/b.txt", "HEAD"));
        assert_eq!(handler, "get_file");
    }
}
