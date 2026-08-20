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
    /// Host this pattern is published on; see [`IndexInner`]
    host: String,
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

/// Routes indexed per host.
///
/// Registrations are expanded across the hosts their script is bound to, so
/// the host is part of every key rather than a filter applied afterwards. Two
/// scripts can then register the same path on different hosts without one
/// shadowing the other, which is the point of binding scripts to hosts at all.
#[derive(Debug, Default)]
struct IndexInner {
    /// (host, path, method) -> target, for patterns without params or wildcards
    exact: HashMap<(String, String, String), RouteTarget>,
    /// Param and wildcard patterns, competing on specificity at lookup time
    patterns: Vec<PatternRoute>,
    /// script URI -> hosts it publishes on, for the registrations that are not
    /// routes (asset paths, streams, GraphQL operations, MCP tools). Cached
    /// here so it is rebuilt and invalidated together with the routes above.
    /// Covers every script, including ones with no routes of their own.
    script_hosts: HashMap<String, Vec<String>>,
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
        // A script is published on the hosts it is bound to: the default host
        // when unbound, every configured host for a `*` binding. Before hosts
        // are configured at all there is nothing to key on, so the script is
        // indexed under a single empty host and lookup does the same.
        let script_hosts = if crate::hosts::is_configured() {
            crate::hosts::effective_hosts(&script.hosts)
        } else {
            vec![String::new()]
        };
        // Recorded for every script, so non-route registrations can be checked
        // even when the script registered no routes or has not initialized.
        inner
            .script_hosts
            .insert(script.uri.clone(), script_hosts.clone());

        if !script.initialized || script.registrations.is_empty() {
            continue;
        }
        if script_hosts.is_empty() {
            debug!(
                "Script {} is bound to no served host; its routes are not indexed",
                script.uri
            );
            continue;
        }

        for ((pattern, method), route_meta) in &script.registrations {
            for host in &script_hosts {
                let target = RouteTarget {
                    script_uri: script.uri.clone(),
                    handler_name: route_meta.handler_name.clone(),
                };
                if pattern.ends_with("/*") {
                    inner.patterns.push(PatternRoute {
                        host: host.clone(),
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
                        host: host.clone(),
                        pattern: pattern.clone(),
                        method: method.clone(),
                        kind: PatternKind::Param,
                        specificity: calculate_route_specificity(pattern),
                        target,
                    });
                } else {
                    inner
                        .exact
                        .insert((host.clone(), pattern.clone(), method.clone()), target);
                }
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
/// `host` is the request's host resolved onto a configured one — see
/// [`crate::hosts::canonical_host`]. Only routes published on that host match.
pub async fn lookup(host: &str, path: &str, method: &str) -> Result<RouteLookup, String> {
    let index = current_index().await?;
    Ok(resolve(&index, host, path, method))
}

/// The scripts publishing on `host`, or `None` when host binding is not in
/// force and every script should be treated as publishing everywhere.
///
/// For registries that filter a whole collection at once — the GraphQL schema
/// and the MCP tool list — rather than checking one script at a time.
pub async fn scripts_for_host(host: &str) -> Option<std::collections::HashSet<String>> {
    if !crate::hosts::is_configured() {
        return None;
    }
    let index = match current_index().await {
        Ok(index) => index,
        Err(e) => {
            debug!("Could not resolve scripts for host {}: {}", host, e);
            return None;
        }
    };
    Some(
        index
            .script_hosts
            .iter()
            .filter(|(_, script_hosts)| script_hosts.iter().any(|h| h == host))
            .map(|(uri, _)| uri.clone())
            .collect(),
    )
}

/// Whether `script_uri` publishes on `host`.
///
/// For the registrations that are not routes — asset paths, streams, GraphQL
/// operations and MCP tools — which are looked up by their own registries and
/// then checked against the script that owns them.
///
/// A script missing from the index (deleted, or added since the last rebuild)
/// answers true: refusing would hide a registration the owning registry still
/// considers live, and the registries are the authority on what exists.
pub async fn script_serves_host(script_uri: &str, host: &str) -> bool {
    if !crate::hosts::is_configured() {
        return true;
    }
    match current_index().await {
        Ok(index) => match index.script_hosts.get(script_uri) {
            Some(script_hosts) => script_hosts.iter().any(|h| h == host),
            None => true,
        },
        Err(e) => {
            debug!(
                "Host check for {} fell back to allowing the request: {}",
                script_uri, e
            );
            true
        }
    }
}

fn resolve(index: &IndexInner, host: &str, path: &str, method: &str) -> RouteLookup {
    let result = match_index(index, host, path, method);
    if method == "HEAD"
        && !matches!(result, RouteLookup::Handler { .. })
        && let RouteLookup::Handler {
            script_uri,
            handler_name,
            params,
            ..
        } = match_index(index, host, path, "GET")
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

fn match_index(index: &IndexInner, host: &str, path: &str, method: &str) -> RouteLookup {
    if let Some(target) = index
        .exact
        .get(&(host.to_string(), path.to_string(), method.to_string()))
    {
        return RouteLookup::Handler {
            script_uri: target.script_uri.clone(),
            handler_name: target.handler_name.clone(),
            params: HashMap::new(),
            strip_body: false,
        };
    }

    let mut best: Option<(&PatternRoute, HashMap<String, String>)> = None;
    for route in &index.patterns {
        if route.host != host || route.method != method {
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
    // another method) from 404. Scoped to this host: a path published only on
    // another host is genuinely not found here, not a method mismatch.
    let path_registered = index.exact.keys().any(|(h, p, _)| h == host && p == path)
        || index
            .patterns
            .iter()
            .any(|route| route.host == host && route.matches(path).is_some());
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

    /// Host these tests index and look up under. Unit tests run without a host
    /// configuration, so `build_index` files every route under the empty host.
    const DEFAULT_TEST_HOST: &str = "";

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
    fn test_redeployed_script_keeps_routing_until_reinit() {
        // A deploy upserts the source and re-inits asynchronously. The index is
        // rebuilt in between, and must still route to the previous handlers
        // instead of 404ing every route of the script.
        let mut metadata = script_with_routes("s1", &[("/api/world", "GET", "world_handler")]);
        metadata.update_content("// redeployed source".to_string());

        let index = build_index(&[metadata]);

        let (handler, _) = handler_of(match_index(&index, DEFAULT_TEST_HOST, "/api/world", "GET"));
        assert_eq!(handler, "world_handler");
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

        let (handler, params) = handler_of(match_index(
            &index,
            DEFAULT_TEST_HOST,
            "/api/users/me",
            "GET",
        ));
        assert_eq!(handler, "exact_handler");
        assert!(params.is_empty());
    }

    #[test]
    fn test_param_match_extracts_params() {
        let index = build_index(&[script_with_routes(
            "s1",
            &[("/api/users/:id", "GET", "param_handler")],
        )]);

        let (handler, params) = handler_of(match_index(
            &index,
            DEFAULT_TEST_HOST,
            "/api/users/42",
            "GET",
        ));
        assert_eq!(handler, "param_handler");
        assert_eq!(params.get("id").map(String::as_str), Some("42"));
    }

    #[test]
    fn test_wildcard_prefix_matching() {
        let index = build_index(&[script_with_routes(
            "s1",
            &[("/files/*", "GET", "files_handler")],
        )]);

        let (handler, _) = handler_of(match_index(
            &index,
            DEFAULT_TEST_HOST,
            "/files/a/b/c.txt",
            "GET",
        ));
        assert_eq!(handler, "files_handler");
        // The prefix keeps its slash: /filesx must not match
        assert!(matches!(
            match_index(&index, DEFAULT_TEST_HOST, "/filesx", "GET"),
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

        let (handler, _) = handler_of(match_index(&index, DEFAULT_TEST_HOST, "/a/b/c/d", "GET"));
        assert_eq!(handler, "deep_wildcard");
    }

    #[test]
    fn test_method_not_allowed_vs_not_found() {
        let index = build_index(&[script_with_routes(
            "s1",
            &[("/api/thing", "POST", "post_handler")],
        )]);

        assert!(matches!(
            match_index(&index, DEFAULT_TEST_HOST, "/api/thing", "GET"),
            RouteLookup::MethodNotAllowed
        ));
        assert!(matches!(
            match_index(&index, DEFAULT_TEST_HOST, "/api/other", "GET"),
            RouteLookup::NotFound
        ));
    }

    #[test]
    fn test_uninitialized_scripts_are_excluded() {
        let mut metadata = script_with_routes("s1", &[("/route", "GET", "handler")]);
        metadata.initialized = false;
        let index = build_index(&[metadata]);

        assert!(matches!(
            match_index(&index, DEFAULT_TEST_HOST, "/route", "GET"),
            RouteLookup::NotFound
        ));
    }

    #[test]
    fn test_head_falls_back_to_get_and_strips_body() {
        let index = build_index(&[script_with_routes(
            "s1",
            &[("/api/users", "GET", "list_users")],
        )]);

        match resolve(&index, DEFAULT_TEST_HOST, "/api/users", "HEAD") {
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

        match resolve(&index, DEFAULT_TEST_HOST, "/api/users", "HEAD") {
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
            resolve(&index, DEFAULT_TEST_HOST, "/api/thing", "HEAD"),
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

        let (handler, params) =
            handler_of(resolve(&index, DEFAULT_TEST_HOST, "/api/users/42", "HEAD"));
        assert_eq!(handler, "get_user");
        assert_eq!(params.get("id").map(String::as_str), Some("42"));

        let (handler, _) = handler_of(resolve(&index, DEFAULT_TEST_HOST, "/files/a/b.txt", "HEAD"));
        assert_eq!(handler, "get_file");
    }

    /// Index a script under explicit hosts, bypassing the global host config
    /// so these tests do not depend on startup state.
    fn index_for_hosts(scripts: &[(ScriptMetadata, &[&str])]) -> IndexInner {
        let mut inner = IndexInner::default();
        for (script, script_hosts) in scripts {
            for ((pattern, method), route_meta) in &script.registrations {
                for host in *script_hosts {
                    let target = RouteTarget {
                        script_uri: script.uri.clone(),
                        handler_name: route_meta.handler_name.clone(),
                    };
                    if pattern.split('/').any(|part| part.starts_with(':')) {
                        inner.patterns.push(PatternRoute {
                            host: (*host).to_string(),
                            pattern: pattern.clone(),
                            method: method.clone(),
                            kind: PatternKind::Param,
                            specificity: calculate_route_specificity(pattern),
                            target,
                        });
                    } else {
                        inner.exact.insert(
                            ((*host).to_string(), pattern.clone(), method.clone()),
                            target,
                        );
                    }
                }
            }
        }
        inner
    }

    #[test]
    fn same_path_on_two_hosts_routes_to_different_scripts() {
        // The reason the host is part of the key rather than a later filter:
        // neither registration may shadow the other.
        let admin = script_with_routes("admin", &[("/dashboard", "GET", "admin_dashboard")]);
        let public = script_with_routes("public", &[("/dashboard", "GET", "public_dashboard")]);
        let index = index_for_hosts(&[
            (admin, &["manage.softagen.com"]),
            (public, &["softagen.com"]),
        ]);

        let (handler, _) = handler_of(match_index(
            &index,
            "manage.softagen.com",
            "/dashboard",
            "GET",
        ));
        assert_eq!(handler, "admin_dashboard");

        let (handler, _) = handler_of(match_index(&index, "softagen.com", "/dashboard", "GET"));
        assert_eq!(handler, "public_dashboard");
    }

    #[test]
    fn a_route_is_not_found_on_a_host_it_is_not_published_on() {
        let admin = script_with_routes("admin", &[("/secrets", "GET", "list_secrets")]);
        let index = index_for_hosts(&[(admin, &["manage.softagen.com"])]);

        assert!(matches!(
            match_index(&index, "softagen.com", "/secrets", "GET"),
            RouteLookup::NotFound
        ));
    }

    #[test]
    fn a_path_on_another_host_is_not_reported_as_method_not_allowed() {
        // 405 would leak that the path exists somewhere; on this host it does
        // not exist at all.
        let admin = script_with_routes("admin", &[("/secrets", "GET", "list_secrets")]);
        let index = index_for_hosts(&[(admin, &["manage.softagen.com"])]);

        assert!(matches!(
            match_index(&index, "softagen.com", "/secrets", "POST"),
            RouteLookup::NotFound
        ));
        assert!(matches!(
            match_index(&index, "manage.softagen.com", "/secrets", "POST"),
            RouteLookup::MethodNotAllowed
        ));
    }

    #[test]
    fn a_script_on_every_host_answers_on_each_of_them() {
        let about = script_with_routes("about", &[("/about", "GET", "about_page")]);
        let index = index_for_hosts(&[(
            about,
            &["softagen.com", "manage.softagen.com", "world.softagen.com"],
        )]);

        for host in ["softagen.com", "manage.softagen.com", "world.softagen.com"] {
            let (handler, _) = handler_of(match_index(&index, host, "/about", "GET"));
            assert_eq!(
                handler, "about_page",
                "expected /about to answer on {}",
                host
            );
        }
    }

    #[test]
    fn pattern_routes_are_scoped_to_their_host_too() {
        let admin = script_with_routes("admin", &[("/users/:id", "GET", "admin_user")]);
        let index = index_for_hosts(&[(admin, &["manage.softagen.com"])]);

        let (handler, params) = handler_of(match_index(
            &index,
            "manage.softagen.com",
            "/users/42",
            "GET",
        ));
        assert_eq!(handler, "admin_user");
        assert_eq!(params.get("id").map(String::as_str), Some("42"));

        assert!(matches!(
            match_index(&index, "softagen.com", "/users/42", "GET"),
            RouteLookup::NotFound
        ));
    }

    #[test]
    fn build_index_publishes_unbound_scripts_on_one_host_only() {
        // Without a host configuration every script lands under the empty host,
        // which is what single-host and pre-host-binding deployments rely on.
        let script = script_with_routes("s1", &[("/a", "GET", "h")]);
        let index = build_index(&[script]);

        assert_eq!(index.exact.len(), 1);
        assert!(index.exact.contains_key(&(
            DEFAULT_TEST_HOST.to_string(),
            "/a".to_string(),
            "GET".to_string()
        )));
    }
}
