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

/// Name the resource this request is asking for, in the form a session's
/// audience is compared against.
///
/// Host-qualified on purpose. `/mcp` on one host and `/mcp` on another are two
/// resources — an engine serving a game and a management surface publishes both
/// — and a token issued for one must not reach the other. A bearer token is not
/// bound by cookie host scoping, so this is the only thing that separates them.
fn requested_resource(headers: &HeaderMap, path: &str) -> Option<String> {
    if !path.starts_with("/mcp") {
        return None;
    }

    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .unwrap_or_default();

    Some(format!("{}{}", host, path))
}

/// Extract client IP from headers
fn get_client_ip(headers: &HeaderMap) -> String {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .or_else(|| {
            headers
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "unknown".to_string())
}

/// Extract user agent from headers
fn get_user_agent(headers: &HeaderMap) -> String {
    headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string()
}

/// Create WWW-Authenticate challenge header for MCP endpoints
fn create_auth_challenge(
    realm: &str,
    error: Option<&str>,
    error_description: Option<&str>,
) -> String {
    let mut challenge = format!("Bearer realm=\"{}\"", realm);

    if let Some(err) = error {
        challenge.push_str(&format!(", error=\"{}\"", err));
    }

    if let Some(desc) = error_description {
        challenge.push_str(&format!(", error_description=\"{}\"", desc));
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
            let challenge = create_auth_challenge(
                "MCP API",
                Some("invalid_token"),
                Some("Bearer token required for MCP endpoints"),
            );

            return (
                StatusCode::UNAUTHORIZED,
                [(header::WWW_AUTHENTICATE, challenge)],
                "Unauthorized: Bearer token required",
            )
                .into_response();
        }
    };

    let ip_addr = get_client_ip(headers);
    let user_agent = get_user_agent(headers);
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

            let challenge = create_auth_challenge("MCP API", Some(error), Some(error_desc));

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
        let ip_addr = get_client_ip(headers);
        let user_agent = get_user_agent(headers);
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
