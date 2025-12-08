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
use chrono;
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

    // Try to extract token from Bearer header first
    let mut token = extract_bearer_token(headers);

    // If not found, try query parameter (api_key or token)
    if token.is_none() {
        if let Some(query) = request.uri().query() {
            let params: std::collections::HashMap<String, String> =
                url::form_urlencoded::parse(query.as_bytes())
                    .into_owned()
                    .collect();

            token = params
                .get("api_key")
                .or_else(|| params.get("token"))
                .cloned();
        }
    }

    let token = match token {
        Some(t) => t,
        None => {
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
    let path = request.uri().path().to_string();

    // Determine resource indicator from request path
    let resource = if path.starts_with("/mcp") {
        Some(path.as_str())
    } else {
        None
    };

    // Check if token is a valid API key
    if auth_manager.validate_api_key(&token) {
        // Create a synthetic session for API key access
        // API keys have admin privileges for MCP
        let session = AuthSession {
            user_id: "system-api".to_string(),
            provider: "api_key".to_string(),
            email: Some("system@aiwebengine.com".to_string()),
            name: Some("System API".to_string()),
            picture: None,
            is_admin: true,
            is_editor: true,
            created_at: chrono::Utc::now(),
            expires_at: chrono::Utc::now() + chrono::Duration::hours(1),
        };

        // Store session in request extensions
        request.extensions_mut().insert(McpAuthSession { session });

        return next.run(request).await;
    }

    // Validate session with resource indicator
    let session = match auth_manager
        .validate_session_with_resource(&token, &ip_addr, &user_agent, resource)
        .await
    {
        Ok(s) => s,
        Err(e) => {
            // Log the specific AuthError reason at debug level for diagnostics
            tracing::debug!("MCP session validation error for token (redacted): {:?}", e);
            // Session validation failed - return 401 with appropriate error
            let (error, error_desc) = match e {
                AuthError::NoSession | AuthError::SessionError(_) => {
                    ("invalid_token", "Session not found or expired")
                }
                AuthError::TokenExpired => ("invalid_token", "Session expired"),
                AuthError::InsufficientPermissions => (
                    "insufficient_scope",
                    "Insufficient permissions for this resource",
                ),
                _ => ("invalid_token", "Session validation failed"),
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
        let path = request.uri().path().to_string();
        let resource = if path.starts_with("/mcp") {
            Some(path.as_str())
        } else {
            None
        };

        if let Ok(session) = auth_manager
            .validate_session_with_resource(&token, &ip_addr, &user_agent, resource)
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
