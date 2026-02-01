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

use crate::secrets::SecretsManager;
use reqwest::blocking::Client;
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

/// Default request timeout (30 seconds)
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

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

    #[error("Timeout")]
    Timeout,
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
        self.access_order.push(server_url);

        debug!(
            "Cached tools for MCP server: {}",
            self.access_order.last().unwrap()
        );
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
    client: Client,
    request_id_counter: std::sync::atomic::AtomicU64,
}

impl McpClient {
    /// Create a new MCP client
    ///
    /// # Arguments
    ///
    /// * `server_url` - URL of the MCP server (e.g., "https://api.githubcopilot.com/mcp/")
    /// * `secret_identifier` - Identifier for the secret to use for authentication (e.g., "github_token")
    pub fn new(server_url: String, secret_identifier: String) -> Result<Self, McpClientError> {
        // Validate URL
        let url = reqwest::Url::parse(&server_url)
            .map_err(|e| McpClientError::InvalidUrl(e.to_string()))?;

        if url.scheme() != "http" && url.scheme() != "https" {
            return Err(McpClientError::InvalidUrl(format!(
                "Invalid scheme: {}. Only http and https are supported",
                url.scheme()
            )));
        }

        // Create HTTP client
        let client = Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .use_rustls_tls()
            .build()
            .map_err(|e| McpClientError::Network(e.to_string()))?;

        Ok(Self {
            server_url,
            secret_identifier,
            client,
            request_id_counter: std::sync::atomic::AtomicU64::new(1),
        })
    }

    /// Initialize connection with the MCP server
    ///
    /// Performs protocol version negotiation
    fn initialize(&self, secrets_manager: &SecretsManager) -> Result<Value, McpClientError> {
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

        let response = self.send_request(request_body, secrets_manager)?;

        debug!("MCP server initialized: {}", self.server_url);

        Ok(response)
    }

    /// List available tools from the MCP server
    ///
    /// Results are cached for 1 hour
    pub fn list_tools(
        &self,
        secrets_manager: &SecretsManager,
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
        let _ = self.initialize(secrets_manager);

        // List tools
        let request_id = self.next_request_id();

        let request_body = json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "method": "tools/list",
            "params": {}
        });

        let response = self.send_request(request_body, secrets_manager)?;

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
        secrets_manager: &SecretsManager,
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

        let response = self.send_request(request_body, secrets_manager)?;

        debug!("Called tool '{}' on MCP server: {}", name, self.server_url);

        Ok(response)
    }

    /// Send a JSON-RPC request to the MCP server
    fn send_request(
        &self,
        request_body: Value,
        secrets_manager: &SecretsManager,
    ) -> Result<Value, McpClientError> {
        // Get authorization token from secrets manager
        // Note: MCP client is internal system use, constraints are enforced at script fetch() level
        #[allow(deprecated)]
        let token = secrets_manager
            .get(&self.secret_identifier)
            .ok_or_else(|| McpClientError::SecretNotFound(self.secret_identifier.clone()))?;

        // Build request
        let response = self
            .client
            .post(&self.server_url)
            .header("Content-Type", "application/json")
            .header("Authorization", format!("Bearer {}", token))
            .json(&request_body)
            .send()
            .map_err(|e| {
                if e.is_timeout() {
                    McpClientError::Timeout
                } else {
                    McpClientError::Network(e.to_string())
                }
            })?;

        // Check status code
        let status = response.status();
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(McpClientError::Auth(format!(
                "HTTP {} - Check your authentication token",
                status.as_u16()
            )));
        }

        if !status.is_success() {
            let error_text = response
                .text()
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(McpClientError::Network(format!(
                "HTTP {}: {}",
                status.as_u16(),
                error_text
            )));
        }

        // Parse JSON-RPC response
        let response_text = response.text().map_err(|e| {
            McpClientError::InvalidResponse(format!("Failed to read response body: {}", e))
        })?;

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
