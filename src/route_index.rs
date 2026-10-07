//! Cached index over script route registrations.
//!
//! Matching a request must not read every script's metadata from the
//! database. This module builds the lookup table once and serves matching from
//! memory; script changes invalidate the index and the next request rebuilds
//! it.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use tracing::debug;

use crate::repository::{self, Repository as _};

/// Result of a route lookup.
#[derive(Debug)]
pub enum RouteLookup {
    /// A registration matched.
    Handler {
        script_uri: String,
        handler_name: String,
        /// What the engine does with it: run the handler, or send a file.
        kind: repository::RouteKind,
        /// The file to send, for [`repository::RouteKind::File`].
        file: Option<String>,
        /// The function that decides who may read it, where no handler runs.
        authorize: Option<String>,
        /// The registered pattern that matched, e.g. `/things/:id`. The caller
        /// has the concrete path already; what it cannot reconstruct is which
        /// registration served it, which is what attributes a request's logs
        /// and metrics to a handler rather than to one parameter value.
        pattern: String,
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
    /// What the engine does once this path matches.
    kind: repository::RouteKind,
    /// The file to send, for [`repository::RouteKind::File`].
    file: Option<String>,
    /// The function that decides who may read it, where no handler runs.
    authorize: Option<String>,
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
    /// routes (asset paths, streams, MCP tools). Cached
    /// here so it is rebuilt and invalidated together with the routes above.
    /// Covers every script, including ones with no routes of their own.
    script_hosts: HashMap<String, Vec<String>>,
    /// (host, registered path, method) -> the script that holds it.
    ///
    /// One holder per key. Where several scripts claim the same key the
    /// highest-ranked one (see [`rank`]) holds it and the others are listed in
    /// [`Self::collisions`] instead of being indexed.
    holders: HashMap<(String, String, String), String>,
    /// script URI -> rank, for deciding who outranks whom at registration time.
    ranks: HashMap<String, Rank>,
    /// Registrations that lost their key to another script.
    collisions: Vec<Collision>,
}

/// Which of two scripts claiming the same path keeps it: the older one, then
/// the one whose URI sorts first.
///
/// A rule about the scripts rather than about when their `init()`s happened to
/// run, so a restart, a re-deploy or a different startup concurrency cannot
/// hand a path to the other script. "The holder keeps it" has to be true of
/// the script that was serving the path yesterday, and the one that has been
/// here longest is the one that was.
type Rank = (std::time::SystemTime, String);

fn rank(script: &repository::ScriptMetadata) -> Rank {
    (script.created_at, script.uri.clone())
}

/// One registration that was not indexed because another script holds its
/// `(host, path, method)`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Collision {
    /// The script whose registration was refused.
    pub script_uri: String,
    /// The host the two scripts share.
    pub host: String,
    pub path: String,
    pub method: String,
    /// The script that holds it.
    pub held_by: String,
}

impl Collision {
    /// What a script is told when it asks for something already held.
    pub fn reason(&self) -> String {
        format!(
            "'{} {}' on {} is held by script '{}'. Register a different path, or bind \
             this script to a different host.",
            self.method,
            self.path,
            if self.host.is_empty() {
                "this engine"
            } else {
                &self.host
            },
            self.held_by
        )
    }
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
    // Highest rank first, so the first claimant of a key is the one that
    // keeps it.
    let mut ordered: Vec<&repository::ScriptMetadata> = metadata.iter().collect();
    ordered.sort_by_key(|script| rank(script));
    for script in ordered {
        inner.ranks.insert(script.uri.clone(), rank(script));
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

        // Sorted so the collisions come out in a stable order.
        let mut registrations: Vec<_> = script.registrations.iter().collect();
        registrations.sort_by(|a, b| a.0.cmp(b.0));
        for ((pattern, method), route_meta) in registrations {
            for host in &script_hosts {
                let key = (host.clone(), pattern.clone(), method.clone());
                if let Some(holder) = inner.holders.get(&key) {
                    inner.collisions.push(Collision {
                        script_uri: script.uri.clone(),
                        host: host.clone(),
                        path: pattern.clone(),
                        method: method.clone(),
                        held_by: holder.clone(),
                    });
                    continue;
                }
                inner.holders.insert(key, script.uri.clone());
                let target = RouteTarget {
                    script_uri: script.uri.clone(),
                    handler_name: route_meta.handler_name.clone(),
                    kind: route_meta.kind,
                    file: route_meta.file.clone(),
                    authorize: route_meta.authorize.clone(),
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

/// Every registration that lost its key to another script, in a stable order.
///
/// Derived from the scripts' stored registrations rather than recorded as
/// refusals happen, so it describes the deployment as it now stands and needs
/// no clearing when a script is initialised again.
pub fn collisions_in(metadata: &[repository::ScriptMetadata]) -> Vec<Collision> {
    let mut collisions = build_index(metadata).collisions;
    collisions.sort_by(|a, b| {
        (&a.script_uri, &a.host, &a.path, &a.method).cmp(&(
            &b.script_uri,
            &b.host,
            &b.path,
            &b.method,
        ))
    });
    collisions
}

/// The index, from the cache or built now, for callers that are not async.
///
/// Registration runs on a blocking thread in the middle of an `init()`, where
/// the awaiting form is not available. The cache is the point: a script's own
/// registrations reach the stored metadata only when its `init()` finishes, so
/// the index does not change under it while it registers.
fn current_index_blocking() -> Option<Arc<IndexInner>> {
    if let Ok(guard) = INDEX.read()
        && let Some(index) = guard.as_ref()
    {
        return Some(Arc::clone(index));
    }
    let metadata = repository::get_all_script_metadata().ok()?;
    let index = Arc::new(build_index(&metadata));
    if let Ok(mut guard) = INDEX.write() {
        *guard = Some(Arc::clone(&index));
    }
    Some(index)
}

/// Whether another script that outranks `script_uri` already holds `path` for
/// `method` on a host `script_uri` is published on — and so would refuse it.
///
/// What a script's `registerRoute` asks before it records anything. It is
/// feedback, not the rule: [`build_index`] decides who holds a key from the
/// scripts' ranks alone, so a script that registers before the holder has
/// initialised is still outranked afterwards, and the two answers agree about
/// who ends up serving the path.
pub fn refusal_for(script_uri: &str, path: &str, method: &str) -> Option<Collision> {
    let index = current_index_blocking()?;
    let mine = index.ranks.get(script_uri);
    let hosts = index.script_hosts.get(script_uri)?;
    for host in hosts {
        let key = (host.clone(), path.to_string(), method.to_string());
        let Some(holder) = index.holders.get(&key) else {
            continue;
        };
        if holder == script_uri {
            continue;
        }
        // A script not yet in the index (a first deploy) is newer than
        // everything that is.
        let outranked = match (mine, index.ranks.get(holder)) {
            (Some(mine), Some(theirs)) => theirs < mine,
            _ => true,
        };
        if outranked {
            return Some(Collision {
                script_uri: script_uri.to_string(),
                host: host.clone(),
                path: path.to_string(),
                method: method.to_string(),
                held_by: holder.clone(),
            });
        }
    }
    None
}

/// What binding `script_uri` to `hosts` would put in conflict, whoever ranks
/// higher: the registrations it holds now that another script already holds on
/// a host it would move onto.
///
/// For the "move this script to that host" call, which is where an operator
/// should learn about it rather than from a path that stopped answering.
pub fn conflicts_if_bound(
    metadata: &[repository::ScriptMetadata],
    script_uri: &str,
    hosts: &[String],
) -> Vec<Collision> {
    let Some(script) = metadata.iter().find(|m| m.uri == script_uri) else {
        return Vec::new();
    };
    // Only the hosts it would arrive on. What it already shares today is the
    // collision report's business, and refusing to re-save an unchanged
    // binding would make the one tool for moving it away unusable.
    let current = crate::hosts::effective_hosts(&script.hosts);
    let new_hosts: Vec<String> = crate::hosts::effective_hosts(hosts)
        .into_iter()
        .filter(|host| !current.contains(host))
        .collect();
    let mut found = Vec::new();
    for other in metadata {
        if other.uri == script_uri || !other.initialized {
            continue;
        }
        let theirs = crate::hosts::effective_hosts(&other.hosts);
        for host in new_hosts.iter().filter(|h| theirs.contains(h)) {
            for (path, method) in script.registrations.keys() {
                if other
                    .registrations
                    .contains_key(&(path.clone(), method.clone()))
                {
                    found.push(Collision {
                        script_uri: script_uri.to_string(),
                        host: host.clone(),
                        path: path.clone(),
                        method: method.clone(),
                        held_by: other.uri.clone(),
                    });
                }
            }
        }
    }
    found.sort_by(|a, b| (&a.host, &a.path, &a.method).cmp(&(&b.host, &b.path, &b.method)));
    found
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
/// For registries that filter a whole collection at once — the MCP registry
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

/// The scripts publishing on at least one host `script_uri` publishes on, or
/// `None` when every script should be treated as reachable from it.
///
/// What `tools.call` filters by. A script has no request host of its own — a
/// delegated task has none at all — so the hosts it serves stand in for the
/// one `/mcp` would filter by, and a tool bound away from every one of them
/// stays as unreachable from the script as it is from `/mcp` there.
pub async fn scripts_sharing_a_host(script_uri: &str) -> Option<std::collections::HashSet<String>> {
    if !crate::hosts::is_configured() {
        return None;
    }
    let index = match current_index().await {
        Ok(index) => index,
        Err(e) => {
            debug!("Could not resolve hosts for {}: {}", script_uri, e);
            return None;
        }
    };
    // Unknown to the index: the rule `script_serves_host` follows, where the
    // registries rather than a stale index decide what exists.
    let own = index.script_hosts.get(script_uri)?;
    Some(
        index
            .script_hosts
            .iter()
            .filter(|(_, hosts)| hosts.iter().any(|host| own.contains(host)))
            .map(|(uri, _)| uri.clone())
            .collect(),
    )
}

/// Every file route in the engine, as `(path, script_uri, file)`.
///
/// Reads the registrations rather than a registry of its own — there is not
/// one any more. For introspection and for tests that need to ask what a
/// script published.
pub async fn file_routes() -> Vec<(String, String, String)> {
    let Ok(index) = current_index().await else {
        return Vec::new();
    };
    let mut found: Vec<(String, String, String)> = index
        .exact
        .iter()
        .filter(|((_, _, method), _)| method == repository::ASSET_METHOD)
        .map(|((_, path, _), target)| {
            (
                path.clone(),
                target.script_uri.clone(),
                target.file.clone().unwrap_or_default(),
            )
        })
        .chain(
            index
                .patterns
                .iter()
                .filter(|route| route.method == repository::ASSET_METHOD)
                .map(|route| {
                    (
                        route.pattern.clone(),
                        route.target.script_uri.clone(),
                        route.target.file.clone().unwrap_or_default(),
                    )
                }),
        )
        .collect();
    found.sort();
    found.dedup();
    found
}

/// Whether `script_uri` publishes on `host`.
///
/// For the registrations that are not routes — asset paths, streams, MCP
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
    // A file route answers GET and HEAD, and is keyed under its own
    // pseudo-method so it does not share a slot with a handler on the same
    // path. Which of the two wins is stated here, once: the file does. A
    // collision between scripts is refused at registration rather than
    // resolved here.
    if method == "GET" || method == "HEAD" {
        let as_file = match_index(index, host, path, repository::ASSET_METHOD);
        if let RouteLookup::Handler { strip_body, .. } = &as_file {
            debug_assert!(!strip_body);
            let mut found = as_file;
            if method == "HEAD"
                && let RouteLookup::Handler { strip_body, .. } = &mut found
            {
                *strip_body = true;
            }
            return found;
        }
    }

    // A stream answers GET only — a HEAD of one would have to open a
    // connection to say nothing about it.
    if method == "GET" {
        let as_stream = match_index(index, host, path, repository::STREAM_METHOD);
        if matches!(as_stream, RouteLookup::Handler { .. }) {
            return as_stream;
        }
    }

    let result = match_index(index, host, path, method);
    if method == "HEAD"
        && !matches!(result, RouteLookup::Handler { .. })
        && let RouteLookup::Handler {
            script_uri,
            handler_name,
            kind,
            file,
            authorize,
            pattern,
            params,
            ..
        } = match_index(index, host, path, "GET")
    {
        return RouteLookup::Handler {
            script_uri,
            handler_name,
            kind,
            file,
            authorize,
            pattern,
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
            kind: target.kind,
            file: target.file.clone(),
            authorize: target.authorize.clone(),
            // An exact registration is its own pattern.
            pattern: path.to_string(),
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
            kind: route.target.kind,
            file: route.target.file.clone(),
            authorize: route.target.authorize.clone(),
            pattern: route.pattern.clone(),
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
                        kind: route_meta.kind,
                        file: route_meta.file.clone(),
                        authorize: route_meta.authorize.clone(),
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

    fn older(mut script: ScriptMetadata, seconds: u64) -> ScriptMetadata {
        script.created_at = std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds);
        script
    }

    #[test]
    fn the_older_script_keeps_a_contested_path() {
        let first = older(
            script_with_routes("zzz-first", &[("/shop", "GET", "first_handler")]),
            100,
        );
        let second = older(
            script_with_routes("aaa-second", &[("/shop", "GET", "second_handler")]),
            200,
        );

        // Whatever order the scripts are listed in, and whatever their names
        // sort as, the one that was here first serves it.
        for metadata in [
            vec![first.clone(), second.clone()],
            vec![second.clone(), first.clone()],
        ] {
            let index = build_index(&metadata);
            let (handler, _) = handler_of(match_index(&index, DEFAULT_TEST_HOST, "/shop", "GET"));
            assert_eq!(handler, "first_handler");
            assert_eq!(
                index.collisions,
                vec![Collision {
                    script_uri: "aaa-second".to_string(),
                    host: String::new(),
                    path: "/shop".to_string(),
                    method: "GET".to_string(),
                    held_by: "zzz-first".to_string(),
                }]
            );
        }
    }

    #[test]
    fn equal_ages_are_settled_by_name() {
        let a = script_with_routes("a", &[("/x", "GET", "ha")]);
        let b = script_with_routes("b", &[("/x", "GET", "hb")]);
        let index = build_index(&[b.clone(), a.clone()]);
        let (handler, _) = handler_of(match_index(&index, DEFAULT_TEST_HOST, "/x", "GET"));
        assert_eq!(handler, "ha");
    }

    #[test]
    fn different_methods_and_paths_do_not_collide() {
        let a = script_with_routes("a", &[("/x", "GET", "ha"), ("/y", "GET", "ha")]);
        let b = script_with_routes("b", &[("/x", "POST", "hb"), ("/z", "GET", "hb")]);
        assert!(build_index(&[a, b]).collisions.is_empty());
    }

    #[test]
    fn patterns_collide_on_the_registered_string() {
        let a = older(script_with_routes("a", &[("/t/:id", "GET", "ha")]), 1);
        let b = older(script_with_routes("b", &[("/t/:id", "GET", "hb")]), 2);
        let index = build_index(&[a, b]);
        assert_eq!(index.collisions.len(), 1);
        assert_eq!(index.collisions[0].held_by, "a");
    }

    #[test]
    fn binding_onto_a_host_that_holds_the_path_is_a_conflict() {
        let mine = script_with_routes("mine", &[("/shop", "GET", "h")]);
        let theirs = script_with_routes("theirs", &[("/shop", "GET", "h")]);
        // With no host configuration both publish everywhere already, so
        // moving changes nothing and is not what is being refused.
        assert!(conflicts_if_bound(&[mine, theirs], "mine", &[]).is_empty());
    }
}
