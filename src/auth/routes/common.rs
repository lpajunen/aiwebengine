//! Request plumbing the auth pages share: cookies, redirects, request styles, CSRF
//! and error answers.

use super::*;
use crate::auth::AuthManager;
use crate::engine_page::Width;
use axum::{
    Json,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use serde::Serialize;

/// JSON response for successful authentication
#[derive(Debug, Serialize)]
pub struct AuthResponse {
    pub success: bool,
    pub user_id: Option<String>,
    pub is_admin: Option<bool>,
    pub is_editor: Option<bool>,
    /// What to call the person: their email, else their name, else their id.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub redirect: Option<String>,
}

/// JSON response for session refresh
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RefreshResponse {
    pub success: bool,
    pub message: String,
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

/// Read one cookie's value out of a request's `Cookie` header.
pub(super) fn cookie_from_headers(headers: &HeaderMap, cookie_name: &str) -> Option<String> {
    headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').find_map(|cookie| {
                let (name, value) = cookie.trim().split_once('=')?;
                if name == cookie_name {
                    Some(value.to_string())
                } else {
                    None
                }
            })
        })
}

/// Read the session token out of the request's cookies.
pub(super) fn session_token_from_headers(headers: &HeaderMap, cookie_name: &str) -> Option<String> {
    cookie_from_headers(headers, cookie_name)
}

/// Add a `Set-Cookie` to a response.
///
/// Appends rather than inserts, because a response may carry more than one —
/// the OAuth callback sets a session and retires a pending-login cookie in the
/// same reply — and because a cookie a script route set has to survive beside
/// the engine's.
pub(super) fn append_cookie(headers: &mut HeaderMap, value: &str) -> Result<(), ErrorResponse> {
    let cookie = value.parse().map_err(|_| ErrorResponse {
        error: "internal_error".to_string(),
        message: "Invalid cookie header value".to_string(),
    })?;
    headers.append(header::SET_COOKIE, cookie);
    Ok(())
}

/// Build the `Set-Cookie` value that carries a session.
///
/// Max-Age is the absolute session age rather than the idle timeout, so the
/// browser keeps the cookie for as long as the session can live.
///
/// `SameSite=Lax` is written unconditionally rather than read from
/// configuration, matching what the OAuth callback has always sent. It is load
/// bearing: it is what stops a cross-site POST from carrying the session, and
/// so what protects `/auth/local/claim` — an endpoint that, reached with a
/// victim's session, would attach an attacker's password to their account.
pub(super) fn session_cookie_value(
    config: &crate::auth::manager::AuthManagerConfig,
    token: &str,
) -> String {
    format!(
        "{}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{}",
        config.session_cookie_name,
        token,
        config.max_session_age,
        if config.cookie_secure { "; Secure" } else { "" }
    )
}

/// Extract the host a request was addressed to, used to pick the OAuth
/// redirect URI so a login completes on the host it started on, and the issuer
/// the discovery documents advertise.
///
/// The value is only ever used as a lookup key against hosts registered at
/// startup, so an unrecognised or spoofed Host header degrades to the
/// configured base URL rather than steering the flow anywhere new.
pub(crate) fn get_request_host(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty())
}

/// Reduce a caller-supplied post-login redirect to a same-host relative path.
///
/// The value originates in a query parameter, so an absolute URL here would
/// let anyone bounce a freshly authenticated user to another origin. Keeping
/// it relative also keeps the user on the host whose session cookie was just
/// set. Applied when a login starts, before the target is stored in the
/// pending-login cookie, and again on the way out.
pub(super) fn safe_redirect_target(candidate: Option<&str>) -> String {
    let fallback = "/".to_string();
    let Some(target) = candidate else {
        return fallback;
    };
    let target = target.trim();

    // Must be an absolute path. Reject protocol-relative ("//host") and
    // backslash variants that some browsers normalise into an authority.
    if !target.starts_with('/')
        || target.starts_with("//")
        || target.starts_with("/\\")
        || target.contains(['\r', '\n'])
    {
        return fallback;
    }

    target.to_string()
}

/// Escape a value being placed inside a double-quoted HTML attribute.
///
/// Both of the values this page interpolates are engine-produced — an HMAC
/// token and a path already reduced by `safe_redirect_target` — so this is a
/// belt on top of braces rather than the only thing standing between a query
/// parameter and the page. It is here so that stays true if a third value is
/// added later.
pub(super) fn html_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Serve one of the engine's pages: `body` inside the shared shell, under a
/// policy naming the nonce its inline blocks carry.
pub(super) fn page_response(title: &str, width: Width, body: &str, nonce: &str) -> Response {
    crate::engine_page::response(
        StatusCode::OK,
        crate::engine_page::document(title, nonce, width, body),
        nonce,
    )
}

/// [`page_response`] for a page only a signed-in person sees, with the account
/// menu in its corner.
pub(super) fn signed_in_page_response(
    title: &str,
    width: Width,
    body: &str,
    nonce: &str,
) -> Response {
    let body = format!("{body}\n{}", crate::engine_page::ACCOUNT_MENU);
    page_response(title, width, &body, nonce)
}

/// A timestamp, in the one spelling this page uses.
///
/// UTC and absolute rather than "three minutes ago". A relative time reads
/// better and is the wrong tool here: the question a person is answering is
/// whether *they* were signed in at that moment, and "yesterday at 03:11" is
/// something you can check against your own day.
pub(super) fn page_timestamp(at: chrono::DateTime<chrono::Utc>) -> String {
    at.format("%Y-%m-%d %H:%M UTC").to_string()
}

pub(super) fn page_timestamp_utc(at: chrono::DateTime<chrono::Utc>) -> String {
    at.format("%Y-%m-%d %H:%M UTC").to_string()
}

/// How a request arrived, which decides how the answer is shaped and whether a
/// CSRF token is demanded.
///
/// A form submission is a browser, so it wants a redirect and a rendered error
/// rather than JSON — and it is the shape an attacker's page can forge, since
/// a cross-site form POST needs no preflight. A JSON body cannot be sent
/// cross-origin without a CORS preflight the engine does not grant, so it is
/// already unforgeable and asking it for a token would only break API callers
/// that have no page to take one from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum RequestStyle {
    Json,
    Form,
}

/// Read a request body as JSON or as an HTML form, reporting which it was.
///
/// Mirrors what the engine API does for role changes: two short scalar fields
/// are worth accepting in either shape rather than making callers guess.
pub(super) fn parse_auth_body<T: serde::de::DeserializeOwned + Default>(
    headers: &HeaderMap,
    body: &[u8],
) -> (T, RequestStyle) {
    let is_form = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .eq_ignore_ascii_case("application/x-www-form-urlencoded")
        });

    if is_form {
        (
            serde_urlencoded::from_bytes(body).unwrap_or_default(),
            RequestStyle::Form,
        )
    } else {
        (
            serde_json::from_slice(body).unwrap_or_default(),
            RequestStyle::Json,
        )
    }
}

/// JSON answer to an internal-credential flow.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct InternalAuthResponse {
    pub success: bool,
    pub user_id: Option<String>,
    /// Present once an account has a username; absent for a guest.
    pub username: Option<String>,
}

/// An [`AuthError`] on its way out over HTTP, carrying the status the error
/// itself decides rather than flattening everything to 400.
///
/// The message is the error's own `Display`, which is why
/// [`AuthError::InvalidCredentials`] is deliberately one variant for "no such
/// user" and "wrong password" — the response cannot leak a distinction the
/// error does not draw.
pub struct AuthErrorResponse(crate::auth::error::AuthError);

impl From<crate::auth::error::AuthError> for AuthErrorResponse {
    fn from(error: crate::auth::error::AuthError) -> Self {
        Self(error)
    }
}

impl IntoResponse for AuthErrorResponse {
    fn into_response(self) -> Response {
        let status =
            StatusCode::from_u16(self.0.status_code()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        // An internal fault must not describe itself to the caller.
        let message = if status == StatusCode::INTERNAL_SERVER_ERROR {
            tracing::error!("internal authentication error: {}", self.0);
            "Authentication failed".to_string()
        } else {
            self.0.to_string()
        };
        (
            status,
            Json(ErrorResponse {
                error: "authentication_failed".to_string(),
                message,
            }),
        )
            .into_response()
    }
}

/// Refuse a form submission that did not carry a valid CSRF token.
///
/// Only form submissions. A cross-site page can POST a form to any origin
/// without a preflight, which is how login CSRF logs a victim into an
/// attacker's account; it cannot POST `application/json` without one the
/// engine does not grant. Tokens are stateless HMACs, so this works before
/// there is any session to bind one to.
pub(super) async fn require_form_csrf(
    auth_manager: &AuthManager,
    style: RequestStyle,
    token: Option<&str>,
) -> Result<(), AuthErrorResponse> {
    if style != RequestStyle::Form {
        return Ok(());
    }

    let token = token.ok_or(AuthErrorResponse(
        crate::auth::error::AuthError::CsrfValidationFailed,
    ))?;

    auth_manager
        .security_context()
        .csrf
        .validate_token(token, None)
        .await
        .map_err(|_| AuthErrorResponse(crate::auth::error::AuthError::CsrfValidationFailed))
}

/// Refuse a form submission that acts on a session, unless its token was issued
/// to that session.
///
/// [`require_form_csrf`] accepts a token bound to nobody, which is right for
/// the sign-in forms: they are submitted before there is a session to bind one
/// to. It is wrong for a form that changes an account, because an unbound token
/// is one anybody can fetch from `/auth/login` with no browser and no account,
/// leaving only the cookie's `SameSite=Lax` between the form and a cross-site
/// POST — and a browser will carry a Lax cookie on a cross-site POST for the
/// first couple of minutes after it is set.
///
/// `binding_required` is the difference between the two endpoints this guards.
/// The password form is new, so nothing was ever submitting an unbound token to
/// it and it can demand a bound one. `/auth/local/claim` has been reachable
/// from solutions' own pages since it shipped, taking a token from the sign-in
/// page, so it accepts either — a bound token must match the session, an
/// unbound one is still allowed.
pub(super) async fn require_session_form_csrf(
    auth_manager: &AuthManager,
    style: RequestStyle,
    token: Option<&str>,
    user_id: &str,
    binding_required: bool,
) -> Result<(), AuthErrorResponse> {
    if style != RequestStyle::Form {
        return Ok(());
    }

    let token = token.ok_or(AuthErrorResponse(
        crate::auth::error::AuthError::CsrfValidationFailed,
    ))?;

    let csrf = &auth_manager.security_context().csrf;
    let outcome = if binding_required {
        csrf.validate_token_for(token, user_id).await
    } else {
        csrf.validate_token(token, Some(user_id)).await
    };

    outcome.map_err(|_| AuthErrorResponse(crate::auth::error::AuthError::CsrfValidationFailed))
}

/// Short, fixed codes for what a browser is told went wrong.
///
/// The login page renders a message chosen from these rather than echoing the
/// error, so nothing a caller supplies reaches the page — and so the reasons
/// stay as coarse as [`AuthError::InvalidCredentials`] intends.
pub(super) fn error_code_for(error: &crate::auth::error::AuthError) -> &'static str {
    use crate::auth::error::AuthError as E;
    match error {
        E::InvalidCredentials => "credentials",
        E::UsernameTaken => "taken",
        E::CredentialAlreadySet => "claimed",
        E::InvalidUsername(_) => "username",
        E::WeakPassword(_) => "password",
        E::LocalAuthDisabled => "disabled",
        E::GuestAuthDisabled => "guests_disabled",
        E::RecoveryCodesDisabled => "recovery_disabled",
        E::RateLimitExceeded => "rate_limit",
        E::CsrfValidationFailed => "csrf",
        // Both reachable from the elevation page, where "failed" would be the
        // least useful thing to read: one means the account cannot hold what
        // was asked for and the other means it has not proved it is here.
        E::InsufficientPermissions => "permissions",
        E::AuthenticationRequired => "authentication_required",
        _ => "failed",
    }
}

/// The answer a browser gets: back to the login page, carrying a code.
pub(super) fn redirect_to_login_with_error(
    error: &crate::auth::error::AuthError,
    redirect: Option<&str>,
) -> Response {
    redirect_to_login_form_with_error(error, redirect, LoginForm::SignIn)
}

/// The same, naming the form to come back to.
///
/// A failed recovery has to land on the recovery form and not on the sign-in
/// form: the person submitting it does not know their password, which is the
/// one thing the page would otherwise be asking them for.
pub(super) fn redirect_to_login_form_with_error(
    error: &crate::auth::error::AuthError,
    redirect: Option<&str>,
    form: LoginForm,
) -> Response {
    let form_param = match form {
        LoginForm::SignIn => "",
        LoginForm::SignUp => "signup=1&",
        LoginForm::Recover => "recover=1&",
    };

    let target = match redirect {
        Some(value) => format!(
            "/auth/login?{}error={}&redirect={}",
            form_param,
            error_code_for(error),
            urlencoding::encode(&safe_redirect_target(Some(value)))
        ),
        None => format!("/auth/login?{}error={}", form_param, error_code_for(error)),
    };
    Redirect::to(&target).into_response()
}

/// The answer a browser gets when a form submitted from the account page fails.
///
/// Which page that is comes from where the submission was going: a form whose
/// success lands on the account page was submitted from the account page, and
/// its error message belongs there rather than on the sign-in page. Sending a
/// signed-in person to a sign-in page to read "that is not your current
/// password" hides both the message and the thing they were doing.
///
/// Everything else keeps going back to the sign-in page, which is where a
/// solution posting these forms from its own UI has always been sent.
pub(super) fn redirect_to_form_with_error(
    error: &crate::auth::error::AuthError,
    redirect: Option<&str>,
) -> Response {
    let from_account_page = redirect
        .map(|value| safe_redirect_target(Some(value)))
        .is_some_and(|target| {
            target == ACCOUNT_PATH || target.starts_with(&format!("{}?", ACCOUNT_PATH))
        });

    if from_account_page {
        return Redirect::to(&format!("{}?error={}", ACCOUNT_PATH, error_code_for(error)))
            .into_response();
    }

    redirect_to_login_with_error(error, redirect)
}

/// Shape the answer to an internal-credential flow by how the request arrived:
/// a browser that submitted a form is sent on its way, an API caller gets JSON.
pub(super) fn respond_to_style(
    auth_manager: &AuthManager,
    style: RequestStyle,
    token: &str,
    redirect: Option<&str>,
    body: InternalAuthResponse,
) -> Result<Response, AuthErrorResponse> {
    match style {
        RequestStyle::Json => respond_with_session(auth_manager, token, body),
        RequestStyle::Form => {
            let target = safe_redirect_target(redirect);
            let cookie = session_cookie_value(auth_manager.config(), token);
            let response = Redirect::to(&target).into_response();
            let (mut parts, body) = response.into_parts();
            let header_value = cookie.parse().map_err(|_| {
                AuthErrorResponse(crate::auth::error::AuthError::Internal(
                    "invalid cookie header value".to_string(),
                ))
            })?;
            parts.headers.insert(header::SET_COOKIE, header_value);
            Ok(Response::from_parts(parts, body))
        }
    }
}

/// Attach a freshly minted session to a JSON response.
pub(super) fn respond_with_session(
    auth_manager: &AuthManager,
    token: &str,
    body: InternalAuthResponse,
) -> Result<Response, AuthErrorResponse> {
    let cookie = session_cookie_value(auth_manager.config(), token);
    let response = Json(body).into_response();
    let (mut parts, body) = response.into_parts();
    let header_value = cookie.parse().map_err(|_| {
        AuthErrorResponse(crate::auth::error::AuthError::Internal(
            "invalid cookie header value".to_string(),
        ))
    })?;
    parts.headers.insert(header::SET_COOKIE, header_value);
    Ok(Response::from_parts(parts, body))
}
