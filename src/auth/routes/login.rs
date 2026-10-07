//! Signing in: the login page, OAuth providers, local accounts, guests and
//! account recovery.

use super::*;
use crate::auth::AuthManager;
use crate::auth::metadata::MetadataConfig;
use crate::auth::oauth_state;
use crate::engine_page::Width;
use crate::security::client_ip;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::HeaderMap,
    response::{IntoResponse, Redirect, Response},
};
use chrono::Utc;
use serde::Deserialize;
use sqlx::PgPool;
use std::sync::Arc;

/// OAuth2 callback parameters
#[derive(Debug, Deserialize)]
pub struct OAuthCallbackParams {
    /// Authorization code from provider
    pub(super) code: Option<String>,

    /// CSRF state token
    pub(super) state: Option<String>,

    /// Error from provider
    pub(super) error: Option<String>,

    /// Error description from provider
    pub(super) error_description: Option<String>,
}

/// Login initiation parameters
#[derive(Debug, Deserialize)]
pub struct LoginParams {
    /// Optional redirect URL after successful login
    #[allow(dead_code)]
    pub(super) redirect: Option<String>,
    /// Ask the provider to challenge the person again, for a step-up.
    ///
    /// Only `"login"` means anything; anything else is ignored rather than
    /// refused, since it arrives in a URL and an unknown prompt is a request
    /// for the ordinary sign-in the engine was about to do anyway. Refused by
    /// the manager when the provider cannot honour it, because a bounce that
    /// nobody is challenged by is not a re-authentication.
    pub(super) prompt: Option<String>,
}

/// Logout parameters
#[derive(Debug, Deserialize)]
pub struct LogoutParams {
    /// Optional redirect URL after logout
    pub(super) redirect: Option<String>,
}

/// OAuth2 shared state for protocol endpoints
#[derive(Clone)]
pub struct OAuth2State {
    pub(super) auth_manager: Arc<AuthManager>,
    pub(super) pool: PgPool,
    /// Which issuer this engine is, per host.
    ///
    /// Here because RFC 9207 puts the issuer identifier in every authorization
    /// *response*, not only in the discovery document — so the endpoint that
    /// answers has to know its own name, and on a multi-host deployment that
    /// name depends on the host the flow is running on.
    pub(super) metadata: Arc<MetadataConfig>,
}

impl OAuth2State {
    pub fn new(
        auth_manager: Arc<AuthManager>,
        pool: PgPool,
        metadata: Arc<MetadataConfig>,
    ) -> Self {
        Self {
            auth_manager,
            pool,
            metadata,
        }
    }
}

/// Login page parameters
#[derive(Debug, Deserialize)]
pub struct LoginPageParams {
    /// Optional redirect URL after successful login
    pub(super) redirect: Option<String>,
    /// A code from a failed attempt, set by the engine when it bounces a form
    /// submission back here. Rendered through a fixed table of messages, never
    /// echoed, so a crafted link cannot put text on this page.
    #[serde(default)]
    pub(super) error: Option<String>,
    /// Show the sign-up form rather than the sign-in form.
    #[serde(default)]
    pub(super) signup: Option<String>,
    /// Show the recovery form rather than the sign-in form.
    #[serde(default)]
    pub(super) recover: Option<String>,
}

/// The message shown for a failed attempt.
///
/// Chosen from a fixed table by code. Unknown codes get the generic message
/// rather than their own text — this page must not be a way to render
/// attacker-chosen words next to a password field.
pub(super) fn login_error_message(code: &str) -> &'static str {
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
pub(super) fn recovery_error_message(code: &str) -> &'static str {
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
pub(super) fn render_recovery_form(
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
        r#"<form class="aw-form" method="post" action="/auth/local/recover">
                <h2>Use a recovery code</h2>
                <p class="aw-explain">One of the codes you were given when you set them up. Each works
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
                <p class="aw-small">Remembered it? <a href="/auth/login?redirect={encoded_redirect}">Sign in</a></p>
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
                r#"<label for="name">Display name <span class="aw-hint">(optional)</span></label>
                <input id="name" name="name" type="text" autocomplete="nickname">"#,
                format!(
                    r#"<p class="aw-small">Already have an account? <a href="/auth/login?redirect={}">Sign in</a></p>"#,
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
                            r#"<p class="aw-small">No account yet? <a href="/auth/login?signup=1&amp;redirect={}">Create one</a></p>"#,
                            encoded_redirect
                        ));
                    }
                    // The only way a person who has forgotten their password
                    // finds the thing that lets them in. It is on the sign-in
                    // form because that is where they are when they find out.
                    if internal.allow_recovery_codes {
                        links.push_str(&format!(
                            r#"<p class="aw-small">Forgotten it? <a href="/auth/login?recover=1&amp;redirect={}">Use a recovery code</a></p>"#,
                            encoded_redirect
                        ));
                    }
                    links
                },
            )
        };

        blocks.push(format!(
            r#"<form class="aw-form" method="post" action="{action}">
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
            r#"<p class="aw-small"><a href="{}">Change your password</a></p>"#,
            ACCOUNT_PATH
        ));
    }

    if internal.allow_guests {
        blocks.push(format!(
            r#"<form class="aw-form" method="post" action="/auth/guest">
                <input type="hidden" name="csrf_token" value="{csrf}">
                <input type="hidden" name="redirect" value="{redirect}">
                <button type="submit" class="aw-button--secondary">Continue as guest</button>
            </form>"#,
            csrf = html_attribute(csrf_token),
            redirect = html_attribute(redirect),
        ));
    }

    blocks.join("\n        ")
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
            format!(r#"<div class="aw-notice">{}</div>"#, message)
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
        r#"<p class="aw-explain">Choose a provider to continue:</p>"#.to_string()
    } else {
        r#"<div class="aw-divider"><span>or</span></div>"#.to_string()
    };

    let body = format!(
        r#"<h1>Sign in</h1>
        {error_block}
        {internal_block}
        {providers_intro}
        {provider_buttons}"#,
        error_block = error_block,
        internal_block = internal_block,
        providers_intro = providers_intro,
        provider_buttons = {
            let mut sorted_providers = providers.clone();
            sorted_providers.sort();
            sorted_providers
                .iter()
                .map(|p| format!(
                    r#"<a href="/auth/login/{}?redirect={}" class="aw-button aw-provider aw-provider--{}">{}</a>"#,
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

    page_response("Sign in", Width::Narrow, &body, &nonce)
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
) -> Result<Response, ErrorResponse> {
    let ip_addr = client_ip::from_headers(&headers);
    // Selects the redirect URI, so the flow returns to the host it began on
    // and sets its session cookie there.
    let host = get_request_host(&headers);
    let config = auth_manager.config();

    // The redirect is reduced to a local path here, before it is stored, so
    // the cookie only ever holds a target the engine would follow.
    let redirect = params
        .redirect
        .as_deref()
        .map(|target| safe_redirect_target(Some(target)));

    let now = Utc::now().timestamp();
    let login = oauth_state::PendingLogin::new(&provider, redirect, now);

    let auth_url = auth_manager
        .authorization_url(
            &provider,
            &login.nonce,
            &ip_addr,
            host.as_deref(),
            params.prompt.as_deref() == Some("login"),
        )
        .await
        .map_err(|e| ErrorResponse {
            error: "login_failed".to_string(),
            message: e.to_string(),
        })?;

    // Remember the nonce in the browser, beside whatever other logins this
    // browser already has in flight. The callback is accepted only if it comes
    // back carrying this cookie — which is what the client's IP address used
    // to stand in for, badly.
    let state_cookie_name = oauth_state::cookie_name(config.cookie_secure);
    let pending = oauth_state::push(
        cookie_from_headers(&headers, &state_cookie_name)
            .map(|value| oauth_state::decode(&value))
            .unwrap_or_default(),
        login,
        now,
    );

    let response = Redirect::temporary(&auth_url).into_response();
    let (mut parts, body) = response.into_parts();
    append_cookie(
        &mut parts.headers,
        &oauth_state::set_cookie(&oauth_state::encode(&pending), config.cookie_secure),
    )?;

    Ok(Response::from_parts(parts, body))
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
    let config = auth_manager.config();
    let state_cookie_name = oauth_state::cookie_name(config.cookie_secure);

    // The state is worth something only against what the browser remembers of
    // the login it started. Nothing about the request's network path is
    // consulted: an address is a property of the route a packet took, and
    // holding a login to the one it began on is what refused perfectly good
    // callbacks that came back over a different one.
    let pending = cookie_from_headers(&headers, &state_cookie_name)
        .map(|value| oauth_state::decode(&value))
        .unwrap_or_default();

    let Some(login) = oauth_state::take(&pending, &state, &provider, Utc::now().timestamp()) else {
        return Err(ErrorResponse {
            error: "invalid_state".to_string(),
            message: "This login could not be matched to one started in this browser. \
                      It may have been left open too long — please sign in again."
                .to_string(),
        });
    };

    // Every login still in flight but this one. The nonce just spent is
    // dropped, so a replayed callback finds nothing to match.
    let remaining: Vec<_> = pending
        .into_iter()
        .filter(|entry| entry.nonce != login.nonce)
        .collect();

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

    // Redirect to the target the login was started with, keeping the user on
    // the host whose session cookie was just set.
    let redirect_target = safe_redirect_target(login.redirect.as_deref());

    let response = Redirect::to(&redirect_target).into_response();
    let (mut parts, body) = response.into_parts();
    append_cookie(
        &mut parts.headers,
        &session_cookie_value(config, &session_token),
    )?;
    append_cookie(
        &mut parts.headers,
        &if remaining.is_empty() {
            oauth_state::clear_cookie(config.cookie_secure)
        } else {
            oauth_state::set_cookie(&oauth_state::encode(&remaining), config.cookie_secure)
        },
    )?;

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
