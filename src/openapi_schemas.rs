//! OpenAPI Schema Definitions
//!
//! This module contains all the schema definitions used in the OpenAPI specification
//! for both Rust endpoints and documentation purposes.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

// ============================================================================
// Unified Error Response Schemas
// ============================================================================

/// Standard error response for 4xx and 5xx errors
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ErrorResponse {
    /// Error message describing what went wrong
    pub error: String,
}

/// Validation error response for 400 Bad Request
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ValidationErrorResponse {
    /// Error message describing the validation failure
    pub error: String,
    /// Optional field-level validation errors
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Vec<String>>,
}

/// Unauthorized error response for 401 Unauthorized
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct UnauthorizedErrorResponse {
    /// Error message indicating authentication is required
    pub error: String,
}

// ============================================================================
// Health Check Schemas
// ============================================================================

/// Build and version metadata
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[schema(example = json!({
    "cargo": "0.1.0",
    "git_commit": "abc123def456",
    "git_commit_timestamp": "2026-01-18T10:30:00+00:00",
    "build_timestamp": "2026-01-18T10:35:00+00:00"
}))]
pub struct BuildVersion {
    /// Cargo package version from Cargo.toml
    #[schema(example = "0.1.0")]
    pub cargo: String,
    /// Git commit hash (short SHA) - empty if git unavailable
    #[schema(example = "abc123def456")]
    pub git_commit: String,
    /// Git commit timestamp in ISO 8601 format - empty if git unavailable
    #[schema(example = "2026-01-18T10:30:00+00:00")]
    pub git_commit_timestamp: String,
    /// Build timestamp in ISO 8601 format - empty if unavailable
    #[schema(example = "2026-01-18T10:35:00+00:00")]
    pub build_timestamp: String,
}

/// Health check response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HealthResponse {
    /// Overall status of the service
    pub status: String,
    /// Current timestamp
    pub timestamp: String,
    /// Build and version information
    pub version: BuildVersion,
    /// Database connection status
    pub database: String,
}

/// Detailed cluster health response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ClusterHealthResponse {
    /// Overall cluster status
    pub status: String,
    /// Current timestamp
    pub timestamp: String,
    /// Build and version information
    pub version: BuildVersion,
    /// Database pool status
    pub database: DatabaseStatus,
    /// Script engine status
    pub scripts: ScriptStatus,
    /// System information
    pub system: SystemInfo,
}

/// Database connection pool status
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DatabaseStatus {
    /// Whether database is connected
    pub connected: bool,
    /// Number of active connections
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_connections: Option<u32>,
    /// Number of idle connections
    #[serde(skip_serializing_if = "Option::is_none")]
    pub idle_connections: Option<u32>,
}

/// Script engine status
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ScriptStatus {
    /// Total number of scripts loaded
    pub total: usize,
    /// Number of initialized scripts
    pub initialized: usize,
}

/// System information
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SystemInfo {
    /// Service version
    pub version: String,
    /// Server uptime in seconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uptime_seconds: Option<u64>,
}

// ============================================================================
// GraphQL Schemas (HTTP Transport Layer)
// ============================================================================

/// GraphQL request - generic JSON payload
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct GraphQLRequest {
    /// GraphQL query string
    pub query: String,
    /// Optional operation name
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "operationName")]
    pub operation_name: Option<String>,
    /// Optional variables as JSON object
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variables: Option<serde_json::Value>,
}

/// GraphQL response - generic JSON payload
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct GraphQLResponse {
    /// Response data (null if errors occurred)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    /// Errors encountered during execution
    #[serde(skip_serializing_if = "Option::is_none")]
    pub errors: Option<Vec<GraphQLError>>,
}

/// GraphQL error object
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct GraphQLError {
    /// Error message
    pub message: String,
    /// Optional path to the field that caused the error
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<Vec<serde_json::Value>>,
    /// Optional extensions with additional error information
    #[serde(skip_serializing_if = "Option::is_none")]
    pub extensions: Option<serde_json::Value>,
}

// ============================================================================
// MCP (Model Context Protocol) JSON-RPC Schemas
// ============================================================================

/// MCP JSON-RPC 2.0 request
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct McpRpcRequest {
    /// JSON-RPC version (must be "2.0")
    pub jsonrpc: String,
    /// Request method name
    pub method: String,
    /// Optional request parameters
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    /// Request ID (can be string, number, or null)
    pub id: serde_json::Value,
}

/// MCP JSON-RPC 2.0 response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct McpRpcResponse {
    /// JSON-RPC version (must be "2.0")
    pub jsonrpc: String,
    /// Result data (present on success)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// Error object (present on failure)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<McpRpcError>,
    /// Request ID matching the request
    pub id: serde_json::Value,
}

/// MCP JSON-RPC error object
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct McpRpcError {
    /// Error code
    pub code: i32,
    /// Error message
    pub message: String,
    /// Optional additional error data
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// MCP tool descriptor
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ToolDescriptor {
    /// Tool name
    pub name: String,
    /// Tool description
    pub description: String,
    /// JSON Schema for input validation
    #[serde(rename = "inputSchema")]
    pub input_schema: serde_json::Value,
}

/// MCP tools list response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct McpToolsListResponse {
    /// JSON-RPC version
    pub jsonrpc: String,
    /// Request ID
    pub id: serde_json::Value,
    /// Result containing tools array
    pub result: McpToolsList,
}

/// MCP tools list result
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct McpToolsList {
    /// Array of available tools
    pub tools: Vec<ToolDescriptor>,
}

// ============================================================================
// OAuth2 / Authentication Schemas
// ============================================================================

/// OAuth2 token response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct OAuth2TokenResponse {
    /// Access token
    pub access_token: String,
    /// Token type (usually "Bearer")
    pub token_type: String,
    /// Expiration time in seconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_in: Option<u64>,
    /// Refresh token (if issued)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// Scope of the access token
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

/// Authentication status response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AuthStatusResponse {
    /// Whether user is authenticated
    pub authenticated: bool,
    /// User ID (if authenticated)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    /// Username (if authenticated)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    /// User roles
    #[serde(skip_serializing_if = "Option::is_none")]
    pub roles: Option<Vec<String>>,
}
