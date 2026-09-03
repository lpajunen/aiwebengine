/// Authentication Middleware
///
/// Axum middleware for extracting and validating authentication from requests,
/// injecting authenticated user context into request extensions.
use crate::auth::{AuthError, AuthManager};
use crate::security::client_ip;
use axum::{
    extract::{Request, State},
    http::{HeaderValue, StatusCode, header},
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

    /// Whether user has administrator privileges
    pub is_admin: bool,

    /// Whether user has editor privileges
    pub is_editor: bool,

    /// User's email address (if available)
    pub email: Option<String>,

    /// User's display name (if available)
    pub name: Option<String>,
}

impl AuthUser {
    pub fn new(
        user_id: String,
        provider: String,
        session_token: String,
        is_admin: bool,
        is_editor: bool,
        email: Option<String>,
        name: Option<String>,
    ) -> Self {
        Self {
            user_id,
            provider,
            session_token,
            is_admin,
            is_editor,
            email,
            name,
        }
    }
}

/// Extract session token from request cookies or Authorization header
fn extract_session_token(req: &Request, cookie_name: &str) -> Option<String> {
    // Try Authorization header first (Bearer token)
    if let Some(auth_header) = req.headers().get(header::AUTHORIZATION)
        && let Ok(auth_str) = auth_header.to_str()
        && let Some(token) = auth_str.strip_prefix("Bearer ")
    {
        return Some(token.to_string());
    }

    // Try cookie
    if let Some(cookie_header) = req.headers().get(header::COOKIE)
        && let Ok(cookie_str) = cookie_header.to_str()
    {
        for cookie in cookie_str.split(';') {
            let cookie = cookie.trim();
            if let Some((name, value)) = cookie.split_once('=')
                && name == cookie_name
            {
                return Some(value.to_string());
            }
        }
    }

    None
}

/// The `Host` header a request arrived on, for realm checking.
fn extract_host(req: &Request) -> Option<String> {
    req.headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Re-send the session cookie on the way out, so its `Max-Age` slides forward
/// while someone is using the engine.
///
/// Never over a session cookie the handler already wrote. A handler that set
/// one has decided what the browser should hold — a session it just minted, or
/// an empty value that signs the browser out — and this runs afterwards holding
/// the token the request arrived with, which by then may name a session that no
/// longer exists. Replacing it is how `POST /auth/local/password` came to sign
/// people out of the browser they changed their password in: it ends every
/// session the account had and issues a fresh one, and the fresh cookie was
/// overwritten with the dead token on the way out. Logging out had the same
/// shape — the cookie it cleared was reinstated before the response left.
///
/// Any other cookie on the response is left alone and this one is added beside
/// it, rather than the whole header being replaced: a script route may set
/// cookies of its own, and they are not this layer's to discard.
fn attach_session_cookie(response: &mut Response, auth_manager: &AuthManager, session_token: &str) {
    let config = auth_manager.config();

    let already_written = response
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .any(|value| {
            value
                .split('=')
                .next()
                .map(str::trim)
                .is_some_and(|name| name == config.session_cookie_name)
        });

    if already_written {
        return;
    }

    // Use max_session_age so the browser retains the cookie for the full session
    // lifetime (up to 30 days) rather than expiring it after one sliding hour.
    let cookie_value = format!(
        "{}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{}",
        config.session_cookie_name,
        session_token,
        config.max_session_age,
        if config.cookie_secure { "; Secure" } else { "" }
    );

    if let Ok(cookie_header) = HeaderValue::from_str(&cookie_value) {
        response
            .headers_mut()
            .append(header::SET_COOKIE, cookie_header);
    }
}

/// Optional authentication middleware - validates session if present but doesn't require it
pub async fn optional_auth_middleware(
    State(auth_manager): State<Arc<AuthManager>>,
    mut req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();
    tracing::debug!("🔐 optional_auth_middleware called for path: {}", path);

    let cookie_name = &auth_manager.config().session_cookie_name;

    if let Some(session_token) = extract_session_token(&req, cookie_name) {
        tracing::debug!(
            "🔑 Session token found for {}: {}...",
            path,
            &session_token[..20.min(session_token.len())]
        );
        let ip_addr = client_ip::from_headers(req.headers());
        let user_agent = client_ip::user_agent_from_headers(req.headers());
        let host = extract_host(&req);

        // Get full session information
        match auth_manager
            .get_session(&session_token, &ip_addr, &user_agent, host.as_deref())
            .await
        {
            Ok(session) => {
                tracing::info!(
                    "✅ Session validated for {}: user_id={} is_admin={}",
                    path,
                    session.user_id,
                    session.is_admin
                );
                // Inject authenticated user into request
                let auth_user = AuthUser::new(
                    session.user_id.clone(),
                    session.provider.clone(),
                    session_token.clone(),
                    session.is_admin,
                    session.is_editor,
                    session.email.clone(),
                    session.name.clone(),
                );
                req.extensions_mut().insert(auth_user);
                tracing::debug!("✅ AuthUser injected into request extensions for {}", path);
                let mut response = next.run(req).await;
                attach_session_cookie(&mut response, auth_manager.as_ref(), &session_token);
                return response;
            }
            Err(e) => {
                tracing::warn!("⚠️  Session validation failed for {}: {}", path, e);
                // Invalid session, but we don't fail - just continue without auth
            }
        }
    } else {
        tracing::debug!(
            "ℹ️  No session token found for {} (looking for cookie: {})",
            path,
            cookie_name
        );
    }

    next.run(req).await
}

/// Required authentication middleware - requires valid session or returns 401
pub async fn required_auth_middleware(
    State(auth_manager): State<Arc<AuthManager>>,
    mut req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let cookie_name = &auth_manager.config().session_cookie_name;
    let session_token = extract_session_token(&req, cookie_name).ok_or(StatusCode::UNAUTHORIZED)?;

    let ip_addr = client_ip::from_headers(req.headers());
    let user_agent = client_ip::user_agent_from_headers(req.headers());
    let host = extract_host(&req);

    // Get full session information
    let session = auth_manager
        .get_session(&session_token, &ip_addr, &user_agent, host.as_deref())
        .await
        .map_err(|_| StatusCode::UNAUTHORIZED)?;

    // Inject authenticated user into request
    let auth_user = AuthUser::new(
        session.user_id.clone(),
        session.provider.clone(),
        session_token.clone(),
        session.is_admin,
        session.is_editor,
        session.email.clone(),
        session.name.clone(),
    );
    req.extensions_mut().insert(auth_user);

    let mut response = next.run(req).await;
    attach_session_cookie(&mut response, auth_manager.as_ref(), &session_token);
    Ok(response)
}

/// Redirect to login middleware - redirects to login page if not authenticated
/// This middleware is used for endpoints that require authentication and should
/// redirect to the login page with the original URL preserved for redirect-back.
pub async fn redirect_to_login_middleware(
    State(auth_manager): State<Arc<AuthManager>>,
    mut req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();
    let query = req
        .uri()
        .query()
        .map(|q| format!("?{}", q))
        .unwrap_or_default();
    let cookie_name = &auth_manager.config().session_cookie_name;

    // Check if user is authenticated
    if let Some(session_token) = extract_session_token(&req, cookie_name) {
        let ip_addr = client_ip::from_headers(req.headers());
        let user_agent = client_ip::user_agent_from_headers(req.headers());
        let host = extract_host(&req);

        // Get full session information
        match auth_manager
            .get_session(&session_token, &ip_addr, &user_agent, host.as_deref())
            .await
        {
            Ok(session) => {
                tracing::info!(
                    "✅ Authenticated user accessing {}: user_id={} is_admin={}",
                    path,
                    session.user_id,
                    session.is_admin
                );
                // Inject authenticated user into request
                let auth_user = AuthUser::new(
                    session.user_id.clone(),
                    session.provider.clone(),
                    session_token.clone(),
                    session.is_admin,
                    session.is_editor,
                    session.email.clone(),
                    session.name.clone(),
                );
                req.extensions_mut().insert(auth_user);
                let mut response = next.run(req).await;
                attach_session_cookie(&mut response, auth_manager.as_ref(), &session_token);
                return response;
            }
            Err(e) => {
                tracing::warn!("⚠️  Session validation failed for {}: {}", path, e);
                // Invalid session, redirect to login
            }
        }
    }

    // User is not authenticated, redirect to login with return URL
    let full_path = format!("{}{}", path, query);
    let return_url = urlencoding::encode(&full_path);
    let login_url = format!("/auth/login?redirect={}", return_url);

    tracing::info!(
        "🔒 Redirecting unauthenticated user from {} to {}",
        full_path,
        login_url
    );

    axum::response::Redirect::to(&login_url).into_response()
}

/// Require editor or admin middleware - requires valid session with editor or admin role
/// This middleware is used for endpoints that require either Editor or Administrator privileges.
/// If the user is authenticated but doesn't have the required role, they are redirected to
/// an insufficient permissions page. If they are not authenticated, they are redirected to login.
pub async fn require_editor_or_admin_middleware(
    State(auth_manager): State<Arc<AuthManager>>,
    mut req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path().to_string();
    let query = req
        .uri()
        .query()
        .map(|q| format!("?{}", q))
        .unwrap_or_default();
    let cookie_name = &auth_manager.config().session_cookie_name;

    // Check if user is authenticated
    if let Some(session_token) = extract_session_token(&req, cookie_name) {
        let ip_addr = client_ip::from_headers(req.headers());
        let user_agent = client_ip::user_agent_from_headers(req.headers());
        let host = extract_host(&req);

        // Get full session information
        match auth_manager
            .get_session(&session_token, &ip_addr, &user_agent, host.as_deref())
            .await
        {
            Ok(session) => {
                // Check if user has editor or admin privileges
                if session.is_editor || session.is_admin {
                    tracing::info!(
                        "✅ Authorized user accessing {}: user_id={} is_admin={} is_editor={}",
                        path,
                        session.user_id,
                        session.is_admin,
                        session.is_editor
                    );
                    // Inject authenticated user into request
                    let auth_user = AuthUser::new(
                        session.user_id.clone(),
                        session.provider.clone(),
                        session_token.clone(),
                        session.is_admin,
                        session.is_editor,
                        session.email.clone(),
                        session.name.clone(),
                    );
                    req.extensions_mut().insert(auth_user);
                    let mut response = next.run(req).await;
                    attach_session_cookie(&mut response, auth_manager.as_ref(), &session_token);
                    return response;
                } else {
                    // User is authenticated but doesn't have required role
                    tracing::warn!(
                        "⚠️  Insufficient permissions for {}: user_id={} is_admin={} is_editor={}",
                        path,
                        session.user_id,
                        session.is_admin,
                        session.is_editor
                    );

                    let full_path = format!("{}{}", path, query);
                    let return_url = urlencoding::encode(&full_path);
                    let auth_url = format!("/auth/unauthorized?attempted={}", return_url);

                    tracing::info!(
                        "🚫 Redirecting user without required role from {} to {}",
                        full_path,
                        auth_url
                    );

                    return axum::response::Redirect::to(&auth_url).into_response();
                }
            }
            Err(e) => {
                tracing::warn!("⚠️  Session validation failed for {}: {}", path, e);
                // Invalid session, redirect to login
            }
        }
    }

    // User is not authenticated, redirect to login with return URL
    let full_path = format!("{}{}", path, query);
    let return_url = urlencoding::encode(&full_path);
    let login_url = format!("/auth/login?redirect={}", return_url);

    tracing::info!(
        "🔒 Redirecting unauthenticated user from {} to {}",
        full_path,
        login_url
    );

    axum::response::Redirect::to(&login_url).into_response()
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
    use axum::http::Request;

    #[test]
    fn test_extract_session_from_bearer() {
        let req = Request::builder()
            .header("Authorization", "Bearer test-token-123")
            .body(Body::empty())
            .unwrap();

        let token = extract_session_token(&req, "auth_session");
        assert_eq!(token, Some("test-token-123".to_string()));
    }

    #[test]
    fn test_extract_session_from_cookie() {
        let req = Request::builder()
            .header("Cookie", "auth_session=cookie-token-456; other=value")
            .body(Body::empty())
            .unwrap();

        let token = extract_session_token(&req, "auth_session");
        assert_eq!(token, Some("cookie-token-456".to_string()));
    }

    /// By the time a middleware runs, the edge has already decided which
    /// address to believe and left exactly that one behind. Reading a chain
    /// here — which is what this used to do — is reading the claim again.
    #[test]
    fn the_client_ip_is_whatever_the_edge_established() {
        let req = Request::builder()
            .header("X-Forwarded-For", "192.168.1.1")
            .body(Body::empty())
            .unwrap();

        assert_eq!(client_ip::from_headers(req.headers()), "192.168.1.1");

        let req = Request::builder().body(Body::empty()).unwrap();
        assert_eq!(client_ip::from_headers(req.headers()), "unknown");
    }

    #[test]
    fn test_extract_user_agent() {
        let req = Request::builder()
            .header("User-Agent", "Mozilla/5.0 Test Browser")
            .body(Body::empty())
            .unwrap();

        let ua = client_ip::user_agent_from_headers(req.headers());
        assert_eq!(ua, "Mozilla/5.0 Test Browser");
    }

    #[test]
    fn test_extract_user_agent_missing() {
        let req = Request::builder().body(Body::empty()).unwrap();

        let ua = client_ip::user_agent_from_headers(req.headers());
        assert_eq!(ua, "unknown");
    }
}
