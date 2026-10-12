//! A person deleting their own account.
//!
//! The administrator's `delete_user` and this page erase the same things
//! (`user_repository::delete_user`): the account, and what the engine and every
//! table declaring a `person` keep about it. What this adds is the person's
//! own way to ask, behind the proof `/auth/elevate` asks for — a cookie alone
//! must not be able to erase somebody.

use super::*;
use crate::auth::AuthManager;
use crate::engine_page::Width;
use crate::security::client_ip;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Json, Redirect, Response},
};
use serde::Deserialize;
use std::sync::Arc;

pub(super) const DELETE_ACCOUNT_PATH: &str = "/auth/account/delete";

/// What has to be typed to confirm, so a stray click or a form filled by
/// something else cannot do it.
const CONFIRMATION: &str = "delete";

/// Request body for `POST /auth/account/delete`.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
pub struct DeleteAccountRequest {
    /// The account's password, when it has one.
    #[serde(default)]
    pub password: Option<String>,
    /// The word `delete`, typed.
    #[serde(default)]
    pub confirm: Option<String>,
    #[serde(default)]
    pub csrf_token: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct DeleteAccountParams {
    #[serde(default)]
    error: Option<String>,
}

fn deletion_error_message(code: &str) -> &'static str {
    match code {
        "confirm" => "Type delete to confirm. Nothing was deleted.",
        "last_administrator" => {
            "You are the only administrator, so deleting your account would leave nobody \
             able to administer this engine. Make somebody else an administrator first."
        }
        other => account_error_message(other),
    }
}

/// Whether `user_id` is the one administrator there is.
fn is_last_administrator(user_id: &str) -> Result<bool, crate::error::AppError> {
    use crate::user_repository::UserRole;
    let users = crate::user_repository::list_users()?;
    let admins: Vec<_> = users
        .iter()
        .filter(|user| user.has_role(&UserRole::Administrator))
        .collect();
    Ok(admins.len() <= 1 && admins.iter().any(|user| user.id == user_id))
}

/// The page that asks.
pub async fn delete_account_page(
    State(auth_manager): State<Arc<AuthManager>>,
    Query(params): Query<DeleteAccountParams>,
    headers: HeaderMap,
) -> Response {
    let config = auth_manager.config();
    let ip_addr = client_ip::from_headers(&headers);
    let user_agent = client_ip::user_agent_from_headers(&headers);
    let host = get_request_host(&headers);

    let token =
        session_token_from_headers(&headers, &config.session_cookie_name).unwrap_or_default();
    let session = if token.is_empty() {
        None
    } else {
        auth_manager
            .get_session(&token, &ip_addr, &user_agent, host.as_deref())
            .await
            .ok()
    };
    let Some(session) = session else {
        return Redirect::to(&format!(
            "/auth/login?redirect={}",
            urlencoding::encode(DELETE_ACCOUNT_PATH)
        ))
        .into_response();
    };

    let username = match crate::auth::local::username_for_user(&session.user_id).await {
        Ok(username) => username,
        Err(e) => {
            tracing::error!("Could not read the credential for the deletion page: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error").into_response();
        }
    };
    let last_admin = match is_last_administrator(&session.user_id) {
        Ok(last) => last,
        Err(e) => {
            tracing::error!(
                "Could not count administrators for the deletion page: {}",
                e
            );
            return (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error").into_response();
        }
    };

    let nonce = crate::security::generate_nonce();
    let csrf_token = auth_manager
        .security_context()
        .csrf
        .generate_token(Some(session.user_id.clone()))
        .await
        .token;

    let (proof, can_prove) = presence_fields(
        &auth_manager,
        &session,
        username.as_deref(),
        host.as_deref(),
        DELETE_ACCOUNT_PATH,
        "Deleting your account",
    );
    let notice_block = match (params.error.as_deref(), last_admin) {
        (Some(code), _) => format!(
            r#"<div class="aw-notice">{}</div>"#,
            deletion_error_message(code)
        ),
        (None, true) => format!(
            r#"<div class="aw-notice">{}</div>"#,
            deletion_error_message("last_administrator")
        ),
        (None, false) => String::new(),
    };
    let can_submit = can_prove && !last_admin;

    let body = format!(
        r#"<h1>Delete your account</h1>
        {notice_block}
        <p class="aw-identity">Signed in as <strong>{label}</strong></p>
        <p class="aw-explain">This cannot be undone. Deleting the account erases, in every app on
        this engine: what the apps keep for you in their storage and their tables, the keys and
        secrets you stored, linked chats, the permissions you gave apps and clients, ownership of
        your scripts, and work queued to run as you. Every session you have is signed out.</p>
        <p class="aw-explain">Records of what was done stay, without your name: a script you
        changed still shows that somebody changed it.</p>
        <form method="post" action="{action}">
            <input type="hidden" name="csrf_token" value="{csrf}">
            {proof}
            <label class="aw-field">Type <strong>{confirmation}</strong> to confirm
                <input type="text" name="confirm" autocomplete="off" required>
            </label>
            <button type="submit"{submit_disabled}>Delete my account</button>
        </form>
        <p class="aw-small"><a href="{account}">Keep my account</a></p>"#,
        notice_block = notice_block,
        label = html_escape::encode_text(
            &username
                .clone()
                .or_else(|| session.email.clone())
                .unwrap_or_else(|| session.user_id.clone())
        ),
        action = DELETE_ACCOUNT_PATH,
        csrf = html_attribute(&csrf_token),
        proof = proof,
        confirmation = CONFIRMATION,
        submit_disabled = if can_submit { "" } else { " disabled" },
        account = ACCOUNT_PATH,
    );

    let mut response = signed_in_page_response("Delete your account", Width::Narrow, &body, &nonce);
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}

/// Delete the signed-in person's account.
#[utoipa::path(
    post,
    path = "/auth/account/delete",
    tags = ["Authentication"],
    request_body = DeleteAccountRequest,
    responses(
        (status = 200, description = "The account and what was kept about the person are erased"),
        (status = 302, description = "Form post refused; back to the page with the reason"),
        (status = 401, description = "No session, a wrong password, or no recent sign-in"),
        (status = 409, description = "The last administrator cannot delete their account"),
    )
)]
pub async fn delete_account_route(
    State(auth_manager): State<Arc<AuthManager>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, AuthErrorResponse> {
    use crate::auth::error::AuthError;

    let (request, style) = parse_auth_body::<DeleteAccountRequest>(&headers, &body);
    let ip_addr = client_ip::from_headers(&headers);
    let user_agent = client_ip::user_agent_from_headers(&headers);
    let config = auth_manager.config();

    let token = session_token_from_headers(&headers, &config.session_cookie_name)
        .ok_or(AuthError::AuthenticationRequired)?;
    let session = auth_manager
        .get_session(
            &token,
            &ip_addr,
            &user_agent,
            get_request_host(&headers).as_deref(),
        )
        .await
        .map_err(|_| AuthError::AuthenticationRequired)?;

    let back = |code: &str| -> Response {
        Redirect::to(&format!("{}?error={}", DELETE_ACCOUNT_PATH, code)).into_response()
    };

    if require_session_form_csrf(
        &auth_manager,
        style,
        request.csrf_token.as_deref(),
        &session.user_id,
        true,
    )
    .await
    .is_err()
    {
        return Ok(back(error_code_for(&AuthError::CsrfValidationFailed)));
    }

    let confirmed = request
        .confirm
        .as_deref()
        .is_some_and(|typed| typed.trim().eq_ignore_ascii_case(CONFIRMATION));
    if !confirmed {
        return match style {
            RequestStyle::Form => Ok(back("confirm")),
            _ => Ok((
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "confirm must be \"delete\"" })),
            )
                .into_response()),
        };
    }

    let method = match prove_presence(
        &auth_manager,
        &session,
        request.password.as_deref(),
        &ip_addr,
    )
    .await
    {
        Ok(method) => method,
        Err(error @ AuthError::Internal(_)) => return Err(error.into()),
        Err(error) if style == RequestStyle::Form => return Ok(back(error_code_for(&error))),
        Err(error) => return Err(error.into()),
    };

    if is_last_administrator(&session.user_id)
        .map_err(|e| AuthError::Internal(format!("could not count administrators: {e}")))?
    {
        return match style {
            RequestStyle::Form => Ok(back("last_administrator")),
            _ => Ok((
                StatusCode::CONFLICT,
                Json(serde_json::json!({
                    "error": "the last administrator cannot delete their account"
                })),
            )
                .into_response()),
        };
    }

    crate::user_repository::delete_user(&session.user_id).map_err(|e| {
        tracing::error!("Could not delete an account at its owner's request: {}", e);
        AuthError::Internal("could not delete the account".into())
    })?;

    auth_manager
        .security_context()
        .auditor
        .log_event(
            crate::security::SecurityEvent::new(
                crate::security::SecurityEventType::SystemSecurityEvent,
                crate::security::SecuritySeverity::High,
                Some(session.user_id.clone()),
            )
            .with_detail("event", "account_deleted_by_owner")
            .with_detail("method", method.as_str())
            .with_detail("ip_address", &ip_addr),
        )
        .await;

    // The session row is gone with the account; the cookie is cleared so the
    // browser stops presenting it. `Secure` must match how it was set, or a
    // `__Host-` cookie's deletion is rejected (`logout`).
    let clear = format!(
        "{}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{}",
        config.session_cookie_name,
        if config.cookie_secure { "; Secure" } else { "" }
    );
    let mut response = match style {
        RequestStyle::Form => {
            let nonce = crate::security::generate_nonce();
            page_response(
                "Account deleted",
                Width::Narrow,
                r#"<h1>Your account is deleted</h1>
        <p class="aw-explain">It is gone, and so is what this engine and its apps kept for
        you. You are signed out.</p>
        <p class="aw-small"><a href="/">Home</a></p>"#,
                &nonce,
            )
        }
        _ => Json(serde_json::json!({ "deleted": true })).into_response(),
    };
    if let Ok(value) = clear.parse() {
        response.headers_mut().insert(header::SET_COOKIE, value);
    }
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    Ok(response)
}
