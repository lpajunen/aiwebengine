//! Signing out, the session's status, and refreshing it.

use super::*;
use crate::auth::AuthManager;
use crate::security::client_ip;
use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use chrono::Utc;
use std::sync::Arc;

/// Logout handler - destroys session
///
/// Served on both methods: a link signs out by GET, a form by POST.
#[utoipa::path(
    method(get, post),
    path = "/auth/logout",
    tags = ["Authentication"],
    params(
        ("redirect" = Option<String>, Query, description = "Redirect URL after logout")
    ),
    responses(
        (status = 302, description = "Redirect to specified location with session cleared"),
        (status = 400, description = "Invalid request", body = crate::openapi_schemas::ErrorResponse),
    )
)]
pub async fn logout(
    State(auth_manager): State<Arc<AuthManager>>,
    Query(params): Query<LogoutParams>,
    headers: HeaderMap,
) -> Result<Response, ErrorResponse> {
    let config = auth_manager.config();

    // Extract session token from cookie
    let session_token = session_token_from_headers(&headers, &config.session_cookie_name);

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

    // Clear cookie. `Secure` has to match what the cookie was set with: under
    // the `__Host-` prefix a browser rejects the whole `Set-Cookie` without it,
    // and a rejected deletion leaves the stale cookie in place.
    let cookie_value = format!(
        "{}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{}",
        config.session_cookie_name,
        if config.cookie_secure { "; Secure" } else { "" }
    );

    // Redirect to specified location or home
    let redirect_url = safe_redirect_target(params.redirect.as_deref());
    let response = Redirect::to(&redirect_url).into_response();
    let (mut parts, body) = response.into_parts();
    let cookie_header = cookie_value.parse().map_err(|_| ErrorResponse {
        error: "internal_error".to_string(),
        message: "Invalid cookie header value".to_string(),
    })?;
    parts.headers.insert(header::SET_COOKIE, cookie_header);

    Ok(Response::from_parts(parts, body))
}

/// Status endpoint - check authentication status
#[utoipa::path(
    get,
    path = "/auth/status",
    tags = ["Authentication"],
    responses(
        (status = 200, description = "Authentication status", body = crate::openapi_schemas::AuthStatusResponse),
    )
)]
/// Answers "am I signed in, and as whom" — deliberately not "with what token".
///
/// The session cookie is `HttpOnly` so that a script injected into a page
/// cannot read it. Returning the token here handed it back through a `fetch`
/// and made that flag decorative, and what leaked is a credential a bearer
/// header accepts. Do not add it back.
pub async fn auth_status(
    State(auth_manager): State<Arc<AuthManager>>,
    headers: HeaderMap,
) -> Json<AuthResponse> {
    let ip_addr = client_ip::from_headers(&headers);
    let user_agent = client_ip::user_agent_from_headers(&headers);

    let config = auth_manager.config();

    // Extract session token
    let session_token = session_token_from_headers(&headers, &config.session_cookie_name);
    if let Some(token) = session_token
        && let Ok(session) = auth_manager
            .get_session(
                &token,
                &ip_addr,
                &user_agent,
                get_request_host(&headers).as_deref(),
            )
            .await
    {
        let label = session
            .email
            .clone()
            .or_else(|| session.name.clone())
            .unwrap_or_else(|| session.user_id.clone());
        return Json(AuthResponse {
            success: true,
            user_id: Some(session.user_id),
            is_admin: Some(session.is_admin),
            is_editor: Some(session.is_editor),
            label: Some(label),
            redirect: None,
        });
    }

    Json(AuthResponse {
        success: false,
        user_id: None,
        is_admin: None,
        is_editor: None,
        label: None,
        redirect: Some("/auth/login".to_string()),
    })
}

/// Refresh authenticated session and renew cookie
#[utoipa::path(
    post,
    path = "/auth/refresh",
    tags = ["Authentication"],
    responses(
        (status = 200, description = "Session refreshed", body = RefreshResponse),
        (status = 401, description = "Session missing or invalid", body = crate::openapi_schemas::ErrorResponse),
    )
)]
pub async fn refresh_session(
    State(auth_manager): State<Arc<AuthManager>>,
    headers: HeaderMap,
) -> Response {
    let config = auth_manager.config();
    let ip_addr = client_ip::from_headers(&headers);
    let user_agent = client_ip::user_agent_from_headers(&headers);

    let session_token = session_token_from_headers(&headers, &config.session_cookie_name);

    let Some(token) = session_token else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "missing_session".to_string(),
                message: "No active session cookie found".to_string(),
            }),
        )
            .into_response();
    };

    match auth_manager
        .session_manager()
        .refresh_session(
            &token,
            &ip_addr,
            &user_agent,
            &crate::hosts::canonical_host(get_request_host(&headers).as_deref()),
            None,
        )
        .await
    {
        Ok(session) => {
            // Use the actual remaining lifetime from the DB session so the cookie
            // never outlives the record (important near the 30-day absolute cap).
            let remaining_secs = (session.expires_at - Utc::now()).num_seconds().max(0) as u64;
            let cookie_value = format!(
                "{}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{}",
                config.session_cookie_name,
                token,
                remaining_secs,
                if config.cookie_secure { "; Secure" } else { "" }
            );

            let response = Json(RefreshResponse {
                success: true,
                message: "Session refreshed".to_string(),
            })
            .into_response();

            let (mut parts, body) = response.into_parts();
            let cookie_header = match cookie_value.parse() {
                Ok(value) => value,
                Err(_) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ErrorResponse {
                            error: "internal_error".to_string(),
                            message: "Invalid cookie header value".to_string(),
                        }),
                    )
                        .into_response();
                }
            };

            parts.headers.insert(header::SET_COOKIE, cookie_header);
            Response::from_parts(parts, body)
        }
        Err(_) => (
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "invalid_session".to_string(),
                message: "Session missing, expired, or invalid".to_string(),
            }),
        )
            .into_response(),
    }
}
