//! MCP Client Module
//!
//! Provides client functionality for connecting to external Model Context Protocol (MCP) servers.
//! Implements JSON-RPC 2.0 protocol for tool discovery and invocation.
//!
//! # Features
//!
//! 1. Protocol version negotiation (supports 2025-11-25 and backward compatibility)
//! 2. Tool discovery via `tools/list`
//! 3. Tool invocation via `tools/call`
//! 4. Simple TTL-based caching (1 hour, max 5 servers with LRU eviction)
//! 5. Secret injection for Authorization headers
//! 6. Error handling for network, auth, and protocol errors
//!
//! Requests go out through [`crate::http_client::HttpClient`] rather than a
//! `reqwest` of its own, so a server URL — which comes from a script — gets the
//! same URL and DNS validation, the same per-hop redirect checking and the same
//! response ceiling that a script's `fetch` gets.

use crate::http_client::{FetchOptions, HttpClient, HttpError};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use thiserror::Error;
use tracing::debug;

/// MCP protocol version (latest stable)
const MCP_PROTOCOL_VERSION: &str = "2025-11-25";

/// Tool list cache TTL (1 hour)
const CACHE_TTL: Duration = Duration::from_secs(3600);

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

/// Cached tool list with timestamp
#[derive(Debug, Clone)]
struct CachedToolList {
    tools: Vec<McpTool>,
    cached_at: Instant,
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
            // Check if cache is still valid
            if cached.cached_at.elapsed() < CACHE_TTL {
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

    fn insert(&mut self, server_url: String, tools: Vec<McpTool>) {
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
    /// Performs protocol version negotiation
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
                    "name": "aiwebengine-mcp-client",
                    "version": "1.0.0"
                }
            }
        });

        let response = self.send_request(request_body, script_uri, user_id)?;

        debug!("MCP server initialized: {}", self.server_url);

        Ok(response)
    }

    /// List available tools from the MCP server
    ///
    /// Results are cached for 1 hour
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

        // Initialize connection (optional, some servers may not require it)
        let _ = self.initialize(script_uri, user_id);

        // List tools
        let request_id = self.next_request_id();

        let request_body = json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "tools/list",
            "params": {}
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

            cache.insert(self.server_url.clone(), tools.clone());
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
        let request_id = self.next_request_id();

        let request_body = json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "tools/call",
            "params": {
                "name": name,
                "arguments": arguments
            }
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
        cache.insert("https://example.com".to_string(), tools.clone());
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
            cache.insert(format!("https://server{}.com", i), tools.clone());
        }

        assert_eq!(cache.cache.len(), MAX_CACHED_SERVERS);

        // Add one more - should evict the oldest (server0)
        cache.insert("https://server-new.com".to_string(), tools.clone());

        assert_eq!(cache.cache.len(), MAX_CACHED_SERVERS);
        assert!(cache.get("https://server0.com").is_none());
        assert!(cache.get("https://server-new.com").is_some());
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
