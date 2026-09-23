//! MCP Client Module
//!
//! Provides client functionality for connecting to external Model Context Protocol (MCP) servers.
//! Implements JSON-RPC 2.0 protocol for tool discovery and invocation.
//!
//! # Features
//!
//! 1. Protocol era negotiation — modern (`2026-07-28`) where the server speaks
//!    it, the `initialize` handshake where it does not
//! 2. Tool discovery via `tools/list`
//! 3. Tool invocation via `tools/call`
//! 4. TTL-based caching, taking the server's own `ttlMs` where it offers one
//! 5. Secret injection for Authorization headers
//! 6. Error handling for network, auth, and protocol errors
//!
//! Requests go out through [`crate::http_client::HttpClient`] rather than a
//! `reqwest` of its own, so a server URL — which comes from a script — gets the
//! same URL and DNS validation, the same per-hop redirect checking and the same
//! response ceiling that a script's `fetch` gets.
//!
//! # Being a client of both eras
//!
//! This module used to speak `2025-11-25` and nothing else: it sent
//! `initialize`, ignored whether it worked, and then sent `tools/list` with no
//! `_meta`. That was survivable only for as long as every server was either
//! legacy or dual-era. A server that implements `2026-07-28` alone answers no
//! `initialize` and refuses a request that names no version, and this client
//! had nothing to fall forward to — the mirror image of the problem the engine's
//! *server* half solved by staying dual-era, and worth fixing in the same
//! place in the argument rather than after somebody hits it.
//!
//! So the era is **learned rather than assumed**, by the method the
//! specification provides for exactly this: `server/discover`, which every
//! modern server MUST implement and no legacy server does. An answer naming a
//! modern version we speak means modern; anything else — a JSON-RPC error, a
//! version we do not implement, a 404 — means legacy, because the one thing a
//! probe like this must not do is turn an old working server into a broken one.
//!
//! The answer is cached per server URL ([`ERA_CACHE`]), since re-probing before
//! every call would double the round trips on a loop that exists to make one.

use crate::mcp::{META_CLIENT_CAPABILITIES, META_CLIENT_INFO, META_PROTOCOL_VERSION};

use crate::http_client::{FetchOptions, HttpClient, HttpError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use thiserror::Error;
use tracing::debug;

/// The legacy revision this client names in an `initialize` handshake.
const MCP_PROTOCOL_VERSION: &str = "2025-11-25";

/// The modern revision this client speaks when a server says it does.
const MCP_MODERN_VERSION: &str = "2026-07-28";

/// How this client names itself, in `_meta` and in the legacy handshake.
const CLIENT_NAME: &str = "aiwebengine-mcp-client";

/// What a tool list falls back to when the server offers no `ttlMs`.
///
/// An hour, which is what this cache always used — and which is only defensible
/// as a *fallback*. A server that says how long its list stays good is answering
/// the question this constant guesses at, and `2026-07-28` made saying so the
/// normal thing: the engine's own server half publishes sixty seconds. Ignoring
/// that while emitting it was the kind of asymmetry that goes unnoticed until a
/// deployment's tool list takes an hour to appear.
const CACHE_TTL_FALLBACK: Duration = Duration::from_secs(3600);

/// A ceiling on the `ttlMs` a server can ask for.
///
/// A server naming a week would otherwise pin a tool list in this process for a
/// week. Cache lifetime is this engine's memory being spent, so the far end's
/// number is taken as advice up to a bound rather than as an instruction.
const CACHE_TTL_MAX: Duration = Duration::from_secs(3600);

/// How long a server's era is remembered before it is probed again.
///
/// Longer than a tool list, because it changes on a deployment rather than on a
/// registration, and shorter than forever, because a server that gains
/// `2026-07-28` should be spoken to properly without restarting the engine.
const ERA_TTL: Duration = Duration::from_secs(6 * 3600);

/// Maximum number of cached MCP servers (LRU eviction)
const MAX_CACHED_SERVERS: usize = 5;

/// MCP Client errors
#[derive(Debug, Error)]
pub enum McpClientError {
    #[error("Network error: {0}")]
    Network(String),

    #[error("Authentication failed: {0}")]
    Auth(String),

    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    #[error("JSON-RPC error: code={0}, message={1}")]
    JsonRpc(i64, String),

    #[error("Invalid response: {0}")]
    InvalidResponse(String),

    #[error("Secret not found: {0}")]
    SecretNotFound(String),

    #[error("Protocol error: {0}")]
    Protocol(String),

    #[error("Blocked URL: {0}")]
    BlockedUrl(String),

    #[error("Timeout")]
    Timeout,
}

impl From<HttpError> for McpClientError {
    fn from(err: HttpError) -> Self {
        match err {
            HttpError::BlockedUrl(reason) => McpClientError::BlockedUrl(reason),
            HttpError::InvalidUrl(reason) => McpClientError::InvalidUrl(reason),
            HttpError::InvalidUrlScheme(scheme) => {
                McpClientError::InvalidUrl(format!("Invalid scheme: {}", scheme))
            }
            HttpError::Timeout => McpClientError::Timeout,
            other => McpClientError::Network(other.to_string()),
        }
    }
}

/// MCP tool schema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    pub name: String,
    pub description: Option<String>,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

/// Which revision a server turned out to speak.
///
/// Deliberately two cases and not a version list: what this client has to
/// decide is whether to stamp `_meta` and skip the handshake, or to send
/// `initialize` and stamp nothing. Every finer distinction is the server's to
/// make.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ServerEra {
    /// Speaks a modern revision; `_meta` carries the version on every request.
    Modern { version: String },
    /// Answers `initialize` and nothing newer. Also the answer for a server we
    /// could not probe, because a failed probe must never downgrade a working
    /// server into a broken one.
    Legacy,
}

/// Cached tool list with timestamp
#[derive(Debug, Clone)]
struct CachedToolList {
    tools: Vec<McpTool>,
    cached_at: Instant,
    /// How long this entry stays good — the server's own `ttlMs` where it gave
    /// one, clamped by [`CACHE_TTL_MAX`], else [`CACHE_TTL_FALLBACK`].
    ttl: Duration,
}

/// Tool list cache with LRU eviction
struct ToolCache {
    cache: HashMap<String, CachedToolList>,
    access_order: Vec<String>, // For LRU tracking
}

impl ToolCache {
    fn new() -> Self {
        Self {
            cache: HashMap::new(),
            access_order: Vec::new(),
        }
    }

    fn get(&mut self, server_url: &str) -> Option<Vec<McpTool>> {
        if let Some(cached) = self.cache.get(server_url) {
            // Check if cache is still valid, against the TTL this entry was
            // stored with rather than against a constant — two servers can
            // legitimately want different lifetimes for their lists.
            if cached.cached_at.elapsed() < cached.ttl {
                // Update access order (move to end = most recently used)
                self.access_order.retain(|url| url != server_url);
                self.access_order.push(server_url.to_string());

                debug!("Cache hit for MCP server: {}", server_url);
                return Some(cached.tools.clone());
            } else {
                // Cache expired
                debug!("Cache expired for MCP server: {}", server_url);
                self.cache.remove(server_url);
                self.access_order.retain(|url| url != server_url);
            }
        }

        None
    }

    fn insert(&mut self, server_url: String, tools: Vec<McpTool>, ttl: Duration) {
        // Evict oldest entry if cache is full
        if self.cache.len() >= MAX_CACHED_SERVERS
            && !self.cache.contains_key(&server_url)
            && let Some(oldest_url) = self.access_order.first().cloned()
        {
            debug!("Evicting oldest MCP server from cache: {}", oldest_url);
            self.cache.remove(&oldest_url);
            self.access_order.remove(0);
        }

        // Insert new entry
        self.cache.insert(
            server_url.clone(),
            CachedToolList {
                tools,
                cached_at: Instant::now(),
                ttl,
            },
        );

        // Update access order
        self.access_order.retain(|url| url != &server_url);
        self.access_order.push(server_url.clone());

        debug!("Cached tools for MCP server: {}", server_url);
    }
}

/// Global tool cache
static TOOL_CACHE: OnceLock<Mutex<ToolCache>> = OnceLock::new();

/// Get or initialize the global tool cache
fn get_tool_cache() -> &'static Mutex<ToolCache> {
    TOOL_CACHE.get_or_init(|| Mutex::new(ToolCache::new()))
}

/// What era each server turned out to speak, and when we found out.
///
/// Separate from [`TOOL_CACHE`] rather than a field on it, because the two
/// answer different questions on different clocks: a tool list changes when
/// somebody deploys a script, an era changes when somebody upgrades a server.
/// Folding them together would mean re-probing the protocol every time a
/// registration moved.
static ERA_CACHE: OnceLock<Mutex<HashMap<String, (ServerEra, Instant)>>> = OnceLock::new();

fn get_era_cache() -> &'static Mutex<HashMap<String, (ServerEra, Instant)>> {
    ERA_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The `ttlMs` a list result asked for, bounded.
///
/// Absent, zero, or unreadable all mean "the server did not say", which is the
/// fallback rather than an error: a list with no hint is the shape every
/// pre-`2026-07-28` server answers with.
fn ttl_from_result(result: &Value) -> Duration {
    match result.get("ttlMs").and_then(|value| value.as_u64()) {
        Some(ms) if ms > 0 => Duration::from_millis(ms).min(CACHE_TTL_MAX),
        _ => CACHE_TTL_FALLBACK,
    }
}

/// MCP Client for connecting to external MCP servers
#[derive(Debug)]
pub struct McpClient {
    server_url: String,
    secret_identifier: String,
    http: HttpClient,
    request_id_counter: std::sync::atomic::AtomicU64,
}

impl McpClient {
    /// Create a new MCP client
    ///
    /// # Arguments
    ///
    /// * `server_url` - URL of the MCP server (e.g., "https://api.githubcopilot.com/mcp/")
    /// * `secret_identifier` - Identifier for the secret to use for authentication (e.g., "github_token")
    ///
    /// The URL comes from a script, which is the whole reason this goes through
    /// [`HttpClient`] rather than a `reqwest` of its own: a client without that
    /// URL and DNS validation is a request forger pointed at whatever network
    /// the engine runs in, and `http://127.0.0.1:3000/mcp` or
    /// `http://169.254.169.254/` would have been as good a server URL as any.
    /// Refusing here rather than at the first call is what keeps a blocked
    /// address from reaching a secret lookup on the way to being refused.
    pub fn new(server_url: String, secret_identifier: String) -> Result<Self, McpClientError> {
        crate::http_client::validate_public_url(&server_url)?;

        Ok(Self {
            server_url,
            secret_identifier,
            http: HttpClient::new()?,
            request_id_counter: std::sync::atomic::AtomicU64::new(1),
        })
    }

    /// A client pointed at a stand-in on loopback.
    ///
    /// Uses the HTTP client's test mode, which is what permits a private
    /// address at all — the production path blocks loopback precisely so that
    /// a script-supplied server URL cannot reach inside the network.
    #[doc(hidden)]
    pub fn for_tests(
        server_url: String,
        secret_identifier: String,
    ) -> Result<Self, McpClientError> {
        Ok(Self {
            server_url,
            secret_identifier,
            http: HttpClient::new_for_tests()?,
            request_id_counter: std::sync::atomic::AtomicU64::new(1),
        })
    }

    /// Initialize connection with the MCP server
    ///
    /// Performs protocol version negotiation. Legacy only: `2026-07-28` removed
    /// this method, so a server we have established speaks it is never sent one.
    fn initialize(&self, script_uri: &str, user_id: Option<&str>) -> Result<Value, McpClientError> {
        let request_id = self.next_request_id();

        let request_body = json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "initialize",
            "params": {
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {
                    "tools": {}
                },
                "clientInfo": {
                    "name": CLIENT_NAME,
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        });

        let response = self.send_request(request_body, script_uri, user_id)?;

        debug!("MCP server initialized: {}", self.server_url);

        Ok(response)
    }

    /// Which era this server speaks, probed once and then remembered.
    ///
    /// `server/discover` is the right probe because a modern server MUST
    /// implement it and a legacy one does not, so one request distinguishes
    /// them without a guess. It is also the one modern method a server answers
    /// without being told a version first, which is what makes it usable before
    /// we know one.
    ///
    /// **Every failure means [`ServerEra::Legacy`].** A JSON-RPC error, a
    /// transport failure, an answer naming only versions we do not implement —
    /// all of it lands on the path that already worked for every server this
    /// client has ever talked to. The cost of guessing legacy at a modern
    /// server is one refused request; the cost of guessing modern at a legacy
    /// one is every request refused, which is a working integration broken by a
    /// probe.
    fn era(&self, script_uri: &str, user_id: Option<&str>) -> ServerEra {
        if let Ok(cache) = get_era_cache().lock()
            && let Some((era, learned_at)) = cache.get(&self.server_url)
            && learned_at.elapsed() < ERA_TTL
        {
            return era.clone();
        }

        let era = self.probe_era(script_uri, user_id);
        debug!("MCP server {} speaks {:?}", self.server_url, era);
        if let Ok(mut cache) = get_era_cache().lock() {
            cache.insert(self.server_url.clone(), (era.clone(), Instant::now()));
        }
        era
    }

    /// One `server/discover`, read for a version we implement.
    fn probe_era(&self, script_uri: &str, user_id: Option<&str>) -> ServerEra {
        let request_id = self.next_request_id();
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "server/discover",
            "params": self.modern_meta(MCP_MODERN_VERSION)
        });

        let Ok(result) = self.send_request(request_body, script_uri, user_id) else {
            return ServerEra::Legacy;
        };

        // Only a version we actually implement counts. A server naming a
        // revision newer than this client's is a modern server we cannot yet
        // speak to modernly, and the honest thing is to fall back rather than
        // to send it `_meta` under rules we do not know.
        let speaks_modern = result
            .get("supportedVersions")
            .and_then(|value| value.as_array())
            .is_some_and(|versions| {
                versions
                    .iter()
                    .any(|version| version.as_str() == Some(MCP_MODERN_VERSION))
            });

        if speaks_modern {
            ServerEra::Modern {
                version: MCP_MODERN_VERSION.to_string(),
            }
        } else {
            // The handshake, once, here rather than before every listing.
            // Best-effort as it always was: some servers require it, some
            // ignore it, and one that refuses it refuses the request that
            // follows too, with a better message than this could give.
            let _ = self.initialize(script_uri, user_id);
            ServerEra::Legacy
        }
    }

    /// The `params` a modern request carries: the version we are speaking, the
    /// capabilities we have, and who we are.
    ///
    /// `clientCapabilities` is present and empty rather than omitted, which is
    /// the distinction the engine's own server half enforces and is right
    /// generally: "I have none" and "I did not say" are different claims, and a
    /// server is entitled to refuse the second. Empty is the true answer here —
    /// this client cannot be elicited from and has no sampling to offer.
    fn modern_meta(&self, version: &str) -> Value {
        json!({
            "_meta": {
                META_PROTOCOL_VERSION: version,
                META_CLIENT_CAPABILITIES: {},
                META_CLIENT_INFO: {
                    "name": CLIENT_NAME,
                    "version": env!("CARGO_PKG_VERSION")
                }
            }
        })
    }

    /// Build a request's `params` for whichever era the server speaks.
    ///
    /// Pure: it stamps `_meta` or it does not, and sends nothing. The legacy
    /// handshake lives in [`McpClient::probe_era`] instead, which is both where
    /// it belongs and one round trip rather than one per listing — `initialize`
    /// here has always been decorative, since the transport is stateless and
    /// its result was discarded.
    fn params_for(&self, era: &ServerEra, mut params: serde_json::Map<String, Value>) -> Value {
        if let ServerEra::Modern { version } = era
            && let Some(meta) = self.modern_meta(version).get("_meta")
        {
            params.insert("_meta".to_string(), meta.clone());
        }
        Value::Object(params)
    }

    /// List available tools from the MCP server
    ///
    /// Results are cached for as long as the server's `ttlMs` says, or an hour
    /// where it says nothing.
    pub fn list_tools(
        &self,
        script_uri: &str,
        user_id: Option<&str>,
    ) -> Result<Vec<McpTool>, McpClientError> {
        // Check cache first
        {
            let mut cache = get_tool_cache()
                .lock()
                .map_err(|_| McpClientError::Protocol("Cache lock poisoned".to_string()))?;

            if let Some(tools) = cache.get(&self.server_url) {
                return Ok(tools);
            }
        }

        let era = self.era(script_uri, user_id);
        let params = self.params_for(&era, serde_json::Map::new());

        let request_id = self.next_request_id();
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "tools/list",
            "params": params
        });

        let response = self.send_request(request_body, script_uri, user_id)?;

        // Parse tools from response
        let tools_array = response
            .get("tools")
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                McpClientError::InvalidResponse("Missing 'tools' array in response".to_string())
            })?;

        let tools: Vec<McpTool> = serde_json::from_value(Value::Array(tools_array.clone()))
            .map_err(|e| McpClientError::InvalidResponse(format!("Invalid tool schema: {}", e)))?;

        // Cache the results
        {
            let mut cache = get_tool_cache()
                .lock()
                .map_err(|_| McpClientError::Protocol("Cache lock poisoned".to_string()))?;

            cache.insert(
                self.server_url.clone(),
                tools.clone(),
                ttl_from_result(&response),
            );
        }

        debug!(
            "Listed {} tools from MCP server: {}",
            tools.len(),
            self.server_url
        );

        Ok(tools)
    }

    /// Call a tool on the MCP server
    ///
    /// # Arguments
    ///
    /// * `name` - Name of the tool to call
    /// * `arguments` - Tool arguments as JSON object
    pub fn call_tool(
        &self,
        name: String,
        arguments: Value,
        script_uri: &str,
        user_id: Option<&str>,
    ) -> Result<Value, McpClientError> {
        // The era matters here as much as on a listing — a modern server
        // refuses a `tools/call` that names no version, and this arm used to
        // send neither a handshake nor `_meta`.
        let era = self.era(script_uri, user_id);
        let mut params = serde_json::Map::new();
        params.insert("name".to_string(), Value::String(name.clone()));
        params.insert("arguments".to_string(), arguments);
        let params = self.params_for(&era, params);

        let request_id = self.next_request_id();
        let request_body = json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "tools/call",
            "params": params
        });

        let response = self.send_request(request_body, script_uri, user_id)?;

        debug!("Called tool '{}' on MCP server: {}", name, self.server_url);

        Ok(response)
    }

    /// Send a JSON-RPC request to the MCP server
    fn send_request(
        &self,
        request_body: Value,
        script_uri: &str,
        user_id: Option<&str>,
    ) -> Result<Value, McpClientError> {
        // Resolve authorization token from database (user_secrets first, then script_secrets).
        // Environment variables and config files are never consulted.
        let token =
            crate::repository::resolve_secret_db(script_uri, &self.secret_identifier, user_id)
                .ok_or_else(|| McpClientError::SecretNotFound(self.secret_identifier.clone()))?;

        // The token is resolved here rather than handed to `fetch` as a
        // `{{secret:...}}` template, which would now carry the `Bearer `
        // prefix perfectly well. What the template cannot do is fail as
        // [`McpClientError::SecretNotFound`]: a missing secret has to be this
        // client's error, named for the server it could not authenticate to,
        // rather than an `HttpError` surfacing from inside a request that was
        // never going to be made.
        let headers = HashMap::from([
            ("Content-Type".to_string(), "application/json".to_string()),
            ("Authorization".to_string(), format!("Bearer {}", token)),
        ]);

        // Through the shared client: URL and DNS validation, a validated
        // redirect at every hop with credentials dropped when the host
        // changes, a bounded response, and the host budget applied to the
        // timeout — the same treatment a script's `fetch` gets, which is the
        // point of not having a client of our own here.
        let response = self.http.fetch(
            self.server_url.clone(),
            FetchOptions {
                method: "POST".to_string(),
                headers: Some(headers),
                body: Some(request_body.to_string()),
                timeout_ms: None,
                // JSON-RPC over HTTP; a response that is not text is a protocol
                // error rather than something to base64 and hand on.
                binary: false,
            },
            Some(script_uri),
            user_id,
        )?;

        // Check status code
        let status = response.status;
        if status == 401 || status == 403 {
            return Err(McpClientError::Auth(format!(
                "HTTP {} - Check your authentication token",
                status
            )));
        }

        if !response.ok {
            return Err(McpClientError::Network(format!(
                "HTTP {}: {}",
                status, response.body
            )));
        }

        // Parse JSON-RPC response
        let response_text = response.body;

        // Handle Server-Sent Events (SSE) format if present
        let json_text = if response_text.starts_with("event:") || response_text.starts_with("data:")
        {
            // Parse SSE format - extract JSON from data: lines
            response_text
                .lines()
                .find(|line| line.starts_with("data:"))
                .and_then(|line| line.strip_prefix("data:").map(|s| s.trim()))
                .ok_or_else(|| {
                    McpClientError::InvalidResponse(
                        "SSE response missing 'data:' field".to_string(),
                    )
                })?
                .to_string()
        } else {
            response_text.clone()
        };

        let response_body: Value = serde_json::from_str(&json_text).map_err(|e| {
            // Include the actual response text (truncated) in the error for debugging
            let preview = if response_text.len() > 200 {
                format!("{}...", &response_text[..200])
            } else {
                response_text.clone()
            };
            McpClientError::InvalidResponse(format!(
                "Invalid JSON: {}. Response preview: {}",
                e, preview
            ))
        })?;

        // Check for JSON-RPC error
        if let Some(error) = response_body.get("error") {
            let code = error.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
            let message = error
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown error")
                .to_string();

            return Err(McpClientError::JsonRpc(code, message));
        }

        // Extract result
        let result = response_body
            .get("result")
            .ok_or_else(|| {
                McpClientError::InvalidResponse("Missing 'result' field in response".to_string())
            })?
            .clone();

        Ok(result)
    }

    /// Get next request ID
    fn next_request_id(&self) -> u64 {
        self.request_id_counter
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_cache_basic() {
        let mut cache = ToolCache::new();

        let tools = vec![McpTool {
            name: "test_tool".to_string(),
            description: Some("Test tool".to_string()),
            input_schema: json!({"type": "object"}),
        }];

        // Insert and retrieve
        cache.insert(
            "https://example.com".to_string(),
            tools.clone(),
            CACHE_TTL_FALLBACK,
        );
        let retrieved = cache.get("https://example.com").unwrap();

        assert_eq!(retrieved.len(), 1);
        assert_eq!(retrieved[0].name, "test_tool");
    }

    #[test]
    fn test_tool_cache_lru_eviction() {
        let mut cache = ToolCache::new();

        let tools = vec![McpTool {
            name: "tool".to_string(),
            description: None,
            input_schema: json!({}),
        }];

        // Fill cache to max capacity
        for i in 0..MAX_CACHED_SERVERS {
            cache.insert(
                format!("https://server{}.com", i),
                tools.clone(),
                CACHE_TTL_FALLBACK,
            );
        }

        assert_eq!(cache.cache.len(), MAX_CACHED_SERVERS);

        // Add one more - should evict the oldest (server0)
        cache.insert(
            "https://server-new.com".to_string(),
            tools.clone(),
            CACHE_TTL_FALLBACK,
        );

        assert_eq!(cache.cache.len(), MAX_CACHED_SERVERS);
        assert!(cache.get("https://server0.com").is_none());
        assert!(cache.get("https://server-new.com").is_some());
    }

    /// A server that says how long its list stays good is believed, up to the
    /// ceiling; one that says nothing gets the fallback.
    #[test]
    fn a_server_ttl_is_taken_as_advice_within_a_bound() {
        assert_eq!(
            ttl_from_result(&json!({ "tools": [], "ttlMs": 60_000 })),
            Duration::from_secs(60),
            "the engine's own server publishes sixty seconds and should be heard"
        );

        // Nothing said, said as zero, and said as something unreadable are one
        // answer: fall back. None of the three is an error worth failing a
        // listing over, and every pre-2026-07-28 server is the first case.
        for result in [
            json!({ "tools": [] }),
            json!({ "tools": [], "ttlMs": 0 }),
            json!({ "tools": [], "ttlMs": "soon" }),
        ] {
            assert_eq!(ttl_from_result(&result), CACHE_TTL_FALLBACK, "{}", result);
        }

        // A week is advice this process does not have to take: the memory being
        // pinned is ours.
        assert_eq!(
            ttl_from_result(&json!({ "ttlMs": 7 * 24 * 3600 * 1000u64 })),
            CACHE_TTL_MAX
        );
    }

    /// Both `_meta` keys, because the engine's own server half refuses a modern
    /// request that declares no capabilities — "I have none" and "I did not
    /// say" being different claims — and other servers are entitled to as well.
    #[test]
    fn a_modern_request_declares_its_version_and_its_capabilities() {
        let client = McpClient::new(
            "https://api.example.com/mcp/".to_string(),
            "test_token".to_string(),
        )
        .expect("client should build");

        let meta = client.modern_meta(MCP_MODERN_VERSION);
        let meta = &meta["_meta"];

        assert_eq!(meta[META_PROTOCOL_VERSION], MCP_MODERN_VERSION);
        assert!(
            meta.get(META_CLIENT_CAPABILITIES).is_some(),
            "capabilities must be present even when empty: {}",
            meta
        );
        assert_eq!(meta[META_CLIENT_INFO]["name"], CLIENT_NAME);
    }

    /// A legacy request carries no `_meta` at all, and a modern one sends no
    /// handshake. Asserted on the params the two eras build, since that is the
    /// one place the distinction is made.
    #[test]
    fn the_two_eras_do_not_contaminate_each_other() {
        let client = McpClient::new(
            "https://api.example.com/mcp/".to_string(),
            "test_token".to_string(),
        )
        .expect("client should build");

        let modern = client.params_for(
            &ServerEra::Modern {
                version: MCP_MODERN_VERSION.to_string(),
            },
            serde_json::Map::new(),
        );
        assert_eq!(modern["_meta"][META_PROTOCOL_VERSION], MCP_MODERN_VERSION);

        let legacy = client.params_for(&ServerEra::Legacy, serde_json::Map::new());
        assert!(
            legacy.get("_meta").is_none(),
            "a legacy request must not carry modern metadata: {}",
            legacy
        );
    }

    #[test]
    fn test_mcp_client_creation() {
        let client = McpClient::new(
            "https://api.example.com/mcp/".to_string(),
            "test_token".to_string(),
        );

        assert!(client.is_ok());
    }

    #[test]
    fn test_mcp_client_invalid_url() {
        let client = McpClient::new("not-a-url".to_string(), "test_token".to_string());

        assert!(client.is_err());
        match client.unwrap_err() {
            McpClientError::InvalidUrl(_) => {}
            _ => panic!("Expected InvalidUrl error"),
        }
    }

    /// The gap this closes: a script naming the engine's own loopback address
    /// as its "external" MCP server, which the scheme check alone allowed.
    #[test]
    fn refuses_loopback_and_localhost() {
        for url in [
            "http://127.0.0.1:3000/mcp",
            "http://localhost:3000/mcp",
            "http://[::1]:3000/mcp",
        ] {
            match McpClient::new(url.to_string(), "test_token".to_string()) {
                Err(McpClientError::BlockedUrl(_)) => {}
                other => panic!(
                    "{} should be blocked, got {:?}",
                    url,
                    other.map(|_| "client")
                ),
            }
        }
    }

    /// Private ranges and the cloud metadata address, which is the one a
    /// request forger reaches for first.
    #[test]
    fn refuses_private_and_link_local_addresses() {
        for url in [
            "http://10.0.0.5/mcp",
            "http://192.168.1.1/mcp",
            "http://169.254.169.254/latest/meta-data/",
        ] {
            match McpClient::new(url.to_string(), "test_token".to_string()) {
                Err(McpClientError::BlockedUrl(_)) => {}
                other => panic!(
                    "{} should be blocked, got {:?}",
                    url,
                    other.map(|_| "client")
                ),
            }
        }
    }

    /// The escape hatch is real, and it is the only thing that permits a
    /// private address — a test pointing at a stand-in on loopback.
    #[test]
    fn test_client_allows_loopback() {
        assert!(
            McpClient::for_tests("http://127.0.0.1:3000/mcp".to_string(), "t".to_string()).is_ok()
        );
    }

    #[test]
    fn test_mcp_client_invalid_scheme() {
        let client = McpClient::new("ftp://example.com".to_string(), "test_token".to_string());

        assert!(client.is_err());
        match client.unwrap_err() {
            McpClientError::InvalidUrl(msg) => {
                assert!(msg.contains("Invalid scheme"));
            }
            _ => panic!("Expected InvalidUrl error"),
        }
    }
}
