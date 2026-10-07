//! The account page and what it manages: password, recovery codes and sessions.

use super::*;
use crate::auth::AuthManager;
use crate::engine_page::Width;
use crate::security::client_ip;
use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Where the account page lives. Named once because the sign-in page links to
/// it, the page redirects back through it, and both forms on it post a redirect
/// target built from it.
pub(super) const ACCOUNT_PATH: &str = "/auth/account";

/// Account page parameters. Both are engine-written codes, rendered through a
/// fixed table rather than echoed — the same rule the sign-in page follows.
#[derive(Debug, Deserialize)]
pub struct AccountPageParams {
    /// What just succeeded, set by the engine when it sends a form submission
    /// back here.
    #[serde(default)]
    pub(super) notice: Option<String>,
    /// What just failed.
    #[serde(default)]
    pub(super) error: Option<String>,
}

/// The message shown for something that just worked.
pub(super) fn account_notice_message(code: &str) -> &'static str {
    match code {
        "password" => {
            "Your password has been changed. Every other session this account had is signed out."
        }
        "claimed" => "Your username and password are set. You can sign in with them from now on.",
        "recovered" => {
            "Your password was set with a recovery code. That code is spent; the rest \
                        of the set still works."
        }
        "session_ended" => "That session is over. Whoever was using it has to sign in again.",
        "sessions_ended" => {
            "Every other session is over, and so is every refresh token that could have \
             minted one."
        }
        "delegation_granted" => {
            "That app can now work on your behalf while you are away. You can withdraw it here \
             at any time."
        }
        "delegation_withdrawn" => {
            "That app can no longer act for you, and anything it had queued has been cancelled."
        }
        "delegation_declined" => "Nothing was authorised.",
        "delegation_missing" => "There was nothing to withdraw.",
        "elevation_granted" => {
            "Those rights are switched on for this session. They go back off by themselves, \
             and you can hand them back sooner from here."
        }
        "elevation_dropped" => "Those rights are switched off again.",
        "elevation_declined" => "Nothing was switched on.",
        "sender_unlinked" => {
            "That sender can no longer start work for you. The app can still act for you \
             otherwise — withdraw it above to stop that too."
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
pub(super) fn account_error_message(code: &str) -> &'static str {
    match code {
        "credentials" => "That is not your current password.",
        "permissions" => {
            "Your account may not do that, so there is nothing to switch on. An \
             administrator can change what your account holds."
        }
        "authentication_required" => {
            "Sign in again before switching these on. Proving you are here is the point \
             of asking."
        }
        "session" => "That session is not one of yours to end. It may already be over.",
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
            r#"<form class="aw-form" method="post" action="/auth/local/password">
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
                <p class="aw-small">Changing it signs out every other session this account has.</p>
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
                r#"<form class="aw-form" method="post" action="/auth/local/claim">
                <h2>Add a username and password</h2>
                <p class="aw-explain">{explain}</p>
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
pub(super) fn render_recovery_codes_form(
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
        r#"<form class="aw-form" method="post" action="/auth/local/recovery_codes">
                <h2>Recovery codes</h2>
                <p class="aw-explain">{standing} Each one can set a new password once, if you forget
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

/// Where an account is signed in, and the controls for ending any of it.
///
/// Rendered for every account, not only ones with a password: a session is a
/// session however it was minted, and somebody signed in through Google has the
/// same reason to want a look at this.
///
/// There is no device name in the list because the engine does not keep one —
/// the fingerprint holds a *hash* of the User-Agent, enough to notice it
/// changed and not enough to say what it was. What a session is recognised by
/// is the address it started from and when it was last used, and the page says
/// so rather than leaving a person wondering which of the two Chromes is which.
pub(super) fn render_sessions(
    csrf_token: &str,
    sessions: &[crate::security::SessionSummary],
) -> String {
    if sessions.is_empty() {
        return String::new();
    }

    let rows = sessions
        .iter()
        .map(|session| {
            let label = if session.current {
                "This session".to_string()
            } else if let Some(audience) = session.audience.as_deref() {
                format!("API token for {}", html_escape::encode_text(audience))
            } else {
                format!(
                    "Signed in via {}",
                    html_escape::encode_text(&session.provider)
                )
            };

            // The current session is not offered a button: ending it is what
            // "Sign out" does, and a button here that silently signed somebody
            // out would read as one that had failed.
            let control = if session.current {
                String::new()
            } else {
                format!(
                    r#"<form method="post" action="/auth/sessions/revoke">
                    <input type="hidden" name="csrf_token" value="{csrf}">
                    <input type="hidden" name="session" value="{id}">
                    <button type="submit" class="aw-button--danger aw-button--small">End</button>
                </form>"#,
                    csrf = html_attribute(csrf_token),
                    id = session.id,
                )
            };

            format!(
                r#"<li>
                <div>
                    <span class="aw-row-title">{label}</span>
                    <span class="aw-row-meta">from {ip} · started {started} · last used {used}</span>
                </div>
                {control}
            </li>"#,
                label = label,
                ip = html_escape::encode_text(&session.ip_addr),
                started = page_timestamp(session.created_at),
                used = page_timestamp(session.last_access),
                control = control,
            )
        })
        .collect::<Vec<_>>()
        .join("\n            ");

    // Offered only when there is something else to end, so the button is never
    // one that does nothing.
    let end_others = if sessions.len() > 1 {
        format!(
            r#"<form class="aw-form" method="post" action="/auth/sessions/revoke">
                <input type="hidden" name="csrf_token" value="{csrf}">
                <button type="submit" class="aw-button--secondary">Sign out everywhere else</button>
            </form>"#,
            csrf = html_attribute(csrf_token),
        )
    } else {
        String::new()
    };

    format!(
        r#"<h2>Where you are signed in</h2>
        <p class="aw-explain">The engine keeps only a hash of the browser that started a session, so
        these are named by the address they came from rather than by device.</p>
        <ul class="aw-rows">
            {rows}
        </ul>
        {end_others}"#,
        rows = rows,
        end_others = end_others,
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

    // The token is kept as well as the session it names: the session listing
    // below marks which row is this one, and comparing them is the only way to
    // know.
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
            r#"<div class="aw-notice">{}</div>"#,
            account_error_message(code)
        ),
        (None, Some(code)) => format!(
            r#"<div class="aw-notice aw-notice--ok">{}</div>"#,
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
        r#"<p class="aw-explain">This engine holds no credentials of its own, so there is nothing to change here.</p>"#
            .to_string()
    } else {
        forms
    };

    // Not gated on internal credentials: a session is a session however it was
    // minted, and somebody signed in through a provider has the same reason to
    // want a look at this. A listing that fails is left out rather than failing
    // the page, so the credential controls above it still work.
    let sessions = match auth_manager.list_sessions(&session.user_id, &token).await {
        Ok(sessions) => render_sessions(&csrf_token, &sessions),
        Err(e) => {
            tracing::error!("Could not list sessions for an account page: {}", e);
            String::new()
        }
    };

    // Beside the sessions, for the same reason they are there: a background
    // job acting as you is the same question as a session acting as you, and
    // there should be one place to look. A listing that fails is left out
    // rather than failing the page.
    let delegations = match crate::delegation::list_for_user(&session.user_id).await {
        Ok(grants) => {
            // A links listing that fails leaves the grants rendered without
            // them, for the same reason the grants listing failing leaves the
            // page without the section: half an account page is better than
            // none, and the part that is shown is accurate.
            let links = crate::delegation::list_channels_for_user(&session.user_id)
                .await
                .unwrap_or_default();
            render_delegations(&csrf_token, &grants, &links)
        }
        Err(e) => {
            tracing::error!("Could not list delegations for an account page: {}", e);
            String::new()
        }
    };

    let elevation = render_elevation(
        &csrf_token,
        session.elevation.as_ref(),
        &crate::security::elevation::configured(),
    );

    let body = format!(
        r#"<h1>Your account</h1>
        {notice_block}
        <p class="aw-identity">Signed in as <strong>{label}</strong>
            <span class="aw-muted">via {provider}</span></p>
        {forms}
        {elevation}
        {delegations}
        {sessions}
        <p class="aw-small"><a href="/auth/logout">Sign out</a></p>"#,
        notice_block = notice_block,
        label = html_escape::encode_text(&label),
        provider = html_escape::encode_text(&session.provider),
        forms = forms,
        elevation = elevation,
        delegations = delegations,
        sessions = sessions,
    );

    let mut response = signed_in_page_response("Your account", Width::Wide, &body, &nonce);
    // The page names the account it belongs to. A shared cache holding it would
    // hand one person's to the next.
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
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

/// Request body for `POST /auth/sessions/revoke`.
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
pub struct RevokeSessionRequest {
    /// Which session to end, by the id the listing gave it. Absent means every
    /// session but the one asking — the control for a lost device, where the
    /// point is not having to know which row it is.
    #[serde(default)]
    pub session: Option<String>,
    #[serde(default)]
    pub redirect: Option<String>,
    #[serde(default)]
    pub csrf_token: Option<String>,
}

/// What `GET /auth/sessions` answers with.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct SessionListResponse {
    #[schema(value_type = Vec<Object>)]
    pub sessions: Vec<crate::security::SessionSummary>,
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
pub(super) fn render_recovery_codes_page(codes: &[String]) -> Response {
    let nonce = crate::security::generate_nonce();
    let items = codes
        .iter()
        .map(|code| format!("<li>{}</li>", html_escape::encode_text(code)))
        .collect::<Vec<_>>()
        .join("\n            ");

    let body = format!(
        r#"<h1>Recovery codes</h1>
        <div class="aw-notice aw-notice--ok">These replace any codes you had before.</div>
        <p class="aw-explain">Write them down somewhere that is not this computer. Each one can set a
        new password once, and they are shown here and nowhere else — the engine keeps only a hash,
        so it cannot show them to you again.</p>
        <ul class="aw-codes">
            {items}
        </ul>
        <p class="aw-small"><a href="/auth/account">Back to your account</a></p>"#,
        items = items,
    );

    let mut response = signed_in_page_response("Recovery codes", Width::Narrow, &body, &nonce);
    // The one response in the engine that carries credentials in its body.
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}

/// The sessions the calling account holds.
///
/// Its own sessions and nobody else's: the account comes from the session the
/// request arrived on, never from a parameter. Nothing in the answer
/// authenticates — a session is named by a surrogate key, and the token that
/// would let somebody use it stays where it is.
#[utoipa::path(
    get,
    path = "/auth/sessions",
    tags = ["Authentication"],
    responses(
        (status = 200, description = "The account's sessions, newest use first", body = SessionListResponse),
        (status = 401, description = "No session", body = crate::openapi_schemas::ErrorResponse),
    )
)]
pub async fn list_sessions_route(
    State(auth_manager): State<Arc<AuthManager>>,
    headers: HeaderMap,
) -> Result<Response, AuthErrorResponse> {
    let config = auth_manager.config();
    let ip_addr = client_ip::from_headers(&headers);
    let user_agent = client_ip::user_agent_from_headers(&headers);

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

    let sessions = auth_manager.list_sessions(&session.user_id, &token).await?;

    let mut response = Json(SessionListResponse { sessions }).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    Ok(response)
}

/// End one of the calling account's sessions, or all but this one.
///
/// A POST rather than a `DELETE` on the session's own path because the control
/// is a button on a page, and a browser form cannot send anything but GET and
/// POST.
#[utoipa::path(
    post,
    path = "/auth/sessions/revoke",
    tags = ["Authentication"],
    request_body = RevokeSessionRequest,
    responses(
        (status = 200, description = "The session, or every other session, is gone"),
        (status = 401, description = "No session", body = crate::openapi_schemas::ErrorResponse),
        (status = 404, description = "This account has no such session", body = crate::openapi_schemas::ErrorResponse),
    )
)]
pub async fn revoke_session_route(
    State(auth_manager): State<Arc<AuthManager>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, AuthErrorResponse> {
    let (request, style) = parse_auth_body::<RevokeSessionRequest>(&headers, &body);
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
        // Defaulting to the account page rather than the sign-in page, the way
        // the success path does: these controls exist on one page, and a caller
        // that named no redirect came from it.
        return Ok(redirect_to_form_with_error(
            &crate::auth::error::AuthError::CsrfValidationFailed,
            request.redirect.as_deref().or(Some(ACCOUNT_PATH)),
        ));
    }

    let named = request
        .session
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    let notice = match named {
        Some(value) => {
            // An id that is not a UUID and an id belonging to somebody else are
            // the same answer, because they are the same thing: this account
            // has no such session. Not a 401 — the caller's own session is
            // fine, and saying otherwise would send them off to sign in again
            // over somebody else's row.
            let ended = match value.parse::<uuid::Uuid>() {
                Ok(id) => {
                    auth_manager
                        .revoke_session(&session.user_id, id, &token)
                        .await?
                }
                Err(_) => false,
            };

            if !ended {
                return Ok(no_such_session(style, request.redirect.as_deref()));
            }

            "session_ended"
        }
        None => {
            auth_manager
                .revoke_other_sessions(&session.user_id, &token)
                .await?;
            "sessions_ended"
        }
    };

    if style == RequestStyle::Form {
        let target = match request.redirect.as_deref() {
            Some(value) if !value.trim().is_empty() => safe_redirect_target(Some(value)),
            _ => format!("{}?notice={}", ACCOUNT_PATH, notice),
        };
        return Ok(Redirect::to(&target).into_response());
    }

    Ok(Json(InternalAuthResponse {
        success: true,
        user_id: Some(session.user_id),
        username: None,
    })
    .into_response())
}

/// What a caller is told when the session they named is not one of theirs.
///
/// A browser gets the page back with a message, because the row it was looking
/// at is simply gone — somebody ended it from another tab, or it aged out. An
/// API caller gets a 404, which is what "no such session" is; a 401 would say
/// the caller's own session was the problem.
pub(super) fn no_such_session(style: RequestStyle, redirect: Option<&str>) -> Response {
    if style == RequestStyle::Form {
        let target = match redirect {
            Some(value) if !value.trim().is_empty() => safe_redirect_target(Some(value)),
            _ => format!("{}?error=session", ACCOUNT_PATH),
        };
        return Redirect::to(&target).into_response();
    }

    (
        StatusCode::NOT_FOUND,
        Json(ErrorResponse {
            error: "not_found".to_string(),
            message: "No such session for this account".to_string(),
        }),
    )
        .into_response()
}
