use crate::auth::client_registration::{ClientRegistrationManager, register_client_handler};
use crate::auth::metadata::{
    MetadataConfig, metadata_handler, protected_resource_metadata_handler,
};
/// Authentication Routes
///
/// HTTP route handlers for OAuth2 authentication flow including
/// login initiation, callback processing, and logout.
use crate::auth::{AuthManager, AuthSecurityContext};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

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
    /// Optional redirect URL after successful login
    #[allow(dead_code)]
    redirect: Option<String>,
}

/// Logout parameters
#[derive(Debug, Deserialize)]
pub struct LogoutParams {
    /// Optional redirect URL after logout
    redirect: Option<String>,
}

use sqlx::PgPool;

/// Authorization code data stored temporarily
#[derive(Debug, Clone, sqlx::FromRow)]
struct AuthorizationCodeData {
    user_id: String,
    #[allow(dead_code)] // Stored for future validation
    client_id: String,
    redirect_uri: String,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
    scope: Option<String>,
    resource: Option<String>,
    expires_at: DateTime<Utc>,
    used: bool,
}

/// OAuth2 shared state for protocol endpoints
#[derive(Clone)]
pub struct OAuth2State {
    auth_manager: Arc<AuthManager>,
    pool: PgPool,
}

impl OAuth2State {
    pub fn new(auth_manager: Arc<AuthManager>, pool: PgPool) -> Self {
        Self { auth_manager, pool }
    }
}

/// JSON response for successful authentication
#[derive(Debug, Serialize)]
pub struct AuthResponse {
    pub success: bool,
    pub session_token: Option<String>,
    pub user_id: Option<String>,
    pub is_admin: Option<bool>,
    pub is_editor: Option<bool>,
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

/// Login page parameters
#[derive(Debug, Deserialize)]
pub struct LoginPageParams {
    /// Optional redirect URL after successful login
    redirect: Option<String>,
}

/// Login page handler - displays available providers
async fn login_page(
    State(auth_manager): State<Arc<AuthManager>>,
    Query(params): Query<LoginPageParams>,
) -> Html<String> {
    let providers = auth_manager.list_providers();
    let redirect_param = params.redirect.unwrap_or_else(|| "/".to_string());
    let encoded_redirect = urlencoding::encode(&redirect_param);

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Login</title>
    <link rel="stylesheet" href="/engine.css">
    <link rel="icon" type="image/x-icon" href="/favicon.ico">
</head>
<body>
    <div class="page-container">
        <main class="page-main">
            <div class="container">
                <div class="row justify-content-center">
                    <div class="col-12 col-md-6 col-lg-4">
                        <div class="card">
                            <div class="card-body text-center">
                                <h1 class="mb-3">Sign In</h1>
                                <p class="text-muted mb-4">Choose a provider to continue:</p>
                                {}
                            </div>
                        </div>
                    </div>
                </div>
            </div>
        </main>
    </div>
</body>
</html>"#,
        {
            let mut sorted_providers = providers.clone();
            sorted_providers.sort();
            sorted_providers
                .iter()
                .map(|p| format!(
                    r#"<a href="/auth/login/{}?redirect={}" class="btn btn-block provider-btn provider-{} mb-2">{}</a>"#,
                    p.to_lowercase(),
                    encoded_redirect,
                    p.to_lowercase(),
                    match p.as_str() {
                        "google" => "Sign in with Google",
                        "microsoft" => "Sign in with Microsoft",
                        "apple" => "Sign in with Apple",
                        _ => "Sign in",
                    }
                ))
                .collect::<Vec<_>>()
                .join("\n                                ")
        }
    );

    Html(html)
}

/// Start OAuth2 login flow - redirects to provider
async fn start_login(
    State(auth_manager): State<Arc<AuthManager>>,
    Path(provider): Path<String>,
    Query(params): Query<LoginParams>,
    headers: HeaderMap,
) -> Result<Redirect, ErrorResponse> {
    let ip_addr = get_client_ip(&headers);

    // Generate authorization URL with or without redirect
    let (auth_url, _state) = if let Some(ref redirect_url) = params.redirect {
        tracing::info!("Starting login with redirect URL: {}", redirect_url);
        auth_manager
            .start_login_with_redirect(&provider, &ip_addr, redirect_url.clone())
            .await
            .map_err(|e| ErrorResponse {
                error: "login_failed".to_string(),
                message: e.to_string(),
            })?
    } else {
        tracing::info!("Starting login without redirect URL");
        auth_manager
            .start_login(&provider, &ip_addr)
            .await
            .map_err(|e| ErrorResponse {
                error: "login_failed".to_string(),
                message: e.to_string(),
            })?
    };

    // Redirect to provider
    Ok(Redirect::temporary(&auth_url))
}

/// Handle OAuth2 callback from provider
async fn oauth_callback(
    State(auth_manager): State<Arc<AuthManager>>,
    Path(provider): Path<String>,
    Query(params): Query<OAuthCallbackParams>,
    headers: HeaderMap,
) -> Result<Response, ErrorResponse> {
    // Check for provider error
    if let Some(error) = params.error {
        let message = params
            .error_description
            .unwrap_or_else(|| "Unknown error".to_string());
        return Err(ErrorResponse { error, message });
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

    // Provider comes from the URL path parameter
    // Extract redirect URL from state (stateless approach)
    let redirect_url = AuthSecurityContext::extract_redirect_url(&state);

    // Log the redirect URL for debugging
    if let Some(ref url) = redirect_url {
        tracing::info!("OAuth callback redirect URL extracted from state: {}", url);
    } else {
        tracing::warn!("No redirect URL found in OAuth state, will redirect to /");
    }

    // Handle callback
    let session_token = auth_manager
        .handle_callback(&provider, &code, &state, &ip_addr, &user_agent)
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

    // Redirect to stored URL or default to home
    let redirect_target = redirect_url.as_deref().unwrap_or("/");

    // Return redirect with cookie
    let response = Redirect::to(redirect_target).into_response();
    let (mut parts, body) = response.into_parts();
    parts
        .headers
        .insert(header::SET_COOKIE, cookie_value.parse().unwrap());

    Ok(Response::from_parts(parts, body))
}

/// Logout handler - destroys session
async fn logout(
    State(auth_manager): State<Arc<AuthManager>>,
    Query(params): Query<LogoutParams>,
    headers: HeaderMap,
) -> Result<Response, ErrorResponse> {
    let config = auth_manager.config();

    // Extract session token from cookie
    let session_token = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').find_map(|cookie| {
                let trimmed = cookie.trim();
                let (name, value) = trimmed.split_once('=')?;
                if name == config.session_cookie_name {
                    Some(value.to_string())
                } else {
                    None
                }
            })
        });

    if let Some(token) = session_token {
        // Destroy session
        if let Err(e) = auth_manager.logout(&token, false).await {
            tracing::error!("Failed to logout session: {}", e);
            // Continue anyway to clear the cookie
        } else {
            tracing::info!("Session successfully invalidated during logout");
        }
    } else {
        tracing::warn!("Logout called but no session token found in cookies");
    }

    // Clear cookie
    let cookie_value = format!(
        "{}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0",
        config.session_cookie_name
    );

    // Redirect to specified location or home
    let redirect_url = params.redirect.as_deref().unwrap_or("/");
    let response = Redirect::to(redirect_url).into_response();
    let (mut parts, body) = response.into_parts();
    parts
        .headers
        .insert(header::SET_COOKIE, cookie_value.parse().unwrap());

    Ok(Response::from_parts(parts, body))
}

/// Status endpoint - check authentication status
async fn auth_status(
    State(auth_manager): State<Arc<AuthManager>>,
    headers: HeaderMap,
) -> Json<AuthResponse> {
    let ip_addr = get_client_ip(&headers);
    let user_agent = get_user_agent(&headers);

    let config = auth_manager.config();

    // Extract session token
    let session_token = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').find_map(|cookie| {
                let trimmed = cookie.trim();
                let (name, value) = trimmed.split_once('=')?;
                if name == config.session_cookie_name {
                    Some(value.to_string())
                } else {
                    None
                }
            })
        });
    if let Some(token) = session_token
        && let Ok(session) = auth_manager
            .get_session(&token, &ip_addr, &user_agent)
            .await
    {
        return Json(AuthResponse {
            success: true,
            session_token: Some(token.to_string()),
            user_id: Some(session.user_id),
            is_admin: Some(session.is_admin),
            is_editor: Some(session.is_editor),
            redirect: None,
        });
    }

    Json(AuthResponse {
        success: false,
        session_token: None,
        user_id: None,
        is_admin: None,
        is_editor: None,
        redirect: Some("/auth/login".to_string()),
    })
}

/// OAuth 2.0 authorization request parameters (RFC 6749)
#[derive(Debug, Deserialize)]
pub struct AuthorizeParams {
    /// Client identifier
    response_type: String,

    /// Client identifier
    client_id: String,

    /// Redirection URI
    #[serde(default)]
    redirect_uri: Option<String>,

    /// Requested scope
    #[serde(default)]
    scope: Option<String>,

    /// Opaque value for CSRF protection
    #[serde(default)]
    state: Option<String>,

    /// PKCE code challenge (RFC 7636)
    #[serde(default)]
    code_challenge: Option<String>,

    /// PKCE code challenge method (S256 or plain)
    #[serde(default)]
    code_challenge_method: Option<String>,

    /// Resource indicator (RFC 8707)
    #[serde(default)]
    resource: Option<String>,
}

/// OAuth 2.0 authorization endpoint
/// This endpoint handles authorization requests from OAuth clients
async fn oauth2_authorize(
    State(oauth2_state): State<OAuth2State>,
    Query(params): Query<AuthorizeParams>,
    req: axum::extract::Request,
) -> Response {
    // Check if user is already authenticated via middleware
    let auth_user = req.extensions().get::<crate::auth::AuthUser>().cloned();
    let is_authenticated = auth_user.is_some();
    // Validate response_type
    if params.response_type != "code" {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "unsupported_response_type".to_string(),
                message: "Only 'code' response type is supported".to_string(),
            }),
        )
            .into_response();
    }

    // Validate client_id (basic validation - in production, check against registered clients)
    if params.client_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "invalid_request".to_string(),
                message: "Missing client_id parameter".to_string(),
            }),
        )
            .into_response();
    }

    // If not authenticated, redirect to login with return URL
    if !is_authenticated {
        // Build query string with only non-empty parameters
        let mut query_params = vec![
            format!(
                "response_type={}",
                urlencoding::encode(&params.response_type)
            ),
            format!("client_id={}", urlencoding::encode(&params.client_id)),
        ];

        if let Some(ref uri) = params.redirect_uri
            && !uri.is_empty()
        {
            query_params.push(format!("redirect_uri={}", urlencoding::encode(uri)));
        }
        if let Some(ref scope) = params.scope
            && !scope.is_empty()
        {
            query_params.push(format!("scope={}", urlencoding::encode(scope)));
        }
        if let Some(ref state) = params.state
            && !state.is_empty()
        {
            query_params.push(format!("state={}", urlencoding::encode(state)));
        }
        if let Some(ref challenge) = params.code_challenge
            && !challenge.is_empty()
        {
            query_params.push(format!("code_challenge={}", urlencoding::encode(challenge)));
        }
        if let Some(ref method) = params.code_challenge_method
            && !method.is_empty()
        {
            query_params.push(format!(
                "code_challenge_method={}",
                urlencoding::encode(method)
            ));
        }
        if let Some(ref resource) = params.resource
            && !resource.is_empty()
        {
            query_params.push(format!("resource={}", urlencoding::encode(resource)));
        }

        let query_string = if query_params.is_empty() {
            String::new()
        } else {
            format!("?{}", query_params.join("&"))
        };

        let return_url = format!("/authorize{}", query_string);
        let encoded_return = urlencoding::encode(&return_url);

        return Redirect::to(&format!("/auth/login?redirect={}", encoded_return)).into_response();
    }

    // TODO: In a complete implementation:
    // 1. Validate the client_id against registered clients
    // 2. Validate redirect_uri matches registered URIs
    // 3. Show consent screen if needed

    // For MCP: Generate authorization code and redirect back to client
    // Since we don't have client validation yet, we'll accept any client_id for development

    // Get authenticated user ID
    let user_id = auth_user
        .as_ref()
        .map(|u| u.user_id.clone())
        .expect("User must be authenticated at this point");

    // Generate a random authorization code
    let auth_code = format!("code_{}", uuid::Uuid::new_v4());

    // Store the authorization code with associated data
    let expires_at = Utc::now() + chrono::Duration::minutes(10); // 10 minute expiry

    let result = sqlx::query(
        "INSERT INTO oauth_authorization_codes (code, user_id, client_id, redirect_uri, code_challenge, code_challenge_method, scope, resource, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"
    )
    .bind(&auth_code)
    .bind(&user_id)
    .bind(&params.client_id)
    .bind(params.redirect_uri.clone().unwrap_or_default())
    .bind(&params.code_challenge)
    .bind(&params.code_challenge_method)
    .bind(&params.scope)
    .bind(&params.resource)
    .bind(expires_at)
    .execute(&oauth2_state.pool)
    .await;

    if let Err(e) = result {
        tracing::error!("Failed to store authorization code: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "server_error".to_string(),
                message: "Failed to generate authorization code".to_string(),
            }),
        )
            .into_response();
    }

    tracing::info!("Stored authorization code for user: {}", user_id);

    // Build redirect URI with code
    let redirect_uri = match params.redirect_uri.as_ref() {
        Some(uri) => {
            tracing::info!("Authorization complete, redirect_uri from client: {}", uri);
            uri
        }
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "invalid_request".to_string(),
                    message: "redirect_uri is required".to_string(),
                }),
            )
                .into_response();
        }
    };

    // Build the redirect URL with code and state parameters
    // Handle both http(s) URLs and custom schemes like vscode://
    let redirect_with_code = if redirect_uri.contains('?') {
        // URL already has query parameters, append with &
        format!("{}&code={}", redirect_uri, urlencoding::encode(&auth_code))
    } else {
        // No existing query parameters, start with ?
        format!("{}?code={}", redirect_uri, urlencoding::encode(&auth_code))
    };

    let final_redirect = if let Some(ref state) = params.state {
        format!(
            "{}&state={}",
            redirect_with_code,
            urlencoding::encode(state)
        )
    } else {
        redirect_with_code
    };

    tracing::info!(
        "Redirecting to client with authorization code: {}",
        final_redirect
    );

    // Return HTML with meta refresh for custom schemes like vscode://
    // because Redirect::to() doesn't work well with custom URI schemes
    // Use serde_json to safely encode the URL for JavaScript
    let js_redirect = serde_json::to_string(&final_redirect)
        .unwrap_or_else(|_| format!("\"{}\"", final_redirect));

    let html = format!(
        r#"<!DOCTYPE html>
<html>
<head>
    <meta http-equiv="refresh" content="0;url={}" />
    <title>Redirecting...</title>
</head>
<body>
    <p>Redirecting to application... If you are not redirected, <a href="{}">click here</a>.</p>
    <script>window.location.href = {};</script>
</body>
</html>"#,
        html_escape::encode_text(&final_redirect),
        html_escape::encode_text(&final_redirect),
        js_redirect
    );

    (StatusCode::OK, Html(html)).into_response()
}

/// OAuth 2.0 token request parameters (RFC 6749)
#[derive(Debug, Deserialize)]
pub struct TokenParams {
    /// Grant type
    grant_type: String,

    /// Authorization code (for authorization_code grant)
    #[serde(default)]
    code: Option<String>,

    /// Redirect URI (for authorization_code grant)
    #[serde(default)]
    redirect_uri: Option<String>,

    /// PKCE code verifier (RFC 7636)
    #[serde(default)]
    code_verifier: Option<String>,

    /// Refresh token (for refresh_token grant)
    #[serde(default)]
    #[allow(dead_code)] // Used by serde for form deserialization
    refresh_token: Option<String>,

    /// Client identifier
    #[serde(default)]
    client_id: Option<String>,
}

/// OAuth 2.0 token response
#[derive(Debug, Serialize)]
pub struct TokenResponse {
    access_token: String,
    token_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_in: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    refresh_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    scope: Option<String>,
}

/// OAuth 2.0 token endpoint
/// This endpoint issues access tokens in exchange for authorization codes
async fn oauth2_token(
    State(oauth2_state): State<OAuth2State>,
    headers: HeaderMap,
    axum::Form(params): axum::Form<TokenParams>,
) -> Response {
    tracing::info!("📩 Token exchange request received");
    tracing::info!("  grant_type: {}", params.grant_type);
    tracing::info!("  code: {:?}", params.code);
    tracing::info!("  client_id: {:?}", params.client_id);
    tracing::info!("  redirect_uri: {:?}", params.redirect_uri);

    // Validate grant_type
    if params.grant_type != "authorization_code" {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "unsupported_grant_type".to_string(),
                message: "Only authorization_code grant type is supported".to_string(),
            }),
        )
            .into_response();
    }

    // Validate required parameters
    let code = match params.code {
        Some(ref c) if c.starts_with("code_") => c,
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "invalid_request".to_string(),
                    message: "Missing or invalid code parameter".to_string(),
                }),
            )
                .into_response();
        }
    };

    tracing::info!("Exchanging code: {}", code);

    // Retrieve and validate the authorization code
    let mut tx = match oauth2_state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => {
            tracing::error!("Database error: {}", e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "server_error".to_string(),
                    message: "Database error".to_string(),
                }),
            )
                .into_response();
        }
    };

    let code_data_opt: Option<AuthorizationCodeData> =
        sqlx::query_as("SELECT * FROM oauth_authorization_codes WHERE code = $1 FOR UPDATE")
            .bind(code)
            .fetch_optional(&mut *tx)
            .await
            .unwrap_or(None);

    let code_data = match code_data_opt {
        Some(data) if !data.used && data.expires_at > Utc::now() => {
            // Mark code as used
            let _ = sqlx::query("UPDATE oauth_authorization_codes SET used = TRUE WHERE code = $1")
                .bind(code)
                .execute(&mut *tx)
                .await;
            data
        }
        Some(data) if data.used => {
            let _ = tx.rollback().await;
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "invalid_grant".to_string(),
                    message: "Authorization code has already been used".to_string(),
                }),
            )
                .into_response();
        }
        Some(_) => {
            let _ = tx.rollback().await;
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "invalid_grant".to_string(),
                    message: "Authorization code has expired".to_string(),
                }),
            )
                .into_response();
        }
        None => {
            let _ = tx.rollback().await;
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "invalid_grant".to_string(),
                    message: "Invalid authorization code".to_string(),
                }),
            )
                .into_response();
        }
    };

    if let Err(e) = tx.commit().await {
        tracing::error!("Database commit error: {}", e);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "server_error".to_string(),
                message: "Database error".to_string(),
            }),
        )
            .into_response();
    }

    // Verify redirect_uri matches
    if let Some(ref redirect_uri) = params.redirect_uri
        && redirect_uri != &code_data.redirect_uri
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "invalid_grant".to_string(),
                message: "redirect_uri does not match authorization request".to_string(),
            }),
        )
            .into_response();
    }

    // Verify PKCE code_verifier if code_challenge was provided
    if let Some(ref challenge) = code_data.code_challenge {
        match params.code_verifier.as_ref() {
            Some(verifier) => {
                // Verify the code_verifier against code_challenge
                let computed_challenge =
                    if code_data.code_challenge_method.as_deref() == Some("S256") {
                        use base64::Engine;
                        use sha2::{Digest, Sha256};
                        let hash = Sha256::digest(verifier.as_bytes());
                        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash)
                    } else {
                        // plain method
                        verifier.clone()
                    };

                if &computed_challenge != challenge {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse {
                            error: "invalid_grant".to_string(),
                            message: "PKCE verification failed".to_string(),
                        }),
                    )
                        .into_response();
                }
            }
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: "invalid_request".to_string(),
                        message: "code_verifier required for PKCE".to_string(),
                    }),
                )
                    .into_response();
            }
        }
    }

    // Create a session for the user
    let ip_addr = extract_client_ip_from_headers(&headers);
    let user_agent = extract_user_agent_from_headers(&headers);

    // Get user info from the user repository to get email, name, is_admin, is_editor
    // For now, we'll use defaults since we don't have direct access to user repo here
    // In production, pass user repository or fetch this info during code storage
    let session_params = crate::auth::session::CreateAuthSessionParams {
        user_id: code_data.user_id.clone(),
        provider: "oauth2".to_string(),
        email: None,      // TODO: Store with code or fetch from user repo
        name: None,       // TODO: Store with code or fetch from user repo
        is_admin: false,  // TODO: Store with code or fetch from user repo
        is_editor: false, // TODO: Store with code or fetch from user repo
        ip_addr: ip_addr.clone(),
        user_agent: user_agent.clone(),
        refresh_token: None,
        audience: code_data.resource.clone(),
    };

    match oauth2_state
        .auth_manager
        .session_manager()
        .create_session(session_params)
        .await
    {
        Ok(session_token) => {
            tracing::info!(
                "Token exchange successful, created session for user: {}",
                code_data.user_id
            );

            let response = TokenResponse {
                access_token: session_token.token, // Extract the token string from SessionToken struct
                token_type: "Bearer".to_string(),
                expires_in: Some(3600),
                refresh_token: None,
                scope: code_data.scope,
            };

            (StatusCode::OK, Json(response)).into_response()
        }
        Err(e) => {
            tracing::error!("Failed to create session: {:?}", e);
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "server_error".to_string(),
                    message: "Failed to create session".to_string(),
                }),
            )
                .into_response()
        }
    }
}

// Helper functions for token endpoint
fn extract_client_ip_from_headers(headers: &HeaderMap) -> String {
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

fn extract_user_agent_from_headers(headers: &HeaderMap) -> String {
    headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("unknown")
        .to_string()
}

/// Create authentication router with all routes
pub fn create_auth_router(auth_manager: Arc<AuthManager>) -> Router {
    Router::new()
        .route("/login", get(login_page))
        .route("/login/{provider}", get(start_login))
        .route("/callback/{provider}", get(oauth_callback))
        .route("/logout", get(logout).post(logout))
        .route("/status", get(auth_status))
        .with_state(auth_manager)
}

/// Create OAuth2 metadata and registration router
pub fn create_oauth2_router(
    metadata_config: Arc<MetadataConfig>,
    registration_manager: Option<Arc<ClientRegistrationManager>>,
    auth_manager: Arc<AuthManager>,
    pool: PgPool,
) -> Router {
    let metadata_router = Router::new()
        .route(
            "/.well-known/oauth-authorization-server",
            get(metadata_handler),
        )
        .route(
            "/.well-known/oauth-protected-resource",
            get(protected_resource_metadata_handler),
        )
        .route(
            "/.well-known/oauth-protected-resource/{*resource}",
            get(protected_resource_metadata_handler),
        )
        .with_state(metadata_config);

    // Add OAuth 2.0 protocol endpoints
    // Enable CORS for token endpoint to allow MCP clients on localhost
    let cors = CorsLayer::new()
        .allow_origin(Any) // Allow requests from any origin (needed for localhost MCP clients)
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::OPTIONS,
        ])
        .allow_headers(Any);

    let oauth2_state = OAuth2State::new(auth_manager, pool);

    let oauth2_protocol_router = Router::new()
        .route("/oauth2/authorize", get(oauth2_authorize))
        .route("/authorize", get(oauth2_authorize)) // Also support /authorize for compatibility
        .route("/oauth2/token", post(oauth2_token))
        .route("/token", post(oauth2_token)) // Also support /token for compatibility
        .layer(cors)
        .with_state(oauth2_state);

    // Add dynamic client registration endpoint if enabled
    let router = metadata_router.merge(oauth2_protocol_router);

    if let Some(manager) = registration_manager {
        let registration_router = Router::new()
            .route("/oauth2/register", post(register_client_handler))
            .with_state(manager);

        router.merge(registration_router)
    } else {
        router
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn test_get_client_ip_from_forwarded() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("192.168.1.1, 10.0.0.1"),
        );

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
            HeaderValue::from_static("Mozilla/5.0 Test"),
        );

        let ua = get_user_agent(&headers);
        assert_eq!(ua, "Mozilla/5.0 Test");
    }
}
