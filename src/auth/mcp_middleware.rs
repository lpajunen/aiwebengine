/// MCP Authorization Middleware
///
/// Implements OAuth 2.0 Bearer token authentication for MCP endpoints
/// with resource indicator support (RFC 8707) and WWW-Authenticate challenges.
use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::sync::Arc;

use crate::auth::{AuthError, AuthManager, AuthSession};
use crate::security::client_ip;

/// Extension to store authenticated session in request
#[derive(Clone, Debug)]
pub struct McpAuthSession {
    pub session: AuthSession,
}

/// Extract Bearer token from Authorization header
fn extract_bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|auth| auth.strip_prefix("Bearer ").map(|s| s.to_string()))
}

/// The one path a bearer token is audience-checked on, and so the one resource
/// this engine issues audiences for.
pub const MCP_ENDPOINT_PATH: &str = "/mcp";

/// Name the resource this request is asking for, in the form a session's
/// audience is compared against.
///
/// Host-qualified on purpose. `/mcp` on one host and `/mcp` on another are two
/// resources — an engine serving a game and a management surface publishes both
/// — and a token issued for one must not reach the other. A bearer token is not
/// bound by cookie host scoping, so this is the only thing that separates them.
///
/// The host is the one the request resolves to rather than the raw header,
/// because that is the host the audience was minted against: a request arriving
/// on something unconfigured — a direct IP, a tunnel, a port-forward — is served
/// the default host's content everywhere else, and naming it by its raw header
/// here would refuse a token that is otherwise exactly right for what it
/// reaches.
fn requested_resource(headers: &HeaderMap, path: &str) -> Option<String> {
    if !path.starts_with(MCP_ENDPOINT_PATH) {
        return None;
    }

    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());

    Some(format!("{}{}", crate::hosts::resolved_host(host), path))
}

/// Where a client that got a 401 here can read what token this endpoint wants.
///
/// RFC 9728 §5.1: the challenge carries the protected-resource document's URL,
/// which is how a client discovers the authorization server and — the part that
/// matters here — the resource identifier to ask a token for.
fn resource_metadata_url(headers: &HeaderMap, path: &str) -> Option<String> {
    let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
    let origin = crate::hosts::origin(&crate::hosts::resolved_host(host));
    if origin.is_empty() {
        return None;
    }

    Some(format!(
        "{}{}{}",
        origin,
        crate::auth::metadata::PROTECTED_RESOURCE_PATH,
        path
    ))
}

/// Create WWW-Authenticate challenge header for MCP endpoints
fn create_auth_challenge(
    realm: &str,
    error: Option<&str>,
    error_description: Option<&str>,
) -> String {
    challenge_with_metadata(realm, error, error_description, None)
}

/// The same challenge, naming where the resource's metadata is published.
///
/// A client that has not discovered this engine yet learns from here which
/// resource identifier to request a token for, so the answer to a 401 is a
/// token that works rather than another 401.
fn challenge_with_metadata(
    realm: &str,
    error: Option<&str>,
    error_description: Option<&str>,
    resource_metadata: Option<&str>,
) -> String {
    let mut challenge = format!("Bearer realm=\"{}\"", realm);

    if let Some(err) = error {
        challenge.push_str(&format!(", error=\"{}\"", err));
    }

    if let Some(desc) = error_description {
        challenge.push_str(&format!(", error_description=\"{}\"", desc));
    }

    if let Some(url) = resource_metadata {
        challenge.push_str(&format!(", resource_metadata=\"{}\"", url));
    }

    challenge
}

/// MCP authorization middleware - requires valid Bearer token
///
/// Returns 401 Unauthorized with WWW-Authenticate header if:
/// - No Authorization header present
/// - Invalid Bearer token format
/// - Session validation fails
/// - Resource indicator validation fails (if applicable)
pub async fn mcp_auth_middleware(
    State(auth_manager): State<Arc<AuthManager>>,
    mut request: Request,
    next: Next,
) -> Response {
    let headers = request.headers();
    let uri = request.uri();

    tracing::debug!("MCP request: {} {}", request.method(), uri);

    // Extract token from Bearer header only
    let token = match extract_bearer_token(headers) {
        Some(t) => {
            tracing::info!(
                "MCP: Received Bearer token (length: {}, first 10 chars: {})",
                t.len(),
                t.chars().take(10).collect::<String>()
            );
            t
        }
        None => {
            tracing::warn!("MCP: No Bearer token found in request headers");
            // No token provided - return 401 with WWW-Authenticate challenge
            let metadata_url = resource_metadata_url(headers, request.uri().path());
            let challenge = challenge_with_metadata(
                "MCP API",
                Some("invalid_token"),
                Some("Bearer token required for MCP endpoints"),
                metadata_url.as_deref(),
            );

            return (
                StatusCode::UNAUTHORIZED,
                [(header::WWW_AUTHENTICATE, challenge)],
                "Unauthorized: Bearer token required",
            )
                .into_response();
        }
    };

    let ip_addr = client_ip::from_headers(headers);
    let user_agent = client_ip::user_agent_from_headers(headers);
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let requested_resource = requested_resource(headers, request.uri().path());
    let resource = requested_resource.as_deref();

    // Validate session with resource indicator
    let session = match auth_manager
        .validate_session_with_resource(&token, &ip_addr, &user_agent, host.as_deref(), resource)
        .await
    {
        Ok(s) => s,
        Err(e) => {
            // Log the specific AuthError reason at error level for diagnostics
            tracing::error!("MCP session validation error: {:?}", e);
            tracing::error!(
                "Token (first 10 chars): {}",
                &token.chars().take(10).collect::<String>()
            );
            tracing::error!(
                "IP: {}, UA: {}, Resource: {:?}",
                ip_addr,
                user_agent,
                resource
            );
            // Session validation failed - return 401 with appropriate error
            let (error, error_desc) = match e {
                AuthError::NoSession => ("invalid_token", "Session not found or expired"),
                AuthError::SessionError(ref msg) => ("invalid_token", msg.as_str()),
                AuthError::Session(ref session_err) => {
                    // Log the specific SessionError
                    tracing::error!("Session validation SessionError: {:?}", session_err);
                    ("invalid_token", "Session validation failed")
                }
                AuthError::TokenExpired => ("invalid_token", "Session expired"),
                AuthError::InsufficientPermissions => (
                    "insufficient_scope",
                    "Insufficient permissions for this resource",
                ),
                _ => {
                    tracing::error!("Unhandled auth error type");
                    ("invalid_token", "Session validation failed")
                }
            };

            let metadata_url = resource_metadata_url(headers, request.uri().path());
            let challenge = challenge_with_metadata(
                "MCP API",
                Some(error),
                Some(error_desc),
                metadata_url.as_deref(),
            );

            return (
                StatusCode::UNAUTHORIZED,
                [(header::WWW_AUTHENTICATE, challenge)],
                format!("Unauthorized: {}", error_desc),
            )
                .into_response();
        }
    };

    // Store session in request extensions
    request.extensions_mut().insert(McpAuthSession { session });

    // Continue to next middleware/handler
    next.run(request).await
}

/// Optional MCP auth middleware - allows unauthenticated access but extracts session if present
pub async fn optional_mcp_auth_middleware(
    State(auth_manager): State<Arc<AuthManager>>,
    mut request: Request,
    next: Next,
) -> Response {
    let headers = request.headers();

    if let Some(token) = extract_bearer_token(headers) {
        let ip_addr = client_ip::from_headers(headers);
        let user_agent = client_ip::user_agent_from_headers(headers);
        let host = headers
            .get(header::HOST)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let requested_resource = requested_resource(headers, request.uri().path());
        let resource = requested_resource.as_deref();

        if let Ok(session) = auth_manager
            .validate_session_with_resource(
                &token,
                &ip_addr,
                &user_agent,
                host.as_deref(),
                resource,
            )
            .await
        {
            request.extensions_mut().insert(McpAuthSession { session });
        }
    }

    next.run(request).await
}

/// Role-based authorization middleware for MCP endpoints
/// Requires admin or editor role
pub async fn mcp_require_editor_middleware(request: Request, next: Next) -> Response {
    // Extract session from extensions (must be run after mcp_auth_middleware)
    let session = match request.extensions().get::<McpAuthSession>() {
        Some(auth_session) => &auth_session.session,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                [(
                    header::WWW_AUTHENTICATE,
                    create_auth_challenge(
                        "MCP API",
                        Some("invalid_token"),
                        Some("Authentication required"),
                    ),
                )],
                "Unauthorized: Authentication required",
            )
                .into_response();
        }
    };

    // Check if user is admin or editor
    if !session.is_admin && !session.is_editor {
        let challenge = create_auth_challenge(
            "MCP API",
            Some("insufficient_scope"),
            Some("Editor or administrator role required"),
        );

        return (
            StatusCode::FORBIDDEN,
            [(header::WWW_AUTHENTICATE, challenge)],
            "Forbidden: Editor or administrator role required",
        )
            .into_response();
    }

    next.run(request).await
}

/// Admin-only authorization middleware for MCP endpoints
pub async fn mcp_require_admin_middleware(request: Request, next: Next) -> Response {
    let session = match request.extensions().get::<McpAuthSession>() {
        Some(auth_session) => &auth_session.session,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                [(
                    header::WWW_AUTHENTICATE,
                    create_auth_challenge(
                        "MCP API",
                        Some("invalid_token"),
                        Some("Authentication required"),
                    ),
                )],
                "Unauthorized: Authentication required",
            )
                .into_response();
        }
    };

    if !session.is_admin {
        let challenge = create_auth_challenge(
            "MCP API",
            Some("insufficient_scope"),
            Some("Administrator role required"),
        );

        return (
            StatusCode::FORBIDDEN,
            [(header::WWW_AUTHENTICATE, challenge)],
            "Forbidden: Administrator role required",
        )
            .into_response();
    }

    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_bearer_token() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            "Bearer test_token_123".parse().unwrap(),
        );

        let token = extract_bearer_token(&headers);
        assert_eq!(token, Some("test_token_123".to_string()));
    }

    #[test]
    fn test_extract_bearer_token_missing() {
        let headers = HeaderMap::new();
        let token = extract_bearer_token(&headers);
        assert_eq!(token, None);
    }

    #[test]
    fn test_extract_bearer_token_wrong_scheme() {
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Basic dXNlcjpwYXNz".parse().unwrap());

        let token = extract_bearer_token(&headers);
        assert_eq!(token, None);
    }

    /// The engine's own 401 is a client's first hint about what token to get.
    /// Without the document's URL the client is left guessing at a resource
    /// identifier, and guessing wrong mints a token this endpoint refuses.
    #[test]
    fn a_challenge_names_where_the_resource_metadata_lives() {
        let challenge = challenge_with_metadata(
            "MCP API",
            Some("invalid_token"),
            Some("Bearer token required for MCP endpoints"),
            Some("https://example.com/.well-known/oauth-protected-resource/mcp"),
        );

        assert!(
            challenge.ends_with(
                ", resource_metadata=\"https://example.com/.well-known/oauth-protected-resource/mcp\""
            ),
            "challenge should carry the document URL: {}",
            challenge
        );
    }

    /// Host binding is not configured in a unit test, so the header stands as
    /// sent. What matters here is that the path is part of the name: an
    /// audience is matched on both, and a token for the host alone is a token
    /// for nothing.
    #[test]
    fn a_requested_resource_names_the_host_and_the_path() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, "game.example.com".parse().unwrap());

        assert_eq!(
            requested_resource(&headers, "/mcp"),
            Some("game.example.com/mcp".to_string())
        );
        assert_eq!(requested_resource(&headers, "/graphql"), None);
    }

    #[test]
    fn test_create_auth_challenge() {
        let challenge = create_auth_challenge("MCP API", None, None);
        assert_eq!(challenge, "Bearer realm=\"MCP API\"");

        let challenge =
            create_auth_challenge("MCP API", Some("invalid_token"), Some("Token expired"));
        assert_eq!(
            challenge,
            "Bearer realm=\"MCP API\", error=\"invalid_token\", error_description=\"Token expired\""
        );
    }
}
