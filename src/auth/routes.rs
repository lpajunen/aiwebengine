use crate::auth::client_registration::{ClientRegistrationManager, register_client_handler};
use crate::auth::metadata::{
    MetadataConfig, metadata_handler, protected_resource_metadata_handler,
};
/// Authentication Routes
///
/// HTTP route handlers for OAuth2 authentication flow including
/// login initiation, callback processing, and logout.
use crate::auth::{AuthManager, AuthSecurityContext};
use crate::security::client_ip;
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

use sqlx::{PgPool, Row};

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
    pub user_id: Option<String>,
    pub is_admin: Option<bool>,
    pub is_editor: Option<bool>,
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

/// Read the session token out of the request's cookies.
fn session_token_from_headers(headers: &HeaderMap, cookie_name: &str) -> Option<String> {
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
fn session_cookie_value(config: &crate::auth::manager::AuthManagerConfig, token: &str) -> String {
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
/// The OAuth state carrying this value is not authenticated, and the value
/// itself originates in a query parameter, so an absolute URL here would let
/// anyone bounce a freshly authenticated user to another origin. Keeping it
/// relative also keeps the user on the host whose session cookie was just set.
fn safe_redirect_target(candidate: Option<&str>) -> String {
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

/// Where the account page lives. Named once because the sign-in page links to
/// it, the page redirects back through it, and both forms on it post a redirect
/// target built from it.
const ACCOUNT_PATH: &str = "/auth/account";

/// The look of the engine's own sign-in and account pages.
///
/// One block for both, because they are one surface: a person moves between
/// them and should not be able to tell they changed pages. Inline rather than a
/// stylesheet, and carried under a per-response nonce, so the pages keep
/// working under a `style-src 'self'` policy with no inline allowance.
const AUTH_PAGE_STYLES: &str = r#"        body {
            margin: 0;
            padding: 1rem;
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
            font-size: 14px;
            line-height: 1.5;
            color: #212529;
            background: #f8f9fa;
            min-height: 100vh;
            display: flex;
            align-items: center;
            justify-content: center;
        }

        .card {
            width: 100%;
            max-width: 400px;
            background: #ffffff;
            border: 1px solid #dee2e6;
            border-radius: 8px;
            box-shadow: 0 1px 3px rgba(0, 0, 0, 0.1), 0 1px 2px rgba(0, 0, 0, 0.06);
            padding: 2rem;
            text-align: center;
        }

        .card h1 {
            margin: 0 0 1rem 0;
            font-size: 1.75rem;
        }

        .card p {
            color: #6c757d;
            margin: 0 0 1.5rem 0;
        }

        .notice {
            padding: 0.6rem 0.75rem;
            margin-bottom: 1rem;
            border: 1px solid #f1aeb5;
            border-radius: 6px;
            background: #fdf2f3;
            color: #842029;
        }

        .credentials, .guest {
            display: block;
            margin: 0 0 1rem;
        }

        .credentials h2 {
            margin: 0 0 0.75rem;
            font-size: 1rem;
            font-weight: 600;
        }

        .credentials label {
            display: block;
            margin-bottom: 0.25rem;
            font-weight: 500;
        }

        .credentials .hint {
            font-weight: 400;
            color: #6c757d;
        }

        .credentials input {
            display: block;
            width: 100%;
            box-sizing: border-box;
            padding: 0.6rem 0.75rem;
            margin-bottom: 0.75rem;
            border: 1px solid #ced4da;
            border-radius: 6px;
            font-size: 1rem;
            font-family: inherit;
        }

        .credentials input:focus {
            outline: 2px solid #4285f4;
            outline-offset: 1px;
            border-color: #4285f4;
        }

        .credentials button, .guest button {
            display: block;
            width: 100%;
            box-sizing: border-box;
            padding: 0.75rem 1rem;
            border: none;
            border-radius: 6px;
            font-weight: 500;
            font-size: 1rem;
            font-family: inherit;
            cursor: pointer;
            background-color: #212529;
            color: #ffffff;
        }

        .credentials button:hover, .guest button:hover {
            background-color: #343a40;
        }

        .guest button.secondary {
            background-color: #ffffff;
            color: #212529;
            border: 1px solid #ced4da;
        }

        .guest button.secondary:hover {
            background-color: #f1f3f5;
        }

        .switch {
            margin: 0.75rem 0 0;
            font-size: 0.9rem;
            color: #6c757d;
        }

        .divider {
            display: flex;
            align-items: center;
            gap: 0.75rem;
            margin: 1rem 0;
            color: #6c757d;
            font-size: 0.9rem;
        }

        .divider::before, .divider::after {
            content: "";
            flex: 1;
            border-top: 1px solid #dee2e6;
        }

        .provider-btn {
            display: block;
            width: 100%;
            box-sizing: border-box;
            padding: 0.75rem 1rem;
            margin-bottom: 0.5rem;
            border: none;
            border-radius: 6px;
            font-weight: 500;
            font-size: 1rem;
            text-decoration: none;
            transition: all 0.2s ease;
        }

        .provider-google {
            background-color: #4285f4;
            color: white;
        }

        .provider-google:hover {
            background-color: #3367d6;
        }

        .provider-microsoft {
            background-color: #00a4ef;
            color: white;
        }

        .provider-microsoft:hover {
            background-color: #0078d4;
        }

        .provider-apple {
            background-color: #000000;
            color: white;
        }

        .provider-apple:hover {
            background-color: #333333;
        }

        .identity {
            margin: 0 0 1.5rem;
            color: #212529;
        }

        .identity .provider {
            display: block;
            font-size: 0.9rem;
            color: #6c757d;
        }

        .notice.ok {
            border-color: #a3cfbb;
            background: #f0f9f4;
            color: #0f5132;
        }

        .explain {
            margin: 0 0 0.75rem;
            font-size: 0.9rem;
            color: #6c757d;
            text-align: left;
        }

        .codes {
            list-style: none;
            margin: 0 0 1.5rem;
            padding: 0.75rem;
            border: 1px solid #dee2e6;
            border-radius: 6px;
            background: #f8f9fa;
            font-family: ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, monospace;
            font-size: 1rem;
            letter-spacing: 0.05em;
        }

        .codes li {
            padding: 0.15rem 0;
        }
"#;

/// Login page parameters
#[derive(Debug, Deserialize)]
pub struct LoginPageParams {
    /// Optional redirect URL after successful login
    redirect: Option<String>,
    /// A code from a failed attempt, set by the engine when it bounces a form
    /// submission back here. Rendered through a fixed table of messages, never
    /// echoed, so a crafted link cannot put text on this page.
    #[serde(default)]
    error: Option<String>,
    /// Show the sign-up form rather than the sign-in form.
    #[serde(default)]
    signup: Option<String>,
    /// Show the recovery form rather than the sign-in form.
    #[serde(default)]
    recover: Option<String>,
}

/// The message shown for a failed attempt.
///
/// Chosen from a fixed table by code. Unknown codes get the generic message
/// rather than their own text — this page must not be a way to render
/// attacker-chosen words next to a password field.
fn login_error_message(code: &str) -> &'static str {
    match code {
        "credentials" => "That username and password do not match an account.",
        "taken" => "That username is already taken.",
        "claimed" => "This account already has a username and password.",
        "username" => "That username is not allowed. Use 3-32 letters, digits, _ . or -.",
        "password" => "That password is too short.",
        "disabled" => "Signing in with a username is not enabled here.",
        "guests_disabled" => "Guest access is not enabled here.",
        "rate_limit" => "Too many attempts. Wait a moment and try again.",
        "csrf" => "That form expired. Try again.",
        "recovery_disabled" => "Recovery codes are not enabled here.",
        _ => "Sign in failed. Try again.",
    }
}

/// The message shown beside the recovery form.
///
/// Differs from [`login_error_message`] exactly where the same code means
/// something else here: what did not match is a username and a code, and what
/// was too short is the new password being chosen.
fn recovery_error_message(code: &str) -> &'static str {
    match code {
        "credentials" => "That username and recovery code do not match an account.",
        "password" => "That new password is too short.",
        other => login_error_message(other),
    }
}

/// The form that spends a recovery code.
///
/// Three fields, because recovery is not a sign-in: the code is not a password
/// and cannot be used as one, so redeeming it and choosing the new password
/// happen in the same act. Anything else would leave an account reachable by a
/// code that had already been shown to work.
fn render_recovery_form(
    internal: &crate::auth::config::InternalAuthConfig,
    csrf_token: &str,
    redirect: &str,
    encoded_redirect: &str,
) -> String {
    if !internal.allow_recovery_codes {
        return String::new();
    }

    // Somebody who reached this form from a solution's page goes back to it.
    // Somebody who reached it from nowhere in particular — the default "/" —
    // goes to their account page instead, which is where the rest of their
    // codes are counted and where a fresh set is generated.
    let target = if redirect == "/" {
        format!("{}?notice=recovered", ACCOUNT_PATH)
    } else {
        redirect.to_string()
    };

    format!(
        r#"<form class="credentials" method="post" action="/auth/local/recover">
                <h2>Use a recovery code</h2>
                <p class="explain">One of the codes you were given when you set them up. Each works
                once, and using one sets a new password and signs out everywhere else.</p>
                <input type="hidden" name="csrf_token" value="{csrf}">
                <input type="hidden" name="redirect" value="{redirect}">
                <label for="username">Username</label>
                <input id="username" name="username" type="text" required autocomplete="username"
                       minlength="3" maxlength="32" autocapitalize="none" spellcheck="false">
                <label for="code">Recovery code</label>
                <input id="code" name="code" type="text" required autocomplete="one-time-code"
                       autocapitalize="none" spellcheck="false">
                <label for="new_password">New password</label>
                <input id="new_password" name="new_password" type="password" required
                       autocomplete="new-password" minlength="{min_password}">
                <button type="submit">Set a new password</button>
                <p class="switch">Remembered it? <a href="/auth/login?redirect={encoded_redirect}">Sign in</a></p>
            </form>"#,
        csrf = html_attribute(csrf_token),
        redirect = html_attribute(&target),
        min_password = internal
            .min_password_length
            .max(crate::auth::local::MIN_PASSWORD_LENGTH),
        encoded_redirect = encoded_redirect,
    )
}

/// Which of the sign-in page's forms is being shown.
///
/// One page with three states rather than three pages: they share the CSRF
/// token, the redirect target, the provider list below the divider and the
/// styling, and a person moving between them is answering one question — how
/// am I getting in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoginForm {
    /// Username and password.
    SignIn,
    /// Create an account.
    SignUp,
    /// Spend a recovery code and set a new password.
    Recover,
}

/// Render the sign-in, sign-up, recovery and guest controls for credentials the
/// engine holds itself.
///
/// Plain forms, no script: the engine's configured Content-Security-Policy
/// names `script-src 'self'` with no inline allowance, and a sign-in page is
/// the last place to depend on one being relaxed. Empty when nothing internal
/// is enabled, which is the default.
pub fn render_internal_auth_forms(
    internal: &crate::auth::config::InternalAuthConfig,
    csrf_token: &str,
    redirect: &str,
    encoded_redirect: &str,
    form: LoginForm,
) -> String {
    let mut blocks: Vec<String> = Vec::new();

    if internal.enabled && form == LoginForm::Recover {
        blocks.push(render_recovery_form(
            internal,
            csrf_token,
            redirect,
            encoded_redirect,
        ));
    } else if internal.enabled {
        let signing_up = form == LoginForm::SignUp;
        let (action, heading, button, name_field, switch) = if signing_up {
            (
                "/auth/local/register",
                "Create an account",
                "Create account",
                r#"<label for="name">Display name <span class="hint">(optional)</span></label>
                <input id="name" name="name" type="text" autocomplete="nickname">"#,
                format!(
                    r#"<p class="switch">Already have an account? <a href="/auth/login?redirect={}">Sign in</a></p>"#,
                    encoded_redirect
                ),
            )
        } else {
            (
                "/auth/local/login",
                "Sign in with a username",
                "Sign in",
                "",
                {
                    let mut links = String::new();
                    if internal.allow_registration {
                        links.push_str(&format!(
                            r#"<p class="switch">No account yet? <a href="/auth/login?signup=1&amp;redirect={}">Create one</a></p>"#,
                            encoded_redirect
                        ));
                    }
                    // The only way a person who has forgotten their password
                    // finds the thing that lets them in. It is on the sign-in
                    // form because that is where they are when they find out.
                    if internal.allow_recovery_codes {
                        links.push_str(&format!(
                            r#"<p class="switch">Forgotten it? <a href="/auth/login?recover=1&amp;redirect={}">Use a recovery code</a></p>"#,
                            encoded_redirect
                        ));
                    }
                    links
                },
            )
        };

        blocks.push(format!(
            r#"<form class="credentials" method="post" action="{action}">
                <h2>{heading}</h2>
                <input type="hidden" name="csrf_token" value="{csrf}">
                <input type="hidden" name="redirect" value="{redirect}">
                <label for="username">Username</label>
                <input id="username" name="username" type="text" required autocomplete="username"
                       minlength="3" maxlength="32" autocapitalize="none" spellcheck="false">
                {name_field}
                <label for="password">Password</label>
                <input id="password" name="password" type="password" required
                       autocomplete="{autocomplete}" minlength="{min_password}">
                <button type="submit">{button}</button>
                {switch}
            </form>"#,
            action = action,
            heading = heading,
            csrf = html_attribute(csrf_token),
            redirect = html_attribute(redirect),
            name_field = name_field,
            autocomplete = if signing_up {
                "new-password"
            } else {
                "current-password"
            },
            // The browser should ask for what the engine will accept, so a
            // password is rejected before it is sent rather than after.
            min_password = internal
                .min_password_length
                .max(crate::auth::local::MIN_PASSWORD_LENGTH),
            button = button,
            switch = switch,
        ));
    }

    // The way someone signed in finds the page that manages their credential.
    // It is a link rather than a form because everything on that page needs a
    // session and a current password, neither of which a sign-in page has —
    // and it is here because the sign-in page is where a person goes looking
    // when they are thinking about their password.
    if internal.enabled && form == LoginForm::SignIn {
        blocks.push(format!(
            r#"<p class="switch"><a href="{}">Change your password</a></p>"#,
            ACCOUNT_PATH
        ));
    }

    if internal.allow_guests {
        blocks.push(format!(
            r#"<form class="guest" method="post" action="/auth/guest">
                <input type="hidden" name="csrf_token" value="{csrf}">
                <input type="hidden" name="redirect" value="{redirect}">
                <button type="submit" class="secondary">Continue as guest</button>
            </form>"#,
            csrf = html_attribute(csrf_token),
            redirect = html_attribute(redirect),
        ));
    }

    blocks.join("\n        ")
}

/// Escape a value being placed inside a double-quoted HTML attribute.
///
/// Both of the values this page interpolates are engine-produced — an HMAC
/// token and a path already reduced by `safe_redirect_target` — so this is a
/// belt on top of braces rather than the only thing standing between a query
/// parameter and the page. It is here so that stays true if a third value is
/// added later.
fn html_attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Login page handler - displays available providers
#[utoipa::path(
    get,
    path = "/auth/login",
    tags = ["Authentication"],
    params(
        ("redirect" = Option<String>, Query, description = "Redirect URL after successful login")
    ),
    responses(
        (status = 200, description = "Login page HTML", content_type = "text/html"),
    )
)]
pub async fn login_page(
    State(auth_manager): State<Arc<AuthManager>>,
    Query(params): Query<LoginPageParams>,
) -> Response {
    let providers = auth_manager.list_providers();
    // Names the one <style> block below and nothing else, so anything injected
    // into this page stays inert. Fresh per response — a nonce a caller can
    // predict is not a nonce.
    let nonce = crate::security::generate_nonce();
    let redirect_param = safe_redirect_target(params.redirect.as_deref());
    let encoded_redirect = urlencoding::encode(&redirect_param);

    let internal = &auth_manager.config().internal;
    // One token serves every form on the page; they are all the same origin
    // and the same short lifetime.
    let csrf_token = auth_manager
        .security_context()
        .csrf
        .generate_token(None)
        .await
        .token;

    let error_block = params
        .error
        .as_deref()
        .map(|code| {
            // Read beside the recovery form, "that username and password do not
            // match an account" is about a field the form does not have.
            let message = if internal.allow_recovery_codes && params.recover.is_some() {
                recovery_error_message(code)
            } else {
                login_error_message(code)
            };
            format!(r#"<div class="notice">{}</div>"#, message)
        })
        .unwrap_or_default();

    // A form the configuration does not offer falls back to signing in, rather
    // than rendering a control that posts to an endpoint that would refuse.
    let form = if internal.allow_recovery_codes && params.recover.is_some() {
        LoginForm::Recover
    } else if internal.allow_registration && params.signup.is_some() {
        LoginForm::SignUp
    } else {
        LoginForm::SignIn
    };
    let internal_block = render_internal_auth_forms(
        internal,
        &csrf_token,
        &redirect_param,
        &encoded_redirect,
        form,
    );
    let providers_intro = if providers.is_empty() {
        String::new()
    } else if internal_block.is_empty() {
        "<p>Choose a provider to continue:</p>".to_string()
    } else {
        r#"<div class="divider"><span>or</span></div>"#.to_string()
    };

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Login</title>
    <link rel="icon" type="image/x-icon" href="/favicon.ico">
    <style nonce="{style_nonce}">{styles}    </style>
</head>
<body>
    <div class="card">
        <h1>Sign In</h1>
        {error_block}
        {internal_block}
        {providers_intro}
        {provider_buttons}
    </div>
</body>
</html>"#,
        style_nonce = html_attribute(&nonce),
        styles = AUTH_PAGE_STYLES,
        error_block = error_block,
        internal_block = internal_block,
        providers_intro = providers_intro,
        provider_buttons = {
            let mut sorted_providers = providers.clone();
            sorted_providers.sort();
            sorted_providers
                .iter()
                .map(|p| format!(
                    r#"<a href="/auth/login/{}?redirect={}" class="provider-btn provider-{}">{}</a>"#,
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

    html_page_response(html, &nonce)
}

/// Serve an engine-authored HTML page under a policy naming its own inline
/// blocks.
///
/// Set here rather than by the security-headers layer because only this side
/// knows the nonce it wrote into the markup. The layer fills in a header a
/// response did not set, so this wins.
fn html_page_response(html: String, nonce: &str) -> Response {
    let mut response = Html(html).into_response();
    match header::HeaderValue::from_str(&crate::security::engine_page_policy(nonce)) {
        Ok(value) => {
            response
                .headers_mut()
                .insert(header::CONTENT_SECURITY_POLICY, value);
        }
        Err(e) => {
            // Serving the page without its policy would let an injected inline
            // block run, which is the thing the nonce exists to prevent.
            tracing::error!("Could not build a content security policy: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error").into_response();
        }
    }
    response
}

/// Account page parameters. Both are engine-written codes, rendered through a
/// fixed table rather than echoed — the same rule the sign-in page follows.
#[derive(Debug, Deserialize)]
pub struct AccountPageParams {
    /// What just succeeded, set by the engine when it sends a form submission
    /// back here.
    #[serde(default)]
    notice: Option<String>,
    /// What just failed.
    #[serde(default)]
    error: Option<String>,
}

/// The message shown for something that just worked.
fn account_notice_message(code: &str) -> &'static str {
    match code {
        "password" => {
            "Your password has been changed. Every other session this account had is signed out."
        }
        "claimed" => "Your username and password are set. You can sign in with them from now on.",
        "recovered" => {
            "Your password was set with a recovery code. That code is spent; the rest \
                        of the set still works."
        }
        _ => "Done.",
    }
}

/// The message shown for something that just failed.
///
/// Delegates to the sign-in page's table for everything the two pages share,
/// and differs where the same error means something else here: on this page
/// [`AuthError::InvalidCredentials`] can only have come from the current
/// password field, and "that username and password do not match an account" is
/// the wrong sentence to read beside it.
fn account_error_message(code: &str) -> &'static str {
    match code {
        "credentials" => "That is not your current password.",
        "password" => "That new password is too short.",
        other => login_error_message(other),
    }
}

/// Render what an account can do about its own credential.
///
/// Empty when internal authentication is off: every form here posts to an
/// endpoint that would refuse, and offering a control that cannot work is worse
/// than offering none.
///
/// Which form appears is decided by whether the account already holds a
/// credential, because the two are different acts. Replacing a password takes
/// the current one; attaching a first one takes a username, and cannot
/// overwrite anything.
pub fn render_account_forms(
    internal: &crate::auth::config::InternalAuthConfig,
    csrf_token: &str,
    username: Option<&str>,
    provider: &str,
    recovery_codes_left: Option<i64>,
) -> String {
    if !internal.enabled {
        return String::new();
    }

    let min_password = internal
        .min_password_length
        .max(crate::auth::local::MIN_PASSWORD_LENGTH);

    match username {
        Some(username) => format!(
            r#"<form class="credentials" method="post" action="/auth/local/password">
                <h2>Change your password</h2>
                <input type="hidden" name="csrf_token" value="{csrf}">
                <input type="hidden" name="redirect" value="/auth/account?notice=password">
                <input type="text" name="username" value="{username}" autocomplete="username"
                       readonly hidden>
                <label for="current_password">Current password</label>
                <input id="current_password" name="current_password" type="password" required
                       autocomplete="current-password">
                <label for="new_password">New password</label>
                <input id="new_password" name="new_password" type="password" required
                       autocomplete="new-password" minlength="{min_password}">
                <button type="submit">Change password</button>
                <p class="switch">Changing it signs out every other session this account has.</p>
            </form>{recovery}"#,
            csrf = html_attribute(csrf_token),
            username = html_attribute(username),
            min_password = min_password,
            recovery = render_recovery_codes_form(internal, csrf_token, recovery_codes_left),
        ),
        None => {
            let explain = if provider == crate::auth::local::GUEST_PROVIDER {
                "This account has no way to sign in again — close this browser and it is gone. \
                 A username and password keep it, along with everything it already has."
            } else {
                "This account signs in through a provider and holds no password here. \
                 Adding one is a way in that does not depend on that provider being reachable."
            };

            format!(
                r#"<form class="credentials" method="post" action="/auth/local/claim">
                <h2>Add a username and password</h2>
                <p class="explain">{explain}</p>
                <input type="hidden" name="csrf_token" value="{csrf}">
                <input type="hidden" name="redirect" value="/auth/account?notice=claimed">
                <label for="username">Username</label>
                <input id="username" name="username" type="text" required autocomplete="username"
                       minlength="3" maxlength="32" autocapitalize="none" spellcheck="false">
                <label for="password">Password</label>
                <input id="password" name="password" type="password" required
                       autocomplete="new-password" minlength="{min_password}">
                <button type="submit">Save</button>
            </form>"#,
                explain = explain,
                csrf = html_attribute(csrf_token),
                min_password = min_password,
            )
        }
    }
}

/// The recovery-codes block on the account page.
///
/// Empty unless the engine offers codes at all. `None` means the account cannot
/// hold them — it has no password for a code to reset — and the caller has
/// already decided that; what is left here is the count, which is the only
/// thing that can honestly be reported about a set of codes the engine stores
/// as hashes.
///
/// It asks for the current password, and it says out loud that generating
/// replaces the set that exists. Both are the same point: a person should be
/// able to take away codes that were seen by somebody, and a stolen session
/// should not be able to mint codes that outlive the owner's next password
/// change.
fn render_recovery_codes_form(
    internal: &crate::auth::config::InternalAuthConfig,
    csrf_token: &str,
    codes_left: Option<i64>,
) -> String {
    // Checked here as well as by the caller, so this cannot render a control
    // for an endpoint the configuration would refuse however it is called.
    if !internal.allow_recovery_codes {
        return String::new();
    }

    let Some(codes_left) = codes_left else {
        return String::new();
    };

    let standing = match codes_left {
        0 => "You have no recovery codes. Without one, a forgotten password takes whoever runs \
              this engine to reset."
            .to_string(),
        1 => "You have 1 unused recovery code left.".to_string(),
        many => format!("You have {} unused recovery codes.", many),
    };

    format!(
        r#"<form class="credentials" method="post" action="/auth/local/recovery_codes">
                <h2>Recovery codes</h2>
                <p class="explain">{standing} Each one can set a new password once, if you forget
                it. Generating a set replaces whatever you have now.</p>
                <input type="hidden" name="csrf_token" value="{csrf}">
                <input type="hidden" name="redirect" value="/auth/account">
                <label for="recovery_current_password">Current password</label>
                <input id="recovery_current_password" name="current_password" type="password"
                       required autocomplete="current-password">
                <button type="submit">Generate new codes</button>
            </form>"#,
        standing = standing,
        csrf = html_attribute(csrf_token),
    )
}

/// The account page: what the signed-in person can do about their own way in.
///
/// The sign-in page cannot hold this. Everything here needs a session, and
/// changing a password needs the current one — neither is something a person
/// looking at a sign-in page has. What the sign-in page gets is a link.
///
/// Signed out, this redirects to the sign-in page and comes back, so the link
/// works for someone whose session has aged out.
#[utoipa::path(
    get,
    path = "/auth/account",
    tags = ["Authentication"],
    responses(
        (status = 200, description = "Account page HTML", content_type = "text/html"),
        (status = 302, description = "No session; redirected to the sign-in page"),
    )
)]
pub async fn account_page(
    State(auth_manager): State<Arc<AuthManager>>,
    Query(params): Query<AccountPageParams>,
    headers: HeaderMap,
) -> Response {
    let config = auth_manager.config();
    let ip_addr = client_ip::from_headers(&headers);
    let user_agent = client_ip::user_agent_from_headers(&headers);
    let host = get_request_host(&headers);

    let session = match session_token_from_headers(&headers, &config.session_cookie_name) {
        Some(token) => auth_manager
            .get_session(&token, &ip_addr, &user_agent, host.as_deref())
            .await
            .ok(),
        None => None,
    };

    let Some(session) = session else {
        return Redirect::to(&format!(
            "/auth/login?redirect={}",
            urlencoding::encode(ACCOUNT_PATH)
        ))
        .into_response();
    };

    // Whether there is a credential decides which form the page offers, so a
    // lookup that failed must not be read as "there is none" — that would show
    // someone with a password the form for setting a first one.
    let username = match crate::auth::local::username_for_user(&session.user_id).await {
        Ok(username) => username,
        Err(e) => {
            tracing::error!("Could not read the credential for an account page: {}", e);
            return (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error").into_response();
        }
    };

    let nonce = crate::security::generate_nonce();
    // Bound to the user, so a token minted for anybody else — including one an
    // attacker fetched from their own server, with no browser and no account —
    // cannot be posted back as this person's password change.
    let csrf_token = auth_manager
        .security_context()
        .csrf
        .generate_token(Some(session.user_id.clone()))
        .await
        .token;

    let label = username
        .clone()
        .or_else(|| session.email.clone())
        .or_else(|| session.name.clone())
        .unwrap_or_else(|| session.user_id.clone());

    let notice_block = match (params.error.as_deref(), params.notice.as_deref()) {
        (Some(code), _) => format!(
            r#"<div class="notice">{}</div>"#,
            account_error_message(code)
        ),
        (None, Some(code)) => format!(
            r#"<div class="notice ok">{}</div>"#,
            account_notice_message(code)
        ),
        (None, None) => String::new(),
    };

    // Only for an account that holds a password, since setting one is all a
    // code can do. A failed count is reported as no codes rather than as no
    // feature: the form is still the way to get some.
    let recovery_codes_left = if config.internal.allow_recovery_codes && username.is_some() {
        match crate::auth::local::unused_recovery_code_count(&session.user_id).await {
            Ok(count) => Some(count),
            Err(e) => {
                tracing::error!("Could not count recovery codes for an account page: {}", e);
                Some(0)
            }
        }
    } else {
        None
    };

    let forms = render_account_forms(
        &config.internal,
        &csrf_token,
        username.as_deref(),
        &session.provider,
        recovery_codes_left,
    );
    let forms = if forms.is_empty() {
        r#"<p class="explain">This engine holds no credentials of its own, so there is nothing to change here.</p>"#
            .to_string()
    } else {
        forms
    };

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Your account</title>
    <link rel="icon" type="image/x-icon" href="/favicon.ico">
    <style nonce="{style_nonce}">{styles}    </style>
</head>
<body>
    <div class="card">
        <h1>Your account</h1>
        {notice_block}
        <p class="identity">Signed in as <strong>{label}</strong>
            <span class="provider">via {provider}</span></p>
        {forms}
        <p class="switch"><a href="/auth/logout">Sign out</a></p>
    </div>
</body>
</html>"#,
        style_nonce = html_attribute(&nonce),
        styles = AUTH_PAGE_STYLES,
        notice_block = notice_block,
        label = html_escape::encode_text(&label),
        provider = html_escape::encode_text(&session.provider),
        forms = forms,
    );

    let mut response = html_page_response(html, &nonce);
    // The page names the account it belongs to. A shared cache holding it would
    // hand one person's to the next.
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}

/// Start OAuth2 login flow - redirects to provider
#[utoipa::path(
    get,
    path = "/auth/login/{provider}",
    tags = ["Authentication"],
    params(
        ("provider" = String, Path, description = "OAuth provider name (google, microsoft, apple)"),
        ("redirect" = Option<String>, Query, description = "Redirect URL after successful login")
    ),
    responses(
        (status = 302, description = "Redirect to OAuth provider for authentication"),
        (status = 400, description = "Invalid request", body = crate::openapi_schemas::ErrorResponse),
    )
)]
pub async fn start_login(
    State(auth_manager): State<Arc<AuthManager>>,
    Path(provider): Path<String>,
    Query(params): Query<LoginParams>,
    headers: HeaderMap,
) -> Result<Redirect, ErrorResponse> {
    let ip_addr = client_ip::from_headers(&headers);
    // Selects the redirect URI, so the flow returns to the host it began on
    // and sets its session cookie there.
    let host = get_request_host(&headers);

    // Generate authorization URL with or without redirect
    let (auth_url, _state) = if let Some(ref redirect_url) = params.redirect {
        let redirect_url = safe_redirect_target(Some(redirect_url));
        tracing::info!("Starting login with redirect URL: {}", redirect_url);
        auth_manager
            .start_login_with_redirect(&provider, &ip_addr, redirect_url, host.as_deref())
            .await
            .map_err(|e| ErrorResponse {
                error: "login_failed".to_string(),
                message: e.to_string(),
            })?
    } else {
        tracing::info!("Starting login without redirect URL");
        auth_manager
            .start_login(&provider, &ip_addr, host.as_deref())
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
#[utoipa::path(
    get,
    path = "/auth/callback/{provider}",
    tags = ["Authentication"],
    params(
        ("provider" = String, Path, description = "OAuth provider name (google, microsoft, apple)"),
        ("code" = Option<String>, Query, description = "Authorization code from provider"),
        ("state" = Option<String>, Query, description = "CSRF state token"),
        ("error" = Option<String>, Query, description = "Error from provider"),
        ("error_description" = Option<String>, Query, description = "Error description from provider")
    ),
    responses(
        (status = 302, description = "Redirect to original requested page with session cookie set"),
        (status = 400, description = "Invalid callback parameters", body = crate::openapi_schemas::ErrorResponse),
    )
)]
pub async fn oauth_callback(
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

    let ip_addr = client_ip::from_headers(&headers);
    let user_agent = client_ip::user_agent_from_headers(&headers);
    // The callback necessarily lands on the redirect URI's host, so this
    // reselects the provider instance the authorization request used and the
    // token exchange repeats the matching redirect URI.
    let host = get_request_host(&headers);

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
        .handle_callback(
            &provider,
            &code,
            &state,
            &ip_addr,
            &user_agent,
            host.as_deref(),
        )
        .await
        .map_err(|e| ErrorResponse {
            error: "authentication_failed".to_string(),
            message: e.to_string(),
        })?;

    // Set session cookie — use absolute max age so the browser retains the
    // cookie for the full session lifetime (up to 30 days), not just one hour.
    let config = auth_manager.config();
    let cookie_value = format!(
        "{}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{}",
        config.session_cookie_name,
        session_token,
        config.max_session_age,
        if config.cookie_secure { "; Secure" } else { "" }
    );

    // Redirect to stored URL or default to home, keeping the user on the host
    // whose session cookie was just set
    let redirect_target = safe_redirect_target(redirect_url.as_deref());

    // Return redirect with cookie
    let response = Redirect::to(&redirect_target).into_response();
    let (mut parts, body) = response.into_parts();
    let cookie_header = cookie_value.parse().map_err(|_| ErrorResponse {
        error: "internal_error".to_string(),
        message: "Invalid cookie header value".to_string(),
    })?;
    parts.headers.insert(header::SET_COOKIE, cookie_header);

    Ok(Response::from_parts(parts, body))
}

/// Request body for `POST /auth/guest`.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
pub struct GuestRequest {
    /// What to call this guest. Not a credential and not unique — a label.
    #[serde(default)]
    pub name: Option<String>,
    /// Where to send the browser afterwards. Form submissions only.
    #[serde(default)]
    pub redirect: Option<String>,
    /// CSRF token from the login page. Required of form submissions, and of
    /// nothing else — see [`RequestStyle`].
    #[serde(default)]
    pub csrf_token: Option<String>,
}

/// Request body for `POST /auth/local/register` and `/auth/local/login`.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
pub struct LocalCredentialRequest {
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    /// Display name, for registration only.
    #[serde(default)]
    pub name: Option<String>,
    /// Where to send the browser afterwards. Form submissions only.
    #[serde(default)]
    pub redirect: Option<String>,
    /// CSRF token from the login page. Required of form submissions, and of
    /// nothing else — see [`RequestStyle`].
    #[serde(default)]
    pub csrf_token: Option<String>,
}

/// Request body for `POST /auth/local/password`.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
pub struct PasswordChangeRequest {
    /// The password the account has now. Required even though the caller
    /// already holds a session: a session someone else got hold of must not be
    /// enough to lock the owner out of their own account.
    #[serde(default)]
    pub current_password: String,
    #[serde(default)]
    pub new_password: String,
    /// Where to send the browser afterwards. Form submissions only.
    #[serde(default)]
    pub redirect: Option<String>,
    #[serde(default)]
    pub csrf_token: Option<String>,
}

/// Request body for `POST /auth/local/recovery_codes`.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
pub struct RecoveryCodesRequest {
    /// The password the account has now. A set of recovery codes is a second
    /// way in, and a session someone else got hold of must not be able to mint
    /// one.
    #[serde(default)]
    pub current_password: String,
    #[serde(default)]
    pub redirect: Option<String>,
    #[serde(default)]
    pub csrf_token: Option<String>,
}

/// The one and only copy of a freshly issued set.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct RecoveryCodesResponse {
    pub success: bool,
    /// Shown once. The engine keeps only hashes, so it cannot show them again.
    pub codes: Vec<String>,
}

/// Request body for `POST /auth/local/recover`.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
pub struct RecoverRequest {
    #[serde(default)]
    pub username: String,
    /// One recovery code, in whatever spelling it was written down in.
    #[serde(default)]
    pub code: String,
    #[serde(default)]
    pub new_password: String,
    #[serde(default)]
    pub redirect: Option<String>,
    #[serde(default)]
    pub csrf_token: Option<String>,
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
enum RequestStyle {
    Json,
    Form,
}

/// Read a request body as JSON or as an HTML form, reporting which it was.
///
/// Mirrors what the engine API does for role changes: two short scalar fields
/// are worth accepting in either shape rather than making callers guess.
fn parse_auth_body<T: serde::de::DeserializeOwned + Default>(
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
async fn require_form_csrf(
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
async fn require_session_form_csrf(
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
fn error_code_for(error: &crate::auth::error::AuthError) -> &'static str {
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
        _ => "failed",
    }
}

/// The answer a browser gets: back to the login page, carrying a code.
fn redirect_to_login_with_error(
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
fn redirect_to_login_form_with_error(
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
fn redirect_to_form_with_error(
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
fn respond_to_style(
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
fn respond_with_session(
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

/// Issue a guest identity and a session.
#[utoipa::path(
    post,
    path = "/auth/guest",
    tags = ["Authentication"],
    request_body = GuestRequest,
    responses(
        (status = 200, description = "Guest session issued", body = InternalAuthResponse),
        (status = 403, description = "Guest accounts are not enabled", body = crate::openapi_schemas::ErrorResponse),
        (status = 429, description = "Rate limit exceeded", body = crate::openapi_schemas::ErrorResponse),
    )
)]
pub async fn start_guest(
    State(auth_manager): State<Arc<AuthManager>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, AuthErrorResponse> {
    let (request, style) = parse_auth_body::<GuestRequest>(&headers, &body);
    let ip_addr = client_ip::from_headers(&headers);
    let user_agent = client_ip::user_agent_from_headers(&headers);
    let host = get_request_host(&headers);

    require_form_csrf(&auth_manager, style, request.csrf_token.as_deref()).await?;

    let token = match auth_manager
        .start_guest_session(request.name, &ip_addr, &user_agent, host.as_deref())
        .await
    {
        Ok(token) => token,
        Err(error) if style == RequestStyle::Form => {
            return Ok(redirect_to_login_with_error(
                &error,
                request.redirect.as_deref(),
            ));
        }
        Err(error) => return Err(error.into()),
    };

    let user_id = auth_manager
        .get_session(&token, &ip_addr, &user_agent, host.as_deref())
        .await
        .ok()
        .map(|session| session.user_id);

    respond_to_style(
        &auth_manager,
        style,
        &token,
        request.redirect.as_deref(),
        InternalAuthResponse {
            success: true,
            user_id,
            username: None,
        },
    )
}

/// Create an account with a username and password held by this engine.
#[utoipa::path(
    post,
    path = "/auth/local/register",
    tags = ["Authentication"],
    request_body = LocalCredentialRequest,
    responses(
        (status = 200, description = "Account created and session issued", body = InternalAuthResponse),
        (status = 400, description = "Username or password rejected", body = crate::openapi_schemas::ErrorResponse),
        (status = 403, description = "Registration is not enabled", body = crate::openapi_schemas::ErrorResponse),
        (status = 409, description = "Username is taken", body = crate::openapi_schemas::ErrorResponse),
    )
)]
pub async fn register_local(
    State(auth_manager): State<Arc<AuthManager>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, AuthErrorResponse> {
    let (request, style) = parse_auth_body::<LocalCredentialRequest>(&headers, &body);
    let ip_addr = client_ip::from_headers(&headers);
    let user_agent = client_ip::user_agent_from_headers(&headers);
    let host = get_request_host(&headers);

    require_form_csrf(&auth_manager, style, request.csrf_token.as_deref()).await?;

    let token = match auth_manager
        .register_local_account(
            &request.username,
            &request.password,
            request.name.clone(),
            &ip_addr,
            &user_agent,
            host.as_deref(),
        )
        .await
    {
        Ok(token) => token,
        Err(error) if style == RequestStyle::Form => {
            return Ok(redirect_to_login_with_error(
                &error,
                request.redirect.as_deref(),
            ));
        }
        Err(error) => return Err(error.into()),
    };

    let user_id = auth_manager
        .get_session(&token, &ip_addr, &user_agent, host.as_deref())
        .await
        .ok()
        .map(|session| session.user_id);

    respond_to_style(
        &auth_manager,
        style,
        &token,
        request.redirect.as_deref(),
        InternalAuthResponse {
            success: true,
            user_id,
            username: Some(crate::auth::local::normalize_username(&request.username)),
        },
    )
}

/// Sign in against a credential held by this engine.
#[utoipa::path(
    post,
    path = "/auth/local/login",
    tags = ["Authentication"],
    request_body = LocalCredentialRequest,
    responses(
        (status = 200, description = "Session issued", body = InternalAuthResponse),
        (status = 401, description = "Invalid username or password", body = crate::openapi_schemas::ErrorResponse),
        (status = 403, description = "Internal authentication is not enabled", body = crate::openapi_schemas::ErrorResponse),
        (status = 429, description = "Rate limit exceeded", body = crate::openapi_schemas::ErrorResponse),
    )
)]
pub async fn login_local(
    State(auth_manager): State<Arc<AuthManager>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, AuthErrorResponse> {
    let (request, style) = parse_auth_body::<LocalCredentialRequest>(&headers, &body);
    let ip_addr = client_ip::from_headers(&headers);
    let user_agent = client_ip::user_agent_from_headers(&headers);
    let host = get_request_host(&headers);

    require_form_csrf(&auth_manager, style, request.csrf_token.as_deref()).await?;

    let token = match auth_manager
        .login_local(&request.username, &request.password, &ip_addr, &user_agent)
        .await
    {
        Ok(token) => token,
        Err(error) if style == RequestStyle::Form => {
            return Ok(redirect_to_login_with_error(
                &error,
                request.redirect.as_deref(),
            ));
        }
        Err(error) => return Err(error.into()),
    };

    let user_id = auth_manager
        .get_session(&token, &ip_addr, &user_agent, host.as_deref())
        .await
        .ok()
        .map(|session| session.user_id);

    respond_to_style(
        &auth_manager,
        style,
        &token,
        request.redirect.as_deref(),
        InternalAuthResponse {
            success: true,
            user_id,
            username: Some(crate::auth::local::normalize_username(&request.username)),
        },
    )
}

/// Give the account behind the current session a way to sign in again.
///
/// The account is identified by the session, never by the request body: this
/// endpoint attaches a credential to whoever is calling, so accepting a
/// `user_id` from the caller would let anyone claim anyone's account.
///
/// POST-only, and the session cookie is `SameSite=Lax`, so a cross-site
/// request arrives without a session and is refused. See
/// [`session_cookie_value`] — that is the reason this endpoint does not need a
/// CSRF token of its own, and the reason it must stay a POST.
#[utoipa::path(
    post,
    path = "/auth/local/claim",
    tags = ["Authentication"],
    request_body = LocalCredentialRequest,
    responses(
        (status = 200, description = "Credential attached to the current account", body = InternalAuthResponse),
        (status = 400, description = "Username or password rejected", body = crate::openapi_schemas::ErrorResponse),
        (status = 401, description = "No session to claim", body = crate::openapi_schemas::ErrorResponse),
        (status = 409, description = "Username taken, or the account already has a credential", body = crate::openapi_schemas::ErrorResponse),
    )
)]
pub async fn claim_account(
    State(auth_manager): State<Arc<AuthManager>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, AuthErrorResponse> {
    let (request, style) = parse_auth_body::<LocalCredentialRequest>(&headers, &body);
    let ip_addr = client_ip::from_headers(&headers);
    let user_agent = client_ip::user_agent_from_headers(&headers);
    let config = auth_manager.config();

    // The session is read before the token is checked, because the token is
    // checked against it: a token bound to this account is what the account
    // page hands out, and one bound to somebody else must not pass.
    let token = session_token_from_headers(&headers, &config.session_cookie_name)
        .ok_or(crate::auth::error::AuthError::AuthenticationRequired)?;
    let session = auth_manager
        .get_session(
            &token,
            &ip_addr,
            &user_agent,
            get_request_host(&headers).as_deref(),
        )
        .await
        .map_err(|_| crate::auth::error::AuthError::AuthenticationRequired)?;

    if require_session_form_csrf(
        &auth_manager,
        style,
        request.csrf_token.as_deref(),
        &session.user_id,
        false,
    )
    .await
    .is_err()
    {
        // Only a form submission can fail this, and a browser gets a page
        // rather than JSON: a token that timed out because the page sat open is
        // the ordinary outcome of leaving it open, and the page it goes back to
        // carries a fresh one.
        return Ok(redirect_to_form_with_error(
            &crate::auth::error::AuthError::CsrfValidationFailed,
            request.redirect.as_deref(),
        ));
    }

    let username = match auth_manager
        .claim_guest_account(
            &session.user_id,
            &request.username,
            &request.password,
            &ip_addr,
        )
        .await
    {
        Ok(username) => username,
        Err(error) if style == RequestStyle::Form => {
            return Ok(redirect_to_form_with_error(
                &error,
                request.redirect.as_deref(),
            ));
        }
        Err(error) => return Err(error.into()),
    };

    // The session already identifies this user and its roles have not changed,
    // so it stays as it is; only the way back in is new.
    if style == RequestStyle::Form {
        return Ok(
            Redirect::to(&safe_redirect_target(request.redirect.as_deref())).into_response(),
        );
    }

    Ok(Json(InternalAuthResponse {
        success: true,
        user_id: Some(session.user_id),
        username: Some(username),
    })
    .into_response())
}

/// Change the password of the account behind the current session.
///
/// The account comes from the session and the authorization comes from the
/// current password: one without the other is not enough, which is what keeps
/// both a stolen session and a guessed password from being a takeover.
///
/// Every other session the account had ends here, and the caller is given a
/// fresh one — so a password changed because it may be known does not leave
/// sessions minted under it running for another thirty days.
#[utoipa::path(
    post,
    path = "/auth/local/password",
    tags = ["Authentication"],
    request_body = PasswordChangeRequest,
    responses(
        (status = 200, description = "Password changed and a new session issued", body = InternalAuthResponse),
        (status = 400, description = "New password rejected", body = crate::openapi_schemas::ErrorResponse),
        (status = 401, description = "No session, or the current password is wrong", body = crate::openapi_schemas::ErrorResponse),
    )
)]
pub async fn change_password_route(
    State(auth_manager): State<Arc<AuthManager>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, AuthErrorResponse> {
    let (request, style) = parse_auth_body::<PasswordChangeRequest>(&headers, &body);
    let ip_addr = client_ip::from_headers(&headers);
    let user_agent = client_ip::user_agent_from_headers(&headers);
    let config = auth_manager.config();

    // Session first, then a token issued to that session. The only page that
    // submits this form is the account page, which mints a bound token, so
    // nothing here ever depended on an unbound one being accepted.
    let token = session_token_from_headers(&headers, &config.session_cookie_name)
        .ok_or(crate::auth::error::AuthError::AuthenticationRequired)?;
    let session = auth_manager
        .get_session(
            &token,
            &ip_addr,
            &user_agent,
            get_request_host(&headers).as_deref(),
        )
        .await
        .map_err(|_| crate::auth::error::AuthError::AuthenticationRequired)?;

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
        // As in [`claim_account`]: a form gets the page back, carrying a code
        // the page renders as "that form expired".
        return Ok(redirect_to_form_with_error(
            &crate::auth::error::AuthError::CsrfValidationFailed,
            request.redirect.as_deref(),
        ));
    }

    let new_token = match auth_manager
        .change_local_password(
            &session.user_id,
            &request.current_password,
            &request.new_password,
            &ip_addr,
            &user_agent,
        )
        .await
    {
        Ok(token) => token,
        Err(error) if style == RequestStyle::Form => {
            return Ok(redirect_to_form_with_error(
                &error,
                request.redirect.as_deref(),
            ));
        }
        Err(error) => return Err(error.into()),
    };

    respond_to_style(
        &auth_manager,
        style,
        &new_token,
        request.redirect.as_deref(),
        InternalAuthResponse {
            success: true,
            user_id: Some(session.user_id),
            username: None,
        },
    )
}

/// Issue a fresh set of recovery codes for the account behind the session.
///
/// Answers with the codes themselves, which is the only time they exist: what
/// is stored is a hash of each, so this response cannot be reproduced. A form
/// submission therefore gets a page rather than a redirect — there is nowhere
/// to redirect to that could carry them, and putting a credential in a URL is
/// the one place it must not go.
#[utoipa::path(
    post,
    path = "/auth/local/recovery_codes",
    tags = ["Authentication"],
    request_body = RecoveryCodesRequest,
    responses(
        (status = 200, description = "A new set of codes, shown once", body = RecoveryCodesResponse),
        (status = 401, description = "No session, or the current password is wrong", body = crate::openapi_schemas::ErrorResponse),
        (status = 403, description = "Recovery codes are not enabled", body = crate::openapi_schemas::ErrorResponse),
    )
)]
pub async fn recovery_codes_route(
    State(auth_manager): State<Arc<AuthManager>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, AuthErrorResponse> {
    let (request, style) = parse_auth_body::<RecoveryCodesRequest>(&headers, &body);
    let ip_addr = client_ip::from_headers(&headers);
    let user_agent = client_ip::user_agent_from_headers(&headers);
    let config = auth_manager.config();

    let token = session_token_from_headers(&headers, &config.session_cookie_name)
        .ok_or(crate::auth::error::AuthError::AuthenticationRequired)?;
    let session = auth_manager
        .get_session(
            &token,
            &ip_addr,
            &user_agent,
            get_request_host(&headers).as_deref(),
        )
        .await
        .map_err(|_| crate::auth::error::AuthError::AuthenticationRequired)?;

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
        return Ok(redirect_to_form_with_error(
            &crate::auth::error::AuthError::CsrfValidationFailed,
            request.redirect.as_deref(),
        ));
    }

    let codes = match auth_manager
        .issue_recovery_codes(&session.user_id, &request.current_password, &ip_addr)
        .await
    {
        Ok(codes) => codes,
        Err(error) if style == RequestStyle::Form => {
            return Ok(redirect_to_form_with_error(
                &error,
                request.redirect.as_deref(),
            ));
        }
        Err(error) => return Err(error.into()),
    };

    if style == RequestStyle::Form {
        return Ok(render_recovery_codes_page(&codes));
    }

    Ok(Json(RecoveryCodesResponse {
        success: true,
        codes,
    })
    .into_response())
}

/// The page that shows a freshly issued set, once.
fn render_recovery_codes_page(codes: &[String]) -> Response {
    let nonce = crate::security::generate_nonce();
    let items = codes
        .iter()
        .map(|code| format!("<li>{}</li>", html_escape::encode_text(code)))
        .collect::<Vec<_>>()
        .join("\n            ");

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Recovery codes</title>
    <link rel="icon" type="image/x-icon" href="/favicon.ico">
    <style nonce="{style_nonce}">{styles}    </style>
</head>
<body>
    <div class="card">
        <h1>Recovery codes</h1>
        <div class="notice ok">These replace any codes you had before.</div>
        <p class="explain">Write them down somewhere that is not this computer. Each one can set a
        new password once, and they are shown here and nowhere else — the engine keeps only a hash,
        so it cannot show them to you again.</p>
        <ul class="codes">
            {items}
        </ul>
        <p class="switch"><a href="/auth/account">Back to your account</a></p>
    </div>
</body>
</html>"#,
        style_nonce = html_attribute(&nonce),
        styles = AUTH_PAGE_STYLES,
        items = items,
    );

    let mut response = html_page_response(html, &nonce);
    // The one response in the engine that carries credentials in its body.
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}

/// Spend a recovery code: set a new password and sign the caller in.
///
/// Takes no session — the whole point is that whoever is calling cannot get
/// one. What stands in for it is the code, which the account was issued ahead
/// of time and which is single-use.
#[utoipa::path(
    post,
    path = "/auth/local/recover",
    tags = ["Authentication"],
    request_body = RecoverRequest,
    responses(
        (status = 200, description = "Password reset and a session issued", body = InternalAuthResponse),
        (status = 400, description = "New password rejected", body = crate::openapi_schemas::ErrorResponse),
        (status = 401, description = "Unknown username, or a code that is wrong or already spent", body = crate::openapi_schemas::ErrorResponse),
        (status = 403, description = "Recovery codes are not enabled", body = crate::openapi_schemas::ErrorResponse),
        (status = 429, description = "Rate limit exceeded", body = crate::openapi_schemas::ErrorResponse),
    )
)]
pub async fn recover_account(
    State(auth_manager): State<Arc<AuthManager>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, AuthErrorResponse> {
    let (request, style) = parse_auth_body::<RecoverRequest>(&headers, &body);
    let ip_addr = client_ip::from_headers(&headers);
    let user_agent = client_ip::user_agent_from_headers(&headers);
    let host = get_request_host(&headers);

    require_form_csrf(&auth_manager, style, request.csrf_token.as_deref()).await?;

    let token = match auth_manager
        .recover_local_account(
            &request.username,
            &request.code,
            &request.new_password,
            &ip_addr,
            &user_agent,
        )
        .await
    {
        Ok(token) => token,
        Err(error) if style == RequestStyle::Form => {
            return Ok(redirect_to_login_form_with_error(
                &error,
                request.redirect.as_deref(),
                LoginForm::Recover,
            ));
        }
        Err(error) => return Err(error.into()),
    };

    let user_id = auth_manager
        .get_session(&token, &ip_addr, &user_agent, host.as_deref())
        .await
        .ok()
        .map(|session| session.user_id);

    respond_to_style(
        &auth_manager,
        style,
        &token,
        request.redirect.as_deref(),
        InternalAuthResponse {
            success: true,
            user_id,
            username: Some(crate::auth::local::normalize_username(&request.username)),
        },
    )
}

/// Logout handler - destroys session
#[utoipa::path(
    get,
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
        return Json(AuthResponse {
            success: true,
            user_id: Some(session.user_id),
            is_admin: Some(session.is_admin),
            is_editor: Some(session.is_editor),
            redirect: None,
        });
    }

    Json(AuthResponse {
        success: false,
        user_id: None,
        is_admin: None,
        is_editor: None,
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

/// Paths the OAuth 2.0 protocol endpoints are served at.
///
/// These are used to mount the routes, to build the return URL an
/// unauthenticated caller comes back to after logging in, and to advertise the
/// endpoints in the discovery documents, so those cannot drift apart — the
/// return URL did drift once, when the generic `/authorize` alias was
/// withdrawn in favour of the reserved `/auth` prefix.
pub(crate) const AUTHORIZE_PATH: &str = "/auth/oauth2/authorize";
pub(crate) const TOKEN_PATH: &str = "/auth/oauth2/token";
pub(crate) const REGISTRATION_PATH: &str = "/auth/oauth2/register";
/// Where the consent page posts its answer. Not advertised in the discovery
/// metadata: it is part of how this server renders the authorization endpoint,
/// not an endpoint a client ever calls.
pub(crate) const CONSENT_PATH: &str = "/auth/oauth2/consent";

/// Cap on a consent form's size. The body is a handful of short fields the
/// engine wrote into its own page, so anything larger is not a consent form.
const MAX_CONSENT_BODY_BYTES: usize = 16 * 1024;

/// OAuth 2.0 authorization request parameters (RFC 6749)
#[derive(Debug, Deserialize, utoipa::ToSchema)]
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

/// Rebuild the authorization request as a relative URL, for a caller who has
/// to log in first and should land back on the request they made.
///
/// The parameters are re-encoded from the parsed request rather than the raw
/// query string being passed through, so nothing the caller appended survives
/// into the URL the login flow will bounce them to.
fn authorize_return_url(params: &AuthorizeParams) -> String {
    let mut query_params = vec![
        format!(
            "response_type={}",
            urlencoding::encode(&params.response_type)
        ),
        format!("client_id={}", urlencoding::encode(&params.client_id)),
    ];

    let optional = [
        ("redirect_uri", &params.redirect_uri),
        ("scope", &params.scope),
        ("state", &params.state),
        ("code_challenge", &params.code_challenge),
        ("code_challenge_method", &params.code_challenge_method),
        ("resource", &params.resource),
    ];
    for (name, value) in optional {
        if let Some(value) = value
            && !value.is_empty()
        {
            query_params.push(format!("{}={}", name, urlencoding::encode(value)));
        }
    }

    format!("{}?{}", AUTHORIZE_PATH, query_params.join("&"))
}

/// Shortest and longest a PKCE code challenge may be (RFC 7636 §4.2). An S256
/// challenge is base64url of a SHA-256 digest, so it is always 43 characters;
/// the range is what the spec allows, not what we expect.
const MIN_CODE_CHALLENGE_LENGTH: usize = 43;
const MAX_CODE_CHALLENGE_LENGTH: usize = 128;

/// Whether a code challenge is shaped like one, before it is stored.
///
/// A malformed challenge would fail verification at the token endpoint anyway,
/// but failing here means the client learns it at the point it made the
/// mistake, rather than after a person has been asked to approve something.
fn code_challenge_is_wellformed(challenge: &str) -> bool {
    (MIN_CODE_CHALLENGE_LENGTH..=MAX_CODE_CHALLENGE_LENGTH).contains(&challenge.len())
        && challenge
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_' | '~'))
}

/// Whether this engine is willing to mint a token whose audience is `resource`.
///
/// RFC 8707 lets a client name what it wants a token to be good for, and the
/// token endpoint copies that name onto the session's audience verbatim. Two
/// rules, both needed:
///
/// - the resource must name a host this engine actually serves, or the audience
///   is just a string somebody typed; and
/// - it must name *the host the authorization is happening on*. A sign-in
///   completed on a solution's host must not hand out a credential for the
///   management host's `/mcp`, and realm scoping only stops that for accounts
///   whose realm is not `*`.
fn resource_is_acceptable(resource: &str, request_host: Option<&str>) -> bool {
    resource_is_acceptable_for(resource, request_host, crate::hosts::config())
}

/// The rule itself, against an explicit host configuration so it can be
/// exercised without the process-global one.
fn resource_is_acceptable_for(
    resource: &str,
    request_host: Option<&str>,
    hosts: &crate::hosts::HostConfig,
) -> bool {
    let normalized = crate::security::session::normalize_resource(resource);
    let authority = normalized.split('/').next().unwrap_or_default();
    if authority.is_empty() {
        return false;
    }

    // Before startup configures hosts there is nothing to check against, and a
    // deployment with no base URL set would otherwise be unable to issue any
    // token at all.
    if hosts.all_hosts().is_empty() {
        return true;
    }

    hosts.is_configured(authority) && hosts.canonical_host(request_host) == authority
}

/// The audience to mint for a `resource` a client asked for.
///
/// Reduced to the form audiences are compared in, and pinned to an endpoint. A
/// client discovers this engine through its protected-resource document and
/// asks for what that document names; several ask for the origin instead —
/// `https://example.com/`, the whole site. An audience is matched on host *and*
/// path, so a token carrying an origin authorizes nothing at all: it would be
/// minted, handed over, and refused at the only endpoint it exists for.
///
/// Naming the MCP endpoint instead narrows the token rather than widening it.
/// `/mcp` is the only path a bearer token is audience-checked on, so this is
/// the same reach an origin-wide audience would have had if one were honoured,
/// and it keeps the host binding that separates two `/mcp` endpoints of the
/// same engine.
fn resource_audience(resource: &str) -> String {
    let normalized = crate::security::session::normalize_resource(resource);

    if normalized.contains('/') {
        normalized
    } else {
        format!(
            "{}{}",
            normalized,
            crate::auth::mcp_middleware::MCP_ENDPOINT_PATH
        )
    }
}

/// An authorization request that survived validation.
///
/// Holding the looked-up client rather than the caller's `client_id` is the
/// point: everything downstream reads the registration, so there is no path
/// where an unregistered value is used by accident.
#[derive(Debug)]
struct ValidatedAuthorization {
    client: crate::auth::client_registration::RegisteredClient,
    redirect_uri: String,
    scope: Option<String>,
    state: Option<String>,
    code_challenge: String,
    resource: Option<String>,
}

/// Why an authorization request was refused, and where the answer goes.
#[derive(Debug)]
enum AuthorizeRejection {
    /// Refused before the redirect URI could be trusted, so the answer is shown
    /// to the browser. Redirecting an error to a URI that has not been matched
    /// against a registration is the hole itself — it is how an unregistered
    /// URI gets to hear from this endpoint at all.
    Direct {
        status: StatusCode,
        error: &'static str,
        description: String,
    },
    /// Refused after the client and its redirect URI checked out, so the error
    /// goes back to the client the way RFC 6749 §4.1.2.1 asks.
    Redirect {
        redirect_uri: String,
        state: Option<String>,
        error: &'static str,
        description: String,
    },
}

impl AuthorizeRejection {
    fn into_response(self) -> Response {
        match self {
            AuthorizeRejection::Direct {
                status,
                error,
                description,
            } => (
                status,
                Json(ErrorResponse {
                    error: error.to_string(),
                    message: description,
                }),
            )
                .into_response(),
            AuthorizeRejection::Redirect {
                redirect_uri,
                state,
                error,
                description,
            } => {
                let mut url = append_query_param(&redirect_uri, "error", error);
                url = append_query_param(&url, "error_description", &description);
                if let Some(state) = state {
                    url = append_query_param(&url, "state", &state);
                }
                redirect_to_client(&url)
            }
        }
    }
}

/// Append one query parameter to a URL that may or may not already have some.
fn append_query_param(url: &str, name: &str, value: &str) -> String {
    let separator = if url.contains('?') { '&' } else { '?' };
    format!(
        "{}{}{}={}",
        url,
        separator,
        name,
        urlencoding::encode(value)
    )
}

/// Send the browser on to a client's redirect URI.
///
/// A meta refresh plus a scripted assignment rather than a 302, because client
/// redirect URIs are routinely custom schemes (`vscode://`, `cursor://`) that
/// `Location` handling treats inconsistently. The URL is escaped for the two
/// contexts it appears in and the script runs under a per-response nonce.
fn redirect_to_client(target: &str) -> Response {
    let js_target = serde_json::to_string(target).unwrap_or_else(|_| "\"/\"".to_string());
    let nonce = crate::security::generate_nonce();
    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta http-equiv="refresh" content="0;url={}" />
    <title>Redirecting…</title>
</head>
<body>
    <p>Returning to the application. If nothing happens, <a href="{}">continue</a>.</p>
    <script nonce="{}">window.location.href = {};</script>
</body>
</html>"#,
        html_escape::encode_text(target),
        html_escape::encode_text(target),
        html_attribute(&nonce),
        js_target
    );

    html_page_response(html, &nonce)
}

/// Check an authorization request against the client registry and the rules.
///
/// This is the whole of the gate, and both the `GET` that shows a consent page
/// and the `POST` that acts on the answer run it — the second time because a
/// consent form is a caller-supplied body like any other, and re-deriving the
/// decision from it is cheaper than trusting it.
async fn validate_authorize_request(
    pool: &PgPool,
    params: &AuthorizeParams,
    request_host: Option<&str>,
) -> Result<ValidatedAuthorization, AuthorizeRejection> {
    let direct =
        |status: StatusCode, error: &'static str, description: &str| AuthorizeRejection::Direct {
            status,
            error,
            description: description.to_string(),
        };

    if params.client_id.trim().is_empty() {
        return Err(direct(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "Missing client_id parameter",
        ));
    }

    let client =
        match crate::auth::client_registration::lookup_client(pool, &params.client_id).await {
            Ok(Some(client)) => client,
            Ok(None) => {
                return Err(direct(
                    StatusCode::BAD_REQUEST,
                    "invalid_client",
                    "Unknown client_id. Register the client before requesting authorization.",
                ));
            }
            Err(e) => {
                tracing::error!("Client lookup failed: {}", e);
                return Err(direct(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "server_error",
                    "Could not read the client registry",
                ));
            }
        };

    let redirect_uri = params
        .redirect_uri
        .as_deref()
        .map(str::trim)
        .filter(|uri| !uri.is_empty());
    let Some(redirect_uri) = redirect_uri else {
        return Err(direct(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "redirect_uri is required",
        ));
    };

    if !client.redirect_uri_registered(redirect_uri) {
        return Err(direct(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "redirect_uri does not match a URI this client registered",
        ));
    }

    // Past this point the redirect URI is one the client itself registered, so
    // errors are the client's to handle and go back to it.
    let redirect = |error: &'static str, description: &str| AuthorizeRejection::Redirect {
        redirect_uri: redirect_uri.to_string(),
        state: params.state.clone(),
        error,
        description: description.to_string(),
    };

    if params.response_type != "code" {
        return Err(redirect(
            "unsupported_response_type",
            "Only the 'code' response type is supported",
        ));
    }

    if !client.allows_grant("authorization_code") {
        return Err(redirect(
            "unauthorized_client",
            "This client did not register the authorization_code grant",
        ));
    }

    // PKCE is required, not merely verified when it happens to be offered.
    // Checking a challenge only if one arrived means a caller who sends none is
    // never asked for a verifier, and an authorization code becomes usable by
    // whoever manages to read it.
    let code_challenge = params
        .code_challenge
        .as_deref()
        .map(str::trim)
        .filter(|challenge| !challenge.is_empty());
    let Some(code_challenge) = code_challenge else {
        return Err(redirect(
            "invalid_request",
            "code_challenge is required (PKCE, RFC 7636)",
        ));
    };

    // `plain` is accepted by RFC 7636 and is worth nothing here: the challenge
    // travels in the same query string as everything else, so a caller who can
    // read the request can also read the verifier.
    if params.code_challenge_method.as_deref() != Some("S256") {
        return Err(redirect(
            "invalid_request",
            "code_challenge_method must be S256",
        ));
    }

    if !code_challenge_is_wellformed(code_challenge) {
        return Err(redirect(
            "invalid_request",
            "code_challenge is not a well-formed S256 challenge",
        ));
    }

    let resource = params
        .resource
        .as_deref()
        .map(str::trim)
        .filter(|resource| !resource.is_empty());
    if let Some(resource) = resource
        && !resource_is_acceptable(resource, request_host)
    {
        return Err(redirect(
            "invalid_target",
            "resource must name this host's endpoint",
        ));
    }

    Ok(ValidatedAuthorization {
        client,
        redirect_uri: redirect_uri.to_string(),
        scope: params.scope.clone().filter(|s| !s.trim().is_empty()),
        state: params.state.clone(),
        code_challenge: code_challenge.to_string(),
        resource: resource.map(resource_audience),
    })
}

/// Whether every scope being asked for is one the stored grant already covers.
///
/// No scope requested is covered by anything, including a grant that named
/// none.
fn scope_is_covered(granted: Option<&str>, requested: Option<&str>) -> bool {
    let Some(requested) = requested else {
        return true;
    };
    let granted = granted.unwrap_or_default();
    requested
        .split_whitespace()
        .all(|wanted| granted.split_whitespace().any(|held| held == wanted))
}

/// Whether this user has already agreed to this client doing this.
///
/// A stored grant covers a new request only when it is at least as wide.
/// Anything else — a scope that was not approved last time, a different
/// resource — is a widening, and widening is the thing that must not happen
/// without being seen.
async fn consent_already_given(
    pool: &PgPool,
    user_id: &str,
    validated: &ValidatedAuthorization,
) -> Result<bool, sqlx::Error> {
    let row = sqlx::query(
        "SELECT scope, resource FROM oauth_client_grants WHERE user_id = $1 AND client_id = $2",
    )
    .bind(user_id)
    .bind(&validated.client.client_id)
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        return Ok(false);
    };

    let granted_scope: Option<String> = row.try_get("scope").unwrap_or(None);
    let granted_resource: Option<String> = row.try_get("resource").unwrap_or(None);

    Ok(
        scope_is_covered(granted_scope.as_deref(), validated.scope.as_deref())
            && granted_resource.as_deref() == validated.resource.as_deref(),
    )
}

/// Record what a person just approved, replacing whatever they approved before.
///
/// Stored as approved rather than merged with the previous grant: a client that
/// asks for less next time should be held to less, and a union would quietly
/// keep privileges nobody re-approved.
async fn record_consent(
    pool: &PgPool,
    user_id: &str,
    validated: &ValidatedAuthorization,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO oauth_client_grants (user_id, client_id, scope, resource, granted_at)
         VALUES ($1, $2, $3, $4, NOW())
         ON CONFLICT (user_id, client_id)
         DO UPDATE SET scope = EXCLUDED.scope, resource = EXCLUDED.resource, granted_at = NOW()",
    )
    .bind(user_id)
    .bind(&validated.client.client_id)
    .bind(&validated.scope)
    .bind(&validated.resource)
    .execute(pool)
    .await
    .map(|_| ())
}

/// Mint an authorization code and send the browser back to the client with it.
async fn issue_authorization_code(
    pool: &PgPool,
    user_id: &str,
    validated: &ValidatedAuthorization,
) -> Response {
    let auth_code = format!("code_{}", uuid::Uuid::new_v4());
    let expires_at = Utc::now() + chrono::Duration::minutes(10);

    let stored = sqlx::query(
        "INSERT INTO oauth_authorization_codes (code, user_id, client_id, redirect_uri, code_challenge, code_challenge_method, scope, resource, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)"
    )
    .bind(&auth_code)
    .bind(user_id)
    .bind(&validated.client.client_id)
    .bind(&validated.redirect_uri)
    .bind(&validated.code_challenge)
    .bind("S256")
    .bind(&validated.scope)
    .bind(&validated.resource)
    .bind(expires_at)
    .execute(pool)
    .await;

    if let Err(e) = stored {
        tracing::error!("Failed to store authorization code: {}", e);
        return AuthorizeRejection::Redirect {
            redirect_uri: validated.redirect_uri.clone(),
            state: validated.state.clone(),
            error: "server_error",
            description: "Failed to record the authorization".to_string(),
        }
        .into_response();
    }

    tracing::info!(
        "Issued authorization code to client {} for user {}",
        validated.client.client_id,
        user_id
    );

    let mut target = append_query_param(&validated.redirect_uri, "code", &auth_code);
    if let Some(state) = validated.state.as_deref() {
        target = append_query_param(&target, "state", state);
    }

    redirect_to_client(&target)
}

/// The page a person sees before a client is given a code in their name.
///
/// This is what actually stands between a cross-site navigation and an
/// authorization code. Registration is open — an attacker can register a client
/// as easily as anyone else — so validating the client and its redirect URI
/// narrows the attack without ending it. Someone saying yes, on a page that
/// names the client and shows where they will be sent, is what ends it.
///
/// The form carries the request back rather than the engine holding it: every
/// field is re-validated on the way in, so a tampered form buys nothing that
/// forging the original request would not have.
fn render_consent_page(
    validated: &ValidatedAuthorization,
    csrf_token: &str,
    user_label: &str,
) -> Response {
    let nonce = crate::security::generate_nonce();

    let scope_block = match validated.scope.as_deref() {
        Some(scope) => {
            let items = scope
                .split_whitespace()
                .map(|s| format!("<li><code>{}</code></li>", html_escape::encode_text(s)))
                .collect::<String>();
            format!(
                r#"<div class="detail"><span class="detail-label">Access requested</span><ul class="scopes">{}</ul></div>"#,
                items
            )
        }
        None => String::new(),
    };

    let resource_block = match validated.resource.as_deref() {
        Some(resource) => format!(
            r#"<div class="detail"><span class="detail-label">For</span><code>{}</code></div>"#,
            html_escape::encode_text(resource)
        ),
        None => String::new(),
    };

    let hidden = |name: &str, value: &str| {
        format!(
            r#"<input type="hidden" name="{}" value="{}">"#,
            name,
            html_attribute(value)
        )
    };

    let mut hidden_fields = String::new();
    hidden_fields.push_str(&hidden("csrf_token", csrf_token));
    hidden_fields.push_str(&hidden("response_type", "code"));
    hidden_fields.push_str(&hidden("client_id", &validated.client.client_id));
    hidden_fields.push_str(&hidden("redirect_uri", &validated.redirect_uri));
    hidden_fields.push_str(&hidden("code_challenge", &validated.code_challenge));
    hidden_fields.push_str(&hidden("code_challenge_method", "S256"));
    if let Some(scope) = validated.scope.as_deref() {
        hidden_fields.push_str(&hidden("scope", scope));
    }
    if let Some(state) = validated.state.as_deref() {
        hidden_fields.push_str(&hidden("state", state));
    }
    if let Some(resource) = validated.resource.as_deref() {
        hidden_fields.push_str(&hidden("resource", resource));
    }

    let html = format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>Authorize {client_title}</title>
    <link rel="icon" type="image/x-icon" href="/favicon.ico">
    <style nonce="{style_nonce}">
        body {{
            margin: 0;
            padding: 1rem;
            font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
            font-size: 14px;
            line-height: 1.5;
            color: #212529;
            background: #f8f9fa;
            min-height: 100vh;
            display: flex;
            align-items: center;
            justify-content: center;
        }}

        .card {{
            width: 100%;
            max-width: 440px;
            background: #ffffff;
            border: 1px solid #dee2e6;
            border-radius: 8px;
            box-shadow: 0 1px 3px rgba(0, 0, 0, 0.1), 0 1px 2px rgba(0, 0, 0, 0.06);
            padding: 2rem;
        }}

        h1 {{
            margin: 0 0 0.5rem 0;
            font-size: 1.4rem;
            text-align: center;
        }}

        .who {{
            margin: 0 0 1.5rem 0;
            color: #6c757d;
            text-align: center;
        }}

        .detail {{
            padding: 0.75rem 0;
            border-top: 1px solid #e9ecef;
        }}

        .detail-label {{
            display: block;
            font-size: 0.8rem;
            text-transform: uppercase;
            letter-spacing: 0.05em;
            color: #6c757d;
            margin-bottom: 0.25rem;
        }}

        code {{
            font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
            font-size: 0.85rem;
            word-break: break-all;
        }}

        .scopes {{
            margin: 0;
            padding-left: 1.25rem;
        }}

        .warning {{
            margin: 1rem 0 0;
            padding: 0.6rem 0.75rem;
            border: 1px solid #ffe08a;
            border-radius: 6px;
            background: #fff9e6;
            color: #664d03;
            font-size: 0.9rem;
        }}

        .actions {{
            display: flex;
            gap: 0.5rem;
            margin-top: 1.5rem;
        }}

        button {{
            flex: 1;
            padding: 0.75rem 1rem;
            border-radius: 6px;
            font-weight: 500;
            font-size: 1rem;
            cursor: pointer;
            border: 1px solid transparent;
        }}

        .allow {{
            background-color: #0d6efd;
            color: #ffffff;
        }}

        .allow:hover {{
            background-color: #0b5ed7;
        }}

        .deny {{
            background-color: #ffffff;
            color: #212529;
            border-color: #ced4da;
        }}

        .deny:hover {{
            background-color: #f1f3f5;
        }}

        button:focus-visible {{
            outline: 2px solid #0d6efd;
            outline-offset: 2px;
        }}
    </style>
</head>
<body>
    <div class="card">
        <h1>Authorize {client_title}</h1>
        <p class="who">Signed in as {user_label}</p>

        <div class="detail">
            <span class="detail-label">Application</span>
            <strong>{client_title}</strong>
        </div>
        <div class="detail">
            <span class="detail-label">Will be sent to</span>
            <code>{redirect_uri}</code>
        </div>
        {scope_block}
        {resource_block}

        <p class="warning">Anyone can register an application with this engine. Approve this only if you started it yourself and recognise where it sends you.</p>

        <form method="post" action="{consent_path}">
            {hidden_fields}
            <div class="actions">
                <button type="submit" class="deny" name="decision" value="deny">Cancel</button>
                <button type="submit" class="allow" name="decision" value="allow">Allow</button>
            </div>
        </form>
    </div>
</body>
</html>"#,
        client_title = html_escape::encode_text(validated.client.display_name()),
        user_label = html_escape::encode_text(user_label),
        redirect_uri = html_escape::encode_text(&validated.redirect_uri),
        style_nonce = html_attribute(&nonce),
        scope_block = scope_block,
        resource_block = resource_block,
        consent_path = CONSENT_PATH,
        hidden_fields = hidden_fields,
    );

    html_page_response(html, &nonce)
}

/// OAuth 2.0 authorization endpoint
///
/// Refuses anything it cannot account for: a `client_id` that was never
/// registered, a `redirect_uri` that client did not register, a request with no
/// PKCE challenge, and a `resource` naming somewhere this host does not serve.
/// What survives that is shown to the person it would act for, and only their
/// approval produces a code.
#[utoipa::path(
    get,
    path = "/auth/oauth2/authorize",
    tags = ["Authentication"],
    params(
        ("response_type" = String, Query, description = "Must be 'code' for authorization code flow"),
        ("client_id" = String, Query, description = "Identifier of a registered client"),
        ("redirect_uri" = String, Query, description = "Must exactly match a URI the client registered"),
        ("scope" = Option<String>, Query, description = "Requested scope"),
        ("state" = Option<String>, Query, description = "Opaque value returned with the code"),
        ("code_challenge" = String, Query, description = "PKCE code challenge (RFC 7636); required"),
        ("code_challenge_method" = String, Query, description = "Must be S256"),
        ("resource" = Option<String>, Query, description = "Resource indicator (RFC 8707); must name this host")
    ),
    responses(
        (status = 200, description = "Consent page, or an HTML redirect back to the client", content_type = "text/html"),
        (status = 302, description = "Redirect to login if not authenticated"),
        (status = 400, description = "Unknown client, unregistered redirect URI, or invalid request", body = crate::openapi_schemas::ErrorResponse),
    )
)]
pub async fn oauth2_authorize(
    State(oauth2_state): State<OAuth2State>,
    Query(params): Query<AuthorizeParams>,
    req: axum::extract::Request,
) -> Response {
    let host = get_request_host(req.headers());

    // Validated before authentication is considered, so a request that would be
    // refused anyway does not first cost someone a sign-in.
    let validated =
        match validate_authorize_request(&oauth2_state.pool, &params, host.as_deref()).await {
            Ok(validated) => validated,
            Err(rejection) => return rejection.into_response(),
        };

    let Some(auth_user) = req.extensions().get::<crate::auth::AuthUser>().cloned() else {
        let return_url = authorize_return_url(&params);
        return Redirect::to(&format!(
            "/auth/login?redirect={}",
            urlencoding::encode(&return_url)
        ))
        .into_response();
    };

    match consent_already_given(&oauth2_state.pool, &auth_user.user_id, &validated).await {
        Ok(true) => {}
        Ok(false) => {
            // Bound to the user, so a token minted for anyone else — including
            // one an attacker fetched from their own server — cannot be posted
            // back as this person's approval.
            let csrf_token = oauth2_state
                .auth_manager
                .security_context()
                .csrf
                .generate_token(Some(auth_user.user_id.clone()))
                .await
                .token;
            let label = user_label_for(&auth_user);
            return render_consent_page(&validated, &csrf_token, &label);
        }
        Err(e) => {
            tracing::error!("Could not read stored consent: {}", e);
            return AuthorizeRejection::Redirect {
                redirect_uri: validated.redirect_uri.clone(),
                state: validated.state.clone(),
                error: "server_error",
                description: "Could not read stored consent".to_string(),
            }
            .into_response();
        }
    }

    issue_authorization_code(&oauth2_state.pool, &auth_user.user_id, &validated).await
}

/// How to name the signed-in person on the consent page.
fn user_label_for(auth_user: &crate::auth::AuthUser) -> String {
    auth_user
        .email
        .clone()
        .or_else(|| auth_user.name.clone())
        .unwrap_or_else(|| auth_user.user_id.clone())
}

/// The consent form's fields: the authorization request, plus the answer.
#[derive(Debug, Default, Deserialize)]
pub struct ConsentForm {
    #[serde(default)]
    csrf_token: String,
    #[serde(default)]
    decision: String,
    #[serde(default)]
    response_type: String,
    #[serde(default)]
    client_id: String,
    #[serde(default)]
    redirect_uri: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    code_challenge: Option<String>,
    #[serde(default)]
    code_challenge_method: Option<String>,
    #[serde(default)]
    resource: Option<String>,
}

impl ConsentForm {
    /// The authorization request this form carries, so it can be re-validated
    /// rather than believed.
    fn to_params(&self) -> AuthorizeParams {
        AuthorizeParams {
            response_type: self.response_type.clone(),
            client_id: self.client_id.clone(),
            redirect_uri: self.redirect_uri.clone(),
            scope: self.scope.clone(),
            state: self.state.clone(),
            code_challenge: self.code_challenge.clone(),
            code_challenge_method: self.code_challenge_method.clone(),
            resource: self.resource.clone(),
        }
    }
}

/// Act on the answer to a consent page.
///
/// Everything the form carries is re-validated here. The form is a
/// caller-supplied body like any other, and the only thing it is trusted for is
/// the answer itself — which is why it must carry a CSRF token: without one,
/// the page that could not get a code by navigation could get one by posting
/// this form instead.
#[utoipa::path(
    post,
    path = "/auth/oauth2/consent",
    tags = ["Authentication"],
    responses(
        (status = 200, description = "HTML redirect back to the client, with a code or an error", content_type = "text/html"),
        (status = 400, description = "Invalid request or CSRF token", body = crate::openapi_schemas::ErrorResponse),
        (status = 401, description = "No session", body = crate::openapi_schemas::ErrorResponse),
    )
)]
pub async fn oauth2_consent(
    State(oauth2_state): State<OAuth2State>,
    req: axum::extract::Request,
) -> Response {
    let auth_user = req.extensions().get::<crate::auth::AuthUser>().cloned();
    let (parts, body) = req.into_parts();

    let bytes = match axum::body::to_bytes(body, MAX_CONSENT_BODY_BYTES).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "invalid_request".to_string(),
                    message: "Could not read the consent form".to_string(),
                }),
            )
                .into_response();
        }
    };

    let form: ConsentForm = serde_urlencoded::from_bytes(&bytes).unwrap_or_default();

    let Some(auth_user) = auth_user else {
        return (
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "authentication_required".to_string(),
                message: "User must be authenticated".to_string(),
            }),
        )
            .into_response();
    };

    // A form, so it needs a token — and one issued to this user, on the consent
    // page this engine rendered for them. An unbound token would not do: those
    // can be collected from the sign-in page by anyone, with no browser and no
    // account, which would leave nothing here but the cookie's `SameSite=Lax`.
    if oauth2_state
        .auth_manager
        .security_context()
        .csrf
        .validate_token_for(&form.csrf_token, &auth_user.user_id)
        .await
        .is_err()
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "invalid_request".to_string(),
                message: "Missing or invalid CSRF token".to_string(),
            }),
        )
            .into_response();
    }

    let params = form.to_params();
    let host = get_request_host(&parts.headers);
    let validated =
        match validate_authorize_request(&oauth2_state.pool, &params, host.as_deref()).await {
            Ok(validated) => validated,
            Err(rejection) => return rejection.into_response(),
        };

    if form.decision != "allow" {
        return AuthorizeRejection::Redirect {
            redirect_uri: validated.redirect_uri.clone(),
            state: validated.state.clone(),
            error: "access_denied",
            description: "The request was declined".to_string(),
        }
        .into_response();
    }

    if let Err(e) = record_consent(&oauth2_state.pool, &auth_user.user_id, &validated).await {
        tracing::error!("Could not record consent: {}", e);
        return AuthorizeRejection::Redirect {
            redirect_uri: validated.redirect_uri.clone(),
            state: validated.state.clone(),
            error: "server_error",
            description: "Could not record the approval".to_string(),
        }
        .into_response();
    }

    issue_authorization_code(&oauth2_state.pool, &auth_user.user_id, &validated).await
}

/// OAuth 2.0 token request parameters (RFC 6749)
#[derive(Debug, Deserialize, utoipa::ToSchema)]
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
    refresh_token: Option<String>,

    /// Client identifier
    #[serde(default)]
    client_id: Option<String>,

    /// Client secret, for a confidential client that authenticates in the body
    /// rather than with an `Authorization: Basic` header.
    #[serde(default)]
    client_secret: Option<String>,
}

/// Client credentials presented at the token endpoint.
///
/// RFC 6749 §2.3.1 puts them in an `Authorization: Basic` header and permits
/// the request body as an alternative; both are read, and the header wins when
/// a client sends both. A public client presents an identifier and no secret,
/// which is the normal case here — every MCP client that registers dynamically
/// is one.
fn client_credentials(
    headers: &HeaderMap,
    params: &TokenParams,
) -> (Option<String>, Option<String>) {
    if let Some(encoded) = headers
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            let mut parts = value.splitn(2, ' ');
            let scheme = parts.next().unwrap_or_default();
            scheme
                .eq_ignore_ascii_case("basic")
                .then(|| parts.next().unwrap_or_default().trim())
        })
    {
        use base64::Engine;
        if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(encoded)
            && let Ok(decoded) = String::from_utf8(decoded)
            && let Some((id, secret)) = decoded.split_once(':')
        {
            // Both halves are form-urlencoded before being base64'd.
            let decode = |value: &str| {
                urlencoding::decode(value)
                    .map(|decoded| decoded.into_owned())
                    .unwrap_or_else(|_| value.to_string())
            };
            return (Some(decode(id)), Some(decode(secret)));
        }
    }

    (
        params
            .client_id
            .clone()
            .filter(|value| !value.trim().is_empty()),
        params
            .client_secret
            .clone()
            .filter(|value| !value.is_empty()),
    )
}

/// Whether a PKCE verifier matches the challenge its code was issued with.
///
/// S256 only. `plain` is permitted by RFC 7636 and is worth nothing here: the
/// challenge travels in the same query string the verifier would, so anyone
/// able to read one can read the other.
fn pkce_verifier_matches(verifier: &str, challenge: &str, method: Option<&str>) -> bool {
    if method != Some("S256") {
        return false;
    }

    use base64::Engine;
    use sha2::{Digest, Sha256};
    let computed = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));

    computed.len() == challenge.len()
        && computed
            .bytes()
            .zip(challenge.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

/// An OAuth2 error response, in the shape RFC 6749 §5.2 asks for.
fn oauth_error(status: StatusCode, error: &str, message: &str) -> Response {
    (
        status,
        Json(ErrorResponse {
            error: error.to_string(),
            message: message.to_string(),
        }),
    )
        .into_response()
}

/// Establish which registered client is making a token request.
///
/// Both grants need the same answer, and both need it before anything is spent:
/// a client that fails authentication must not burn an authorization code or a
/// refresh token that the real one was about to present.
///
/// A confidential client proves it is itself with its secret. A public client
/// holds no secret to prove anything with — PKCE stands in for that on the
/// authorization-code grant, and on the refresh grant what stands in is the
/// token being single-use and bound to the client it was issued to.
async fn authenticate_client(
    pool: &PgPool,
    headers: &HeaderMap,
    params: &TokenParams,
) -> Result<crate::auth::client_registration::RegisteredClient, Response> {
    let (presented_client_id, presented_secret) = client_credentials(headers, params);
    let Some(presented_client_id) = presented_client_id else {
        return Err(oauth_error(
            StatusCode::UNAUTHORIZED,
            "invalid_client",
            "client_id is required",
        ));
    };

    let client =
        match crate::auth::client_registration::lookup_client(pool, &presented_client_id).await {
            Ok(Some(client)) => client,
            Ok(None) => {
                return Err(oauth_error(
                    StatusCode::UNAUTHORIZED,
                    "invalid_client",
                    "Unknown client_id",
                ));
            }
            Err(e) => {
                tracing::error!("Client lookup failed: {}", e);
                return Err(oauth_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "server_error",
                    "Could not read the client registry",
                ));
            }
        };

    if let Some(expected_hash) = client.client_secret_hash.as_deref() {
        if let Some(expires_at) = client.client_secret_expires_at
            && expires_at <= Utc::now()
        {
            return Err(oauth_error(
                StatusCode::UNAUTHORIZED,
                "invalid_client",
                "Client secret has expired",
            ));
        }

        let presented_secret = presented_secret.unwrap_or_default();
        if !crate::auth::client_registration::client_secret_matches(
            &presented_secret,
            expected_hash,
        ) {
            return Err(oauth_error(
                StatusCode::UNAUTHORIZED,
                "invalid_client",
                "Client authentication failed",
            ));
        }
    }

    Ok(client)
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
#[utoipa::path(
    post,
    path = "/auth/oauth2/token",
    tags = ["Authentication"],
    request_body(content = TokenParams, content_type = "application/x-www-form-urlencoded"),
    responses(
        (status = 200, description = "Access token issued successfully", body = crate::openapi_schemas::OAuth2TokenResponse),
        (status = 400, description = "Invalid token request", body = crate::openapi_schemas::ErrorResponse),
        (status = 500, description = "Server error", body = crate::openapi_schemas::ErrorResponse),
    )
)]
pub async fn oauth2_token(
    State(oauth2_state): State<OAuth2State>,
    headers: HeaderMap,
    axum::Form(params): axum::Form<TokenParams>,
) -> Response {
    tracing::info!("📩 Token exchange request received");
    tracing::info!("  grant_type: {}", params.grant_type);
    tracing::info!("  code: {:?}", params.code);
    tracing::info!("  client_id: {:?}", params.client_id);
    tracing::info!("  redirect_uri: {:?}", params.redirect_uri);

    if params.grant_type == "refresh_token" {
        return handle_refresh_token_grant(&oauth2_state, &headers, &params).await;
    }

    // Validate grant_type
    if params.grant_type != "authorization_code" {
        return (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "unsupported_grant_type".to_string(),
                message: "Only authorization_code and refresh_token grant types are supported"
                    .to_string(),
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

    // Who is redeeming this? Established before the code is consumed, so a
    // client that fails authentication does not burn a code a legitimate one
    // was about to present.
    let client = match authenticate_client(&oauth2_state.pool, &headers, &params).await {
        Ok(client) => client,
        Err(response) => return response,
    };
    let presented_client_id = client.client_id.clone();

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

    // The code belongs to the client it was issued to. Without this, a code
    // intercepted from one client is redeemable by any other that can reach
    // this endpoint.
    if code_data.client_id != presented_client_id {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "Authorization code was not issued to this client",
        );
    }

    // Exact match, and required rather than checked only when offered: a
    // redirect URI the client declines to repeat is one it cannot be held to.
    let presented_redirect = params
        .redirect_uri
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    if presented_redirect != code_data.redirect_uri {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "redirect_uri does not match the authorization request",
        );
    }

    // PKCE, unconditionally. Verifying a challenge only when one happened to be
    // stored is what made it optional: a caller who sent none was never asked
    // for a verifier, so the code alone was enough. The authorization endpoint
    // now requires a challenge, and a stored code without one predates that and
    // is refused rather than waved through.
    let Some(challenge) = code_data.code_challenge.as_deref() else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "Authorization code was issued without PKCE and cannot be redeemed",
        );
    };

    let Some(verifier) = params.code_verifier.as_deref().filter(|v| !v.is_empty()) else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "code_verifier is required",
        );
    };

    if !pkce_verifier_matches(
        verifier,
        challenge,
        code_data.code_challenge_method.as_deref(),
    ) {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_grant",
            "PKCE verification failed",
        );
    }

    // Create a session for the user
    let ip_addr = client_ip::from_headers(&headers);
    let user_agent = client_ip::user_agent_from_headers(&headers);

    // Carry the user's identity and roles onto the session. Downstream nothing
    // distinguishes a session minted here from a browser login — the
    // administrator-only engine APIs read `is_admin` straight off it — so the
    // roles have to come from the user repository instead of defaulting to none.
    let identity = session_identity_for_user(&code_data.user_id).await;

    // Every token this endpoint issues carries an audience, whether or not the
    // client sent a `resource` parameter. That is what makes "has an audience"
    // mean "was minted for programmatic use", and so what lets session
    // validation refuse a browser cookie presented as a bearer token. A client
    // that named a resource keeps its own; one that did not gets the MCP
    // endpoint on the host it is talking to.
    //
    // Computed once, because the refresh token records the same audience: a
    // session minted from it must not reach anywhere the original
    // authorization did not.
    let audience = Some(code_data.resource.clone().unwrap_or_else(|| {
        // The canonical host, matching what the MCP endpoint compares against:
        // a request arriving on an unconfigured name is served the default
        // host's content, so a token minted there must name the host it will
        // actually be used against.
        let host = crate::hosts::resolved_host(get_request_host(&headers).as_deref());
        format!("{}{}", host, crate::auth::mcp_middleware::MCP_ENDPOINT_PATH)
    }));

    let session_params = crate::auth::session::CreateAuthSessionParams {
        user_id: code_data.user_id.clone(),
        provider: "oauth2".to_string(),
        email: identity.email,
        name: identity.name,
        is_admin: identity.is_admin,
        is_editor: identity.is_editor,
        ip_addr: ip_addr.clone(),
        user_agent: user_agent.clone(),
        refresh_token: None,
        realm: identity.realm,
        audience: audience.clone(),
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

            let config = oauth2_state.auth_manager.config();

            // A refresh token is a different credential from the session it
            // mints, which is the whole point: this endpoint used to answer
            // with the session token in both fields, so rotation was impossible
            // and a leaked refresh token was a leaked access token.
            let refresh_token = match crate::auth::refresh_tokens::issue(
                &oauth2_state.pool,
                &code_data.user_id,
                &presented_client_id,
                audience.as_deref(),
                code_data.scope.as_deref(),
                None,
                chrono::Duration::seconds(config.max_session_age as i64),
            )
            .await
            {
                Ok(token) => Some(token),
                Err(e) => {
                    // The access token is sound and the client can use it; it
                    // just has to come back through an authorization when the
                    // session times out. Better than failing an exchange that
                    // otherwise succeeded.
                    tracing::error!("Could not issue a refresh token: {}", e);
                    None
                }
            };

            let response = TokenResponse {
                access_token: session_token.token,
                token_type: "Bearer".to_string(),
                expires_in: Some(config.session_timeout),
                refresh_token,
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

/// Identity and roles to stamp onto a session minted for a user.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SessionIdentity {
    pub email: Option<String>,
    pub name: Option<String>,
    pub is_admin: bool,
    pub is_editor: bool,
    /// The host the account is a principal on. Empty when the user record
    /// could not be read, which authorizes nothing — the same fail-closed
    /// answer the roles get.
    pub realm: String,
}

/// Look up the roles a session for `user_id` should carry.
///
/// A user whose record cannot be read gets a session with no roles rather than
/// no session at all: the token exchange has already verified the
/// authorization code, and failing closed on roles keeps a transient database
/// error from handing out privileges it could not confirm.
pub async fn session_identity_for_user(user_id: &str) -> SessionIdentity {
    match crate::user_repository::get_user_async(user_id).await {
        Ok(user) => SessionIdentity {
            email: user.email,
            name: user.name,
            is_admin: user
                .roles
                .contains(&crate::user_repository::UserRole::Administrator),
            is_editor: user
                .roles
                .contains(&crate::user_repository::UserRole::Editor),
            realm: user.realm,
        },
        Err(e) => {
            tracing::warn!(
                "Could not load user {} while minting a session; issuing it with no roles: {}",
                user_id,
                e
            );
            SessionIdentity::default()
        }
    }
}

/// The `refresh_token` grant (RFC 6749 §6).
///
/// A refresh token is not a session and cannot be presented as one. It is
/// redeemed here, by the client it was issued to, for a *new* session — which
/// is why this reads roles and realm from the repository rather than copying
/// them off whatever the previous session carried. An account that lost the
/// administrator role does not get it back by refreshing.
///
/// Single use: redeeming rotates the token, and presenting a spent one revokes
/// the whole chain. See [`crate::auth::refresh_tokens`].
async fn handle_refresh_token_grant(
    oauth2_state: &OAuth2State,
    headers: &HeaderMap,
    params: &TokenParams,
) -> Response {
    let Some(presented) = params.refresh_token.as_deref().filter(|t| !t.is_empty()) else {
        return oauth_error(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "refresh_token is required for refresh_token grant",
        );
    };

    // Established before the token is spent, so a client that fails
    // authentication cannot burn a token the real one was about to present.
    let client = match authenticate_client(&oauth2_state.pool, headers, params).await {
        Ok(client) => client,
        Err(response) => return response,
    };

    let grant =
        match crate::auth::refresh_tokens::redeem(&oauth2_state.pool, presented, &client.client_id)
            .await
        {
            Ok(grant) => grant,
            Err(err) => {
                tracing::warn!("Refresh token grant rejected: {}", err);
                // One answer for every reason. Which of "never existed",
                // "expired", "already spent" and "belongs to another client"
                // it was is not something the presenter should be able to
                // learn by asking.
                return oauth_error(
                    StatusCode::BAD_REQUEST,
                    "invalid_grant",
                    "Invalid or expired refresh token",
                );
            }
        };

    let ip_addr = client_ip::from_headers(headers);
    let user_agent = client_ip::user_agent_from_headers(headers);

    // Read afresh. A session carries the roles and realm it was minted with, so
    // a refresh is the moment a revocation that happened in between takes
    // effect — copying them forward would make a refresh token a way to keep
    // holding a role that was taken away.
    let identity = session_identity_for_user(&grant.user_id).await;

    let session_params = crate::auth::session::CreateAuthSessionParams {
        user_id: grant.user_id.clone(),
        provider: "oauth2".to_string(),
        email: identity.email,
        name: identity.name,
        is_admin: identity.is_admin,
        is_editor: identity.is_editor,
        ip_addr,
        user_agent,
        refresh_token: None,
        realm: identity.realm,
        // The audience the original authorization was for, never re-derived
        // from this request: refreshing must not widen where a token reaches.
        audience: grant.audience.clone(),
    };

    let session_token = match oauth2_state
        .auth_manager
        .session_manager()
        .create_session(session_params)
        .await
    {
        Ok(token) => token,
        Err(err) => {
            tracing::error!("Refresh token grant could not mint a session: {:?}", err);
            return oauth_error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "server_error",
                "Failed to create session",
            );
        }
    };

    let config = oauth2_state.auth_manager.config();

    // Rotation: the spent token's successor, in the same family. If this fails
    // the client still has a working access token and can re-authorize when it
    // expires, which is better than refusing a refresh that already happened.
    let refresh_token = match crate::auth::refresh_tokens::issue(
        &oauth2_state.pool,
        &grant.user_id,
        &grant.client_id,
        grant.audience.as_deref(),
        grant.scope.as_deref(),
        Some(&grant.family_id),
        chrono::Duration::seconds(config.max_session_age as i64),
    )
    .await
    {
        Ok(token) => Some(token),
        Err(e) => {
            tracing::error!("Could not rotate a refresh token: {}", e);
            None
        }
    };

    (
        StatusCode::OK,
        Json(TokenResponse {
            access_token: session_token.token,
            token_type: "Bearer".to_string(),
            expires_in: Some(config.session_timeout),
            refresh_token,
            scope: grant.scope,
        }),
    )
        .into_response()
}

/// Create authentication router with all routes
pub fn create_auth_router(auth_manager: Arc<AuthManager>) -> Router {
    Router::new()
        .route("/login", get(login_page))
        .route("/account", get(account_page))
        .route("/login/{provider}", get(start_login))
        .route("/callback/{provider}", get(oauth_callback))
        .route("/guest", post(start_guest))
        .route("/local/register", post(register_local))
        .route("/local/login", post(login_local))
        .route("/local/password", post(change_password_route))
        .route("/local/claim", post(claim_account))
        .route("/local/recovery_codes", post(recovery_codes_route))
        .route("/local/recover", post(recover_account))
        .route("/logout", get(logout).post(logout))
        .route("/refresh", post(refresh_session))
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
            crate::auth::metadata::PROTECTED_RESOURCE_PATH,
            get(protected_resource_metadata_handler),
        )
        .route(
            &format!(
                "{}/{{*resource}}",
                crate::auth::metadata::PROTECTED_RESOURCE_PATH
            ),
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

    // Taken before the manager is moved into the OAuth2 state: registration is
    // unauthenticated, so its per-address budget is the only thing bounding it.
    let registration_security = auth_manager.security_context();

    let oauth2_state = OAuth2State::new(auth_manager, pool);

    let oauth2_protocol_router = Router::new()
        // Under the reserved /auth prefix, and advertised in the authorization
        // server metadata. Clients discover these per RFC 8414.
        .route(AUTHORIZE_PATH, get(oauth2_authorize))
        .route(CONSENT_PATH, post(oauth2_consent))
        .route(TOKEN_PATH, post(oauth2_token))
        .layer(cors)
        .with_state(oauth2_state);

    // Add dynamic client registration endpoint if enabled
    let router = metadata_router.merge(oauth2_protocol_router);

    if let Some(manager) = registration_manager {
        let registration_router = Router::new()
            .route(REGISTRATION_PATH, post(register_client_handler))
            .with_state(crate::auth::client_registration::ClientRegistrationState {
                manager,
                security: registration_security,
            });

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
    fn test_get_user_agent() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::USER_AGENT,
            HeaderValue::from_static("Mozilla/5.0 Test"),
        );

        let ua = client_ip::user_agent_from_headers(&headers);
        assert_eq!(ua, "Mozilla/5.0 Test");
    }

    fn authorize_params(state: Option<&str>) -> AuthorizeParams {
        AuthorizeParams {
            response_type: "code".to_string(),
            client_id: "client-1".to_string(),
            redirect_uri: Some("http://127.0.0.1:6274/callback".to_string()),
            scope: None,
            state: state.map(|s| s.to_string()),
            code_challenge: Some("abc123".to_string()),
            code_challenge_method: Some("S256".to_string()),
            resource: None,
        }
    }

    /// The login bounce has to return the caller to the path the authorization
    /// endpoint is actually mounted at. It once pointed at the withdrawn
    /// `/authorize` alias, which sent everyone who had to log in to a 404.
    #[test]
    fn test_authorize_return_url_uses_the_mounted_path() {
        let url = authorize_return_url(&authorize_params(Some("xyz")));

        assert!(
            url.starts_with(&format!("{}?", AUTHORIZE_PATH)),
            "return URL {} should start with the mounted authorize path",
            url
        );
        assert_eq!(
            safe_redirect_target(Some(&url)),
            url,
            "must survive sanitisation"
        );
    }

    #[test]
    fn test_authorize_return_url_encodes_and_skips_empty_params() {
        let url = authorize_return_url(&authorize_params(None));

        assert!(url.contains("response_type=code"));
        assert!(url.contains("client_id=client-1"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A6274%2Fcallback"));
        assert!(url.contains("code_challenge=abc123"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(
            !url.contains("state="),
            "absent state should be omitted: {}",
            url
        );
        assert!(
            !url.contains("scope="),
            "absent scope should be omitted: {}",
            url
        );
    }

    #[test]
    fn test_get_request_host_normalises_case() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::HOST,
            HeaderValue::from_static("Manage.Softagen.Com"),
        );

        assert_eq!(
            get_request_host(&headers),
            Some("manage.softagen.com".to_string())
        );
    }

    #[test]
    fn test_get_request_host_keeps_port() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("localhost:3000"));

        assert_eq!(
            get_request_host(&headers),
            Some("localhost:3000".to_string())
        );
    }

    #[test]
    fn test_get_request_host_absent() {
        assert_eq!(get_request_host(&HeaderMap::new()), None);
    }

    #[test]
    fn test_safe_redirect_target_keeps_relative_paths() {
        assert_eq!(
            safe_redirect_target(Some("/engine/installed")),
            "/engine/installed"
        );
        assert_eq!(safe_redirect_target(Some("/a?b=c#d")), "/a?b=c#d");
    }

    #[test]
    fn test_safe_redirect_target_rejects_other_origins() {
        // Absolute, protocol-relative and backslash forms would all take a
        // freshly authenticated user off the host that just set their cookie.
        for hostile in [
            "https://evil.test/",
            "http://evil.test/",
            "//evil.test/",
            "/\\evil.test/",
            "/ok\r\nLocation: https://evil.test/",
        ] {
            assert_eq!(
                safe_redirect_target(Some(hostile)),
                "/",
                "expected '{}' to be rejected",
                hostile
            );
        }
    }

    #[test]
    fn test_safe_redirect_target_defaults_to_root() {
        assert_eq!(safe_redirect_target(None), "/");
        assert_eq!(safe_redirect_target(Some("")), "/");
    }

    // ---- The authorization endpoint's gate ----
    //
    // These cover the finding this endpoint was rebuilt for: it validated
    // `response_type`, checked that `client_id` was non-empty, and stopped.
    // Any site could navigate a signed-in browser to it and be handed an
    // authorization code redirected wherever it asked.

    use crate::auth::client_registration::{ClientRegistrationManager, ClientRegistrationRequest};
    use crate::hosts::HostConfig;

    fn test_pool() -> PgPool {
        PgPool::connect_lazy("postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine")
            .expect("lazy pool should be constructible")
    }

    /// Register a public client with one redirect URI, the way an MCP client
    /// does, and return its identifier.
    async fn register_test_client(redirect_uris: Vec<String>) -> String {
        let manager = ClientRegistrationManager::new(90, test_pool());
        let response = manager
            .register_client(ClientRegistrationRequest {
                redirect_uris,
                client_name: Some("Test Client".to_string()),
                logo_uri: None,
                client_uri: None,
                contacts: None,
                tos_uri: None,
                policy_uri: None,
                token_endpoint_auth_method: Some("none".to_string()),
                grant_types: vec!["authorization_code".to_string()],
                response_types: vec!["code".to_string()],
                scope: None,
            })
            .await
            .expect("registration should succeed");
        response.client_id
    }

    /// A challenge of the shape a real S256 client sends: base64url of a
    /// 32-byte digest, so 43 characters.
    fn valid_challenge() -> String {
        "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM".to_string()
    }

    fn valid_params(client_id: &str, redirect_uri: &str) -> AuthorizeParams {
        AuthorizeParams {
            response_type: "code".to_string(),
            client_id: client_id.to_string(),
            redirect_uri: Some(redirect_uri.to_string()),
            scope: None,
            state: Some("opaque-state".to_string()),
            code_challenge: Some(valid_challenge()),
            code_challenge_method: Some("S256".to_string()),
            resource: None,
        }
    }

    /// The failure that stopped every MCP client from connecting: the client
    /// asks for the origin, and an origin authorizes nothing, because an
    /// audience is matched on host *and* path.
    #[test]
    fn a_resource_naming_only_a_host_becomes_that_host_s_mcp_endpoint() {
        assert_eq!(
            resource_audience("https://softagen.com/"),
            "softagen.com/mcp"
        );
        assert_eq!(
            resource_audience("https://softagen.com"),
            "softagen.com/mcp"
        );
        assert_eq!(resource_audience("softagen.com"), "softagen.com/mcp");
    }

    /// A resource that already names an endpoint is left as it is — narrowing
    /// is for the case where there is nothing to narrow to, never a way to move
    /// a token from the endpoint it was asked for.
    #[test]
    fn a_resource_that_names_an_endpoint_keeps_it() {
        assert_eq!(
            resource_audience("https://softagen.com/mcp"),
            "softagen.com/mcp"
        );
        assert_eq!(
            resource_audience("https://softagen.com/graphql"),
            "softagen.com/graphql"
        );
        assert_eq!(
            resource_audience("https://MANAGE.softagen.com:443/mcp/"),
            "manage.softagen.com/mcp"
        );
    }

    fn rejection_error(rejection: &AuthorizeRejection) -> &str {
        match rejection {
            AuthorizeRejection::Direct { error, .. } => error,
            AuthorizeRejection::Redirect { error, .. } => error,
        }
    }

    fn is_direct(rejection: &AuthorizeRejection) -> bool {
        matches!(rejection, AuthorizeRejection::Direct { .. })
    }

    #[tokio::test]
    async fn a_client_id_that_was_never_registered_is_refused() {
        let pool = test_pool();
        let params = valid_params("client_never-registered", "https://example.com/cb");

        let rejection = validate_authorize_request(&pool, &params, None)
            .await
            .expect_err("an unregistered client must not reach the consent page");

        assert_eq!(rejection_error(&rejection), "invalid_client");
        assert!(
            is_direct(&rejection),
            "the answer must go to the browser, never to a redirect URI no client registered"
        );
    }

    /// The heart of it. A client exists, but the request names a redirect URI
    /// that client never registered — which is how an authorization code ends
    /// up at an attacker's server.
    #[tokio::test]
    async fn a_redirect_uri_the_client_did_not_register_is_refused() {
        let client_id =
            register_test_client(vec!["http://127.0.0.1:6274/callback".to_string()]).await;
        let pool = test_pool();
        let params = valid_params(&client_id, "https://attacker.example/cb");

        let rejection = validate_authorize_request(&pool, &params, None)
            .await
            .expect_err("an unregistered redirect URI must be refused");

        assert_eq!(rejection_error(&rejection), "invalid_request");
        assert!(
            is_direct(&rejection),
            "refusing by redirecting to the unregistered URI would tell it what it wanted to know"
        );
    }

    /// Simple string comparison, per RFC 6749 §3.1.2.3. A URI that differs by a
    /// trailing slash, a case-folded path, or an extra segment is a different
    /// URI, and normalising before comparing is how an allowlist develops
    /// holes.
    #[tokio::test]
    async fn redirect_uri_matching_is_exact() {
        let registered = "http://127.0.0.1:6274/callback";
        let client_id = register_test_client(vec![registered.to_string()]).await;
        let pool = test_pool();

        for near_miss in [
            "http://127.0.0.1:6274/callback/",
            "http://127.0.0.1:6274/Callback",
            "http://127.0.0.1:6274/callback/../callback",
            "http://127.0.0.1:6274/callback?next=https://attacker.example",
            "https://127.0.0.1:6274/callback",
        ] {
            let params = valid_params(&client_id, near_miss);
            assert!(
                validate_authorize_request(&pool, &params, None)
                    .await
                    .is_err(),
                "{:?} is not the registered URI and must be refused",
                near_miss
            );
        }

        let params = valid_params(&client_id, registered);
        assert!(
            validate_authorize_request(&pool, &params, None)
                .await
                .is_ok(),
            "the registered URI itself must still work"
        );
    }

    /// PKCE was verified only when a challenge happened to have been stored, so
    /// a caller who sent none was never asked for a verifier.
    #[tokio::test]
    async fn a_request_without_a_code_challenge_is_refused() {
        let redirect = "http://127.0.0.1:6274/callback";
        let client_id = register_test_client(vec![redirect.to_string()]).await;
        let pool = test_pool();

        let mut params = valid_params(&client_id, redirect);
        params.code_challenge = None;

        let rejection = validate_authorize_request(&pool, &params, None)
            .await
            .expect_err("PKCE is required, not optional");

        assert_eq!(rejection_error(&rejection), "invalid_request");
        assert!(
            !is_direct(&rejection),
            "the client and its redirect URI checked out, so this error is the client's to handle"
        );
    }

    #[tokio::test]
    async fn plain_pkce_is_refused() {
        let redirect = "http://127.0.0.1:6274/callback";
        let client_id = register_test_client(vec![redirect.to_string()]).await;
        let pool = test_pool();

        let mut params = valid_params(&client_id, redirect);
        params.code_challenge_method = Some("plain".to_string());

        assert!(
            validate_authorize_request(&pool, &params, None)
                .await
                .is_err(),
            "a plain challenge travels in the same query string as the verifier would"
        );
    }

    #[tokio::test]
    async fn a_client_without_the_authorization_code_grant_is_refused() {
        let redirect = "http://127.0.0.1:6274/callback";
        let manager = ClientRegistrationManager::new(90, test_pool());
        let client_id = manager
            .register_client(ClientRegistrationRequest {
                redirect_uris: vec![redirect.to_string()],
                client_name: Some("Refresh Only".to_string()),
                logo_uri: None,
                client_uri: None,
                contacts: None,
                tos_uri: None,
                policy_uri: None,
                token_endpoint_auth_method: Some("none".to_string()),
                grant_types: vec!["refresh_token".to_string()],
                response_types: vec!["code".to_string()],
                scope: None,
            })
            .await
            .expect("registration should succeed")
            .client_id;

        let pool = test_pool();
        let params = valid_params(&client_id, redirect);

        let rejection = validate_authorize_request(&pool, &params, None)
            .await
            .expect_err("a client that did not register this grant cannot use it");
        assert_eq!(rejection_error(&rejection), "unauthorized_client");
    }

    // ---- Resource indicators ----

    /// The token endpoint copies `resource` onto the session's audience
    /// verbatim, so an unchecked one makes the audience mean nothing.
    #[test]
    fn a_resource_must_name_the_host_the_authorization_is_happening_on() {
        let hosts = HostConfig::new(
            "https://game.example.com",
            &["https://manage.example.com".to_string()],
        );

        assert!(
            resource_is_acceptable_for(
                "https://game.example.com/mcp",
                Some("game.example.com"),
                &hosts
            ),
            "the host the flow is running on is the one it may mint tokens for"
        );

        assert!(
            !resource_is_acceptable_for(
                "https://manage.example.com/mcp",
                Some("game.example.com"),
                &hosts
            ),
            "a sign-in on a solution host must not hand out a management-host credential"
        );

        assert!(
            !resource_is_acceptable_for(
                "https://attacker.example/mcp",
                Some("game.example.com"),
                &hosts
            ),
            "a resource this engine does not serve names nothing"
        );

        assert!(
            !resource_is_acceptable_for("", Some("game.example.com"), &hosts),
            "an empty resource names nothing either"
        );
    }

    /// A deployment that never set a base URL has nothing to check against, and
    /// refusing everything would leave it unable to issue any token at all.
    #[test]
    fn an_unconfigured_engine_accepts_any_resource() {
        let hosts = HostConfig::default();
        assert!(resource_is_acceptable_for(
            "https://anything.example/mcp",
            None,
            &hosts
        ));
    }

    // ---- Challenge and verifier shapes ----

    #[test]
    fn code_challenge_shapes_are_checked() {
        assert!(code_challenge_is_wellformed(&valid_challenge()));
        assert!(
            !code_challenge_is_wellformed("too-short"),
            "below the RFC 7636 floor of 43"
        );
        assert!(
            !code_challenge_is_wellformed(&"a".repeat(129)),
            "above the RFC 7636 ceiling of 128"
        );
        assert!(
            !code_challenge_is_wellformed(&format!("{}+", &valid_challenge()[..42])),
            "'+' is standard base64, not base64url"
        );
    }

    #[test]
    fn a_verifier_matches_only_its_own_challenge() {
        // The worked example from RFC 7636 Appendix B.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";

        assert!(pkce_verifier_matches(verifier, challenge, Some("S256")));
        assert!(!pkce_verifier_matches(
            "something else",
            challenge,
            Some("S256")
        ));
        assert!(
            !pkce_verifier_matches(challenge, challenge, Some("plain")),
            "plain is refused even when the verifier equals the challenge"
        );
        assert!(
            !pkce_verifier_matches(verifier, challenge, None),
            "a code stored without a method cannot be verified"
        );
    }

    // ---- Consent ----

    #[test]
    fn a_stored_grant_covers_only_what_it_named() {
        assert!(scope_is_covered(Some("read write"), Some("read")));
        assert!(scope_is_covered(Some("read write"), Some("read write")));
        assert!(scope_is_covered(None, None));
        assert!(
            !scope_is_covered(None, Some("read")),
            "a grant that named no scope covers no scope"
        );
        assert!(
            !scope_is_covered(Some("read"), Some("read write")),
            "widening must not happen without being seen"
        );
    }

    /// Approving once means the client is not re-approved every time — but
    /// asking for more than was approved sends the person back to the page.
    #[tokio::test]
    async fn consent_is_remembered_until_the_request_widens() {
        let redirect = "http://127.0.0.1:6274/callback";
        let client_id = register_test_client(vec![redirect.to_string()]).await;
        let pool = test_pool();
        let user_id = format!("consent-{}", uuid::Uuid::new_v4());

        let mut params = valid_params(&client_id, redirect);
        params.scope = Some("read".to_string());
        let validated = validate_authorize_request(&pool, &params, None)
            .await
            .expect("a well-formed request should validate");

        assert!(
            !consent_already_given(&pool, &user_id, &validated)
                .await
                .expect("consent lookup should succeed"),
            "a client nobody approved must be approved"
        );

        record_consent(&pool, &user_id, &validated)
            .await
            .expect("recording consent should succeed");

        assert!(
            consent_already_given(&pool, &user_id, &validated)
                .await
                .expect("consent lookup should succeed"),
            "the same request again must not ask twice"
        );

        let mut wider = valid_params(&client_id, redirect);
        wider.scope = Some("read write".to_string());
        let wider = validate_authorize_request(&pool, &wider, None)
            .await
            .expect("a well-formed request should validate");

        assert!(
            !consent_already_given(&pool, &user_id, &wider)
                .await
                .expect("consent lookup should succeed"),
            "asking for a scope nobody approved must go back to the consent page"
        );

        let _ = sqlx::query("DELETE FROM oauth_client_grants WHERE user_id = $1")
            .bind(&user_id)
            .execute(&pool)
            .await;
    }

    // ---- Token endpoint client authentication ----

    #[test]
    fn client_credentials_are_read_from_a_basic_header_or_the_body() {
        use base64::Engine;

        let body_only = TokenParams {
            grant_type: "authorization_code".to_string(),
            code: None,
            redirect_uri: None,
            code_verifier: None,
            refresh_token: None,
            client_id: Some("from-body".to_string()),
            client_secret: Some("body-secret".to_string()),
        };
        assert_eq!(
            client_credentials(&HeaderMap::new(), &body_only),
            (
                Some("from-body".to_string()),
                Some("body-secret".to_string())
            )
        );

        let mut headers = HeaderMap::new();
        let encoded = base64::engine::general_purpose::STANDARD.encode("from-header:header-secret");
        headers.insert(
            header::AUTHORIZATION,
            HeaderValue::from_str(&format!("Basic {}", encoded)).expect("valid header"),
        );
        assert_eq!(
            client_credentials(&headers, &body_only),
            (
                Some("from-header".to_string()),
                Some("header-secret".to_string())
            ),
            "RFC 6749 §2.3.1 prefers the header when both arrive"
        );
    }

    #[test]
    fn a_client_secret_matches_only_itself() {
        use sha2::{Digest, Sha256};
        let hash = hex::encode(Sha256::digest(b"the-secret"));

        assert!(crate::auth::client_registration::client_secret_matches(
            "the-secret",
            &hash
        ));
        assert!(!crate::auth::client_registration::client_secret_matches(
            "the-secre",
            &hash
        ));
        assert!(!crate::auth::client_registration::client_secret_matches(
            "", &hash
        ));
    }
}
