//! Where the management surface answers: management hosts, reserved route prefixes
//! and the engine's own streams.

use super::*;
use serde_json::{Value, json};
use tracing::{debug, warn};

/// Path prefixes owned by the engine. Scripts may not register HTTP, stream,
/// or asset routes at or under these prefixes; every other path is open to
/// any script. `/` and `/favicon.ico` are intentionally not reserved — the
/// engine serves defaults for them only when no script claims them.
pub const RESERVED_ROUTE_PREFIXES: &[&str] =
    &["/health", "/mcp", "/auth", "/.well-known", "/engine"];

/// The engine-owned SSE stream carrying script change notifications.
///
/// Lives under the reserved `/engine` prefix so that a script cannot register
/// a stream on this path: [`crate::stream_registry::StreamRegistry`] replaces
/// an existing registration that has no active connections, so an unreserved
/// path would let a script take ownership of the engine's stream.
pub const ENGINE_SCRIPT_UPDATES_STREAM: &str = "/engine/script_updates";

/// Hosts allowed to serve the management APIs, normalized to Host-header form.
/// Empty (or unset) means every host serves them, which is what a single-host
/// deployment wants. Set once at startup from `server.management_hosts`.
pub(super) static MANAGEMENT_HOSTS: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();

/// Record which hosts may serve the management APIs. Called once at startup;
/// later calls are ignored so the boundary cannot be widened at runtime.
pub fn init_management_hosts(hosts: Vec<String>) {
    let _ = MANAGEMENT_HOSTS.set(hosts);
}

/// Whether `host` may serve the management APIs, given the configured list.
///
/// An empty list allows every host. Otherwise the match is exact against the
/// normalized entries, and a request without a Host header is refused — HTTP
/// requires one, so its absence should not open the boundary.
pub(super) fn host_is_allowed(allowed: &[String], host: Option<&str>) -> bool {
    if allowed.is_empty() {
        return true;
    }
    match host {
        Some(host) => allowed.contains(&host.trim().to_lowercase()),
        None => false,
    }
}

/// Whether a request arriving on `host` may reach the management APIs.
pub fn is_management_host(host: Option<&str>) -> bool {
    match MANAGEMENT_HOSTS.get() {
        Some(allowed) => host_is_allowed(allowed, host),
        // Not configured yet (tests constructing routers directly, or startup
        // ordering) — behave as an unrestricted single-host deployment.
        None => true,
    }
}

/// Returns the reserved prefix that `path` falls under, if any.
pub fn reserved_route_prefix(path: &str) -> Option<&'static str> {
    RESERVED_ROUTE_PREFIXES.iter().copied().find(|prefix| {
        path == *prefix
            || path
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}

/// Broadcast a script update to the `/engine/script_updates` stream, matching
/// the message format core.js used. Extra `details` entries become message
/// metadata used for connection filtering.
pub fn broadcast_script_update(uri: &str, action: &str, details: &[(&str, Value)]) {
    let mut message = json!({
        "type": "script_update",
        "uri": uri,
        "action": action,
        "timestamp": iso_timestamp(),
    });
    if let Some(obj) = message.as_object_mut() {
        for (key, value) in details {
            obj.insert((*key).to_string(), value.clone());
        }
    }

    match crate::stream_registry::GLOBAL_STREAM_REGISTRY
        .broadcast_to_stream(ENGINE_SCRIPT_UPDATES_STREAM, &message.to_string())
    {
        Ok(_) => debug!("Broadcasted script update: {} {}", action, uri),
        Err(e) => warn!("Failed to broadcast script update for {}: {}", uri, e),
    }
}

/// Register engine-provided streams. Called once at startup.
///
/// [`ENGINE_SCRIPT_UPDATES_STREAM`] carries the script change notifications
/// broadcast by [`broadcast_script_update`]. There is no customization
/// function, so a connection's filter criteria come from its query parameters —
/// a client connecting without any receives all messages, exactly as before.
pub fn register_engine_streams() {
    if let Err(e) = crate::stream_registry::GLOBAL_STREAM_REGISTRY.register_stream(
        ENGINE_SCRIPT_UPDATES_STREAM,
        "engine://native",
        None,
    ) {
        warn!(
            "Failed to register {} stream: {}",
            ENGINE_SCRIPT_UPDATES_STREAM, e
        );
    }
}
