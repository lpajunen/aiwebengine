/// Authentication Routes
///
/// HTTP route handlers for OAuth2 authentication flow including
/// login initiation, callback processing, and logout.

use crate::auth::AuthManager;
use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// OAuth2 callback parameters
#[derive(Debug, Deserialize)]
pub struct OAuthCallbackParams {
    /// Authorization code from provider
    code: Option<String>,
    
    /// CSRF state token
    state: Option<String>,
    
    /// Error from provider
    error: Option<String>,
    
    /// Error description from provider
    error_description: Option<String>,
}

/// Login initiation parameters
#[derive(Debug, Deserialize)]
pub struct LoginParams {
    /// OAuth2 provider name
    provider: String,
    
    /// Optional redirect URL after successful login
    redirect: Option<String>,
}

/// Logout parameters
#[derive(Debug, Deserialize)]
pub struct LogoutParams {
    /// Optional redirect URL after logout
    redirect: Option<String>,
}

/// JSON response for successful authentication
#[derive(Debug, Serialize)]
pub struct AuthResponse {
    pub success: bool,
    pub session_token: Option<String>,
    pub user_id: Option<String>,
    pub redirect: Option<String>,
}

/// JSON error response
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub message: String,
}

impl IntoResponse for ErrorResponse {
    fn into_response(self) -> Response {
        (StatusCode::BAD_REQUEST, Json(self)).into_response()
    }
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

/// Login page handler - displays available providers
async fn login_page(
    State(auth_manager): State<Arc<AuthManager>>,
) -> Html<String> {
    let providers = auth_manager.list_providers();
    
    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Login</title>
    <style>
        body {{
            font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
            max-width: 600px;
            margin: 50px auto;
            padding: 20px;
            background: #f5f5f5;
        }}
        .login-container {{
            background: white;
            padding: 40px;
            border-radius: 8px;
            box-shadow: 0 2px 4px rgba(0,0,0,0.1);
        }}
        h1 {{
            margin-top: 0;
            color: #333;
        }}
        .provider-button {{
            display: block;
            width: 100%;
            padding: 15px;
            margin: 10px 0;
            border: none;
            border-radius: 4px;
            font-size: 16px;
            cursor: pointer;
            text-decoration: none;
            text-align: center;
            color: white;
            transition: opacity 0.2s;
        }}
        .provider-button:hover {{
            opacity: 0.9;
        }}
        .google {{ background: #4285f4; }}
        .microsoft {{ background: #00a4ef; }}
        .apple {{ background: #000; }}
    </style>
</head>
<body>
    <div class="login-container">
        <h1>Sign In</h1>
        <p>Choose a provider to continue:</p>
        {}
    </div>
</body>
</html>"#,
        providers
            .iter()
            .map(|p| format!(
                r#"<a href="/auth/login/{}?redirect=/" class="provider-button {}">{}</a>"#,
                p.to_lowercase(),
                p.to_lowercase(),
                match p.as_str() {
                    "google" => "Sign in with Google",
                    "microsoft" => "Sign in with Microsoft",
                    "apple" => "Sign in with Apple",
                    _ => "Sign in",
                }
            ))
            .collect::<Vec<_>>()
            .join("\n        ")
    );
    
    Html(html)
}

/// Start OAuth2 login flow - redirects to provider
async fn start_login(
    State(auth_manager): State<Arc<AuthManager>>,
    Query(params): Query<LoginParams>,
    headers: HeaderMap,
) -> Result<Redirect, ErrorResponse> {
    let ip_addr = get_client_ip(&headers);
    
    // Generate authorization URL
    let (auth_url, _state) = auth_manager
        .start_login(&params.provider, &ip_addr)
        .await
        .map_err(|e| ErrorResponse {
            error: "login_failed".to_string(),
            message: e.to_string(),
        })?;
    
    // Redirect to provider
    Ok(Redirect::temporary(&auth_url))
}

/// Handle OAuth2 callback from provider
async fn oauth_callback(
    State(auth_manager): State<Arc<AuthManager>>,
    Query(params): Query<OAuthCallbackParams>,
    headers: HeaderMap,
) -> Result<Response, ErrorResponse> {
    // Check for provider error
    if let Some(error) = params.error {
        let message = params.error_description.unwrap_or_else(|| "Unknown error".to_string());
        return Err(ErrorResponse {
            error,
            message,
        });
    }
    
    // Get code and state
    let code = params.code.ok_or_else(|| ErrorResponse {
        error: "missing_code".to_string(),
        message: "Authorization code missing from callback".to_string(),
    })?;
    
    let state = params.state.ok_or_else(|| ErrorResponse {
        error: "missing_state".to_string(),
        message: "State parameter missing from callback".to_string(),
    })?;
    
    let ip_addr = get_client_ip(&headers);
    let user_agent = get_user_agent(&headers);
    
    // Extract provider from state (state format: "provider:random")
    let provider = state.split(':').next().unwrap_or("unknown");
    
    // Handle callback
    let session_token = auth_manager
        .handle_callback(provider, &code, &state, &ip_addr, &user_agent)
        .await
        .map_err(|e| ErrorResponse {
            error: "authentication_failed".to_string(),
            message: e.to_string(),
        })?;
    
    // Set session cookie
    let config = auth_manager.config();
    let cookie_value = format!(
        "{}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{}",
        config.session_cookie_name,
        session_token,
        config.session_timeout,
        if config.cookie_secure { "; Secure" } else { "" }
    );
    
    // Return redirect with cookie
    let response = Redirect::to("/").into_response();
    let (mut parts, body) = response.into_parts();
    parts.headers.insert(
        header::SET_COOKIE,
        cookie_value.parse().unwrap(),
    );
    
    Ok(Response::from_parts(parts, body))
}

/// Logout handler - destroys session
async fn logout(
    State(auth_manager): State<Arc<AuthManager>>,
    Query(params): Query<LogoutParams>,
    headers: HeaderMap,
) -> Result<Response, ErrorResponse> {
    // Extract session token from cookie
    let session_token = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies
                .split(';')
                .find_map(|cookie| {
                    let cookie = cookie.trim();
                    cookie.strip_prefix("auth_session=")
                })
        });
    
    if let Some(token) = session_token {
        // Destroy session
        let _ = auth_manager.logout(token, false).await;
    }
    
    // Clear cookie
    let config = auth_manager.config();
    let cookie_value = format!(
        "{}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0",
        config.session_cookie_name
    );
    
    // Redirect to specified location or home
    let redirect_url = params.redirect.as_deref().unwrap_or("/");
    let response = Redirect::to(redirect_url).into_response();
    let (mut parts, body) = response.into_parts();
    parts.headers.insert(
        header::SET_COOKIE,
        cookie_value.parse().unwrap(),
    );
    
    Ok(Response::from_parts(parts, body))
}

/// Status endpoint - check authentication status
async fn auth_status(
    State(auth_manager): State<Arc<AuthManager>>,
    headers: HeaderMap,
) -> Json<AuthResponse> {
    let ip_addr = get_client_ip(&headers);
    let user_agent = get_user_agent(&headers);
    
    // Extract session token
    let session_token = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies
                .split(';')
                .find_map(|cookie| {
                    let cookie = cookie.trim();
                    cookie.strip_prefix("auth_session=")
                })
        });
    
    if let Some(token) = session_token {
        if let Ok(user_id) = auth_manager.validate_session(token, &ip_addr, &user_agent).await {
            return Json(AuthResponse {
                success: true,
                session_token: Some(token.to_string()),
                user_id: Some(user_id),
                redirect: None,
            });
        }
    }
    
    Json(AuthResponse {
        success: false,
        session_token: None,
        user_id: None,
        redirect: Some("/auth/login".to_string()),
    })
}

/// Create authentication router with all routes
pub fn create_auth_router(auth_manager: Arc<AuthManager>) -> Router {
    Router::new()
        .route("/login", get(login_page))
        .route("/login/:provider", get(start_login))
        .route("/callback", get(oauth_callback))
        .route("/logout", post(logout))
        .route("/status", get(auth_status))
        .with_state(auth_manager)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn test_get_client_ip_from_forwarded() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", HeaderValue::from_static("192.168.1.1, 10.0.0.1"));
        
        let ip = get_client_ip(&headers);
        assert_eq!(ip, "192.168.1.1");
    }

    #[test]
    fn test_get_client_ip_from_real_ip() {
        let mut headers = HeaderMap::new();
        headers.insert("x-real-ip", HeaderValue::from_static("192.168.1.100"));
        
        let ip = get_client_ip(&headers);
        assert_eq!(ip, "192.168.1.100");
    }

    #[test]
    fn test_get_user_agent() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::USER_AGENT,
            HeaderValue::from_static("Mozilla/5.0 Test")
        );
        
        let ua = get_user_agent(&headers);
        assert_eq!(ua, "Mozilla/5.0 Test");
    }
}
