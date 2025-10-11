/// Authentication Middleware
///
/// Axum middleware for extracting and validating authentication from requests,
/// injecting authenticated user context into request extensions.

use crate::auth::{AuthError, AuthManager};
use axum::{
    extract::{Request, State},
    http::{header, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::sync::Arc;

/// Authenticated user context injected into requests
#[derive(Debug, Clone)]
pub struct AuthUser {
    /// Unique user identifier
    pub user_id: String,
    
    /// OAuth2 provider used for authentication
    pub provider: String,
    
    /// Session token
    pub session_token: String,
}

impl AuthUser {
    pub fn new(user_id: String, provider: String, session_token: String) -> Self {
        Self {
            user_id,
            provider,
            session_token,
        }
    }
}

/// Extract session token from request cookies or Authorization header
fn extract_session_token(req: &Request) -> Option<String> {
    // Try Authorization header first (Bearer token)
    if let Some(auth_header) = req.headers().get(header::AUTHORIZATION) {
        if let Ok(auth_str) = auth_header.to_str() {
            if let Some(token) = auth_str.strip_prefix("Bearer ") {
                return Some(token.to_string());
            }
        }
    }
    
    // Try cookie
    if let Some(cookie_header) = req.headers().get(header::COOKIE) {
        if let Ok(cookie_str) = cookie_header.to_str() {
            for cookie in cookie_str.split(';') {
                let cookie = cookie.trim();
                if let Some((name, value)) = cookie.split_once('=') {
                    if name == "auth_session" {
                        return Some(value.to_string());
                    }
                }
            }
        }
    }
    
    None
}

/// Extract client IP address from request
fn extract_client_ip(req: &Request) -> String {
    // Try X-Forwarded-For header first
    if let Some(forwarded) = req.headers().get("x-forwarded-for") {
        if let Ok(forwarded_str) = forwarded.to_str() {
            if let Some(ip) = forwarded_str.split(',').next() {
                return ip.trim().to_string();
            }
        }
    }
    
    // Try X-Real-IP header
    if let Some(real_ip) = req.headers().get("x-real-ip") {
        if let Ok(ip_str) = real_ip.to_str() {
            return ip_str.to_string();
        }
    }
    
    // Fallback to connection info (would need to be passed through state)
    "unknown".to_string()
}

/// Extract user agent from request
fn extract_user_agent(req: &Request) -> String {
    req.headers()
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string()
}

/// Optional authentication middleware - validates session if present but doesn't require it
pub async fn optional_auth_middleware(
    State(auth_manager): State<Arc<AuthManager>>,
    mut req: Request,
    next: Next,
) -> Response {
    if let Some(session_token) = extract_session_token(&req) {
        let ip_addr = extract_client_ip(&req);
        let user_agent = extract_user_agent(&req);
        
        // Validate session
        match auth_manager.validate_session(&session_token, &ip_addr, &user_agent).await {
            Ok(user_id) => {
                // Inject authenticated user into request
                let auth_user = AuthUser::new(
                    user_id,
                    "unknown".to_string(), // Would need to store provider in session
                    session_token,
                );
                req.extensions_mut().insert(auth_user);
            }
            Err(_) => {
                // Invalid session, but we don't fail - just continue without auth
            }
        }
    }
    
    next.run(req).await
}

/// Required authentication middleware - requires valid session or returns 401
pub async fn required_auth_middleware(
    State(auth_manager): State<Arc<AuthManager>>,
    mut req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let session_token = extract_session_token(&req)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    
    let ip_addr = extract_client_ip(&req);
    let user_agent = extract_user_agent(&req);
    
    // Validate session
    let user_id = auth_manager
        .validate_session(&session_token, &ip_addr, &user_agent)
        .await
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    
    // Inject authenticated user into request
    let auth_user = AuthUser::new(
        user_id,
        "unknown".to_string(), // Would need to store provider in session
        session_token,
    );
    req.extensions_mut().insert(auth_user);
    
    Ok(next.run(req).await)
}

/// Error response for authentication failures
#[derive(Debug)]
pub struct AuthErrorResponse {
    pub error: AuthError,
}

impl IntoResponse for AuthErrorResponse {
    fn into_response(self) -> Response {
        let status = StatusCode::from_u16(self.error.status_code())
            .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        
        let body = serde_json::json!({
            "error": self.error.to_string(),
            "status": status.as_u16(),
        });
        
        (status, axum::Json(body)).into_response()
    }
}

impl From<AuthError> for AuthErrorResponse {
    fn from(error: AuthError) -> Self {
        Self { error }
    }
}

/// Extractor for authenticated user from request extensions
/// TODO: Implement proper FromRequestParts extractor
pub struct AuthenticatedUser(pub AuthUser);

// Temporarily commented out until we can resolve the trait implementation
// Will use extensions.get::<AuthUser>() directly in handlers for now
/*
impl<S> axum::extract::FromRequestParts<S> for AuthenticatedUser
where
    S: Send + Sync,
{
    type Rejection = StatusCode;

    fn from_request_parts<'life0, 'life1, 'async_trait>(
        parts: &'life0 mut axum::http::request::Parts,
        _state: &'life1 S,
    ) -> ::core::pin::Pin<Box<dyn ::core::future::Future<Output = Result<Self, Self::Rejection>> + ::core::marker::Send + 'async_trait>>
    where
        'life0: 'async_trait,
        'life1: 'async_trait,
    {
        Box::pin(async move {
            parts
                .extensions
                .get::<AuthUser>()
                .cloned()
                .map(AuthenticatedUser)
                .ok_or(StatusCode::UNAUTHORIZED)
        })
    }
}
*/

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};

    #[test]
    fn test_extract_session_from_bearer() {
        let req = Request::builder()
            .header("Authorization", "Bearer test-token-123")
            .body(Body::empty())
            .unwrap();
        
        let token = extract_session_token(&req);
        assert_eq!(token, Some("test-token-123".to_string()));
    }

    #[test]
    fn test_extract_session_from_cookie() {
        let req = Request::builder()
            .header("Cookie", "auth_session=cookie-token-456; other=value")
            .body(Body::empty())
            .unwrap();
        
        let token = extract_session_token(&req);
        assert_eq!(token, Some("cookie-token-456".to_string()));
    }

    #[test]
    fn test_extract_client_ip_from_forwarded() {
        let req = Request::builder()
            .header("X-Forwarded-For", "192.168.1.1, 10.0.0.1")
            .body(Body::empty())
            .unwrap();
        
        let ip = extract_client_ip(&req);
        assert_eq!(ip, "192.168.1.1");
    }

    #[test]
    fn test_extract_client_ip_from_real_ip() {
        let req = Request::builder()
            .header("X-Real-IP", "192.168.1.100")
            .body(Body::empty())
            .unwrap();
        
        let ip = extract_client_ip(&req);
        assert_eq!(ip, "192.168.1.100");
    }

    #[test]
    fn test_extract_user_agent() {
        let req = Request::builder()
            .header("User-Agent", "Mozilla/5.0 Test Browser")
            .body(Body::empty())
            .unwrap();
        
        let ua = extract_user_agent(&req);
        assert_eq!(ua, "Mozilla/5.0 Test Browser");
    }

    #[test]
    fn test_extract_user_agent_missing() {
        let req = Request::builder()
            .body(Body::empty())
            .unwrap();
        
        let ua = extract_user_agent(&req);
        assert_eq!(ua, "unknown");
    }
}
