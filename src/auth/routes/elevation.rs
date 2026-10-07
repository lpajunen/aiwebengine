//! Step-up: elevating a session for minutes at a time, and dropping it.

use super::*;
use crate::auth::AuthManager;
use crate::engine_page::Width;
use crate::security::client_ip;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use std::sync::Arc;

/// What this person has authorised scripts to do as them, and a way to stop it.
///
/// Rendered beside the sessions and for the same reason: a background job
/// acting as you is the same question as a session acting as you, and the
/// place people look for "what is currently able to act as me" is one place.
/// What this session has switched on, and the way to hand it back.
///
/// Beside the delegations and the sessions, for the reason they are beside
/// each other: an elevated session, a delegated script and a live session are
/// the same question — *what is currently able to act as me* — asked at three
/// lifetimes, and there should be one place to look.
///
/// Empty when the engine gates nothing, because then there is nothing to
/// switch on and a panel offering it would be offering a decision that changes
/// nothing.
pub(super) fn render_elevation(
    csrf_token: &str,
    elevation: Option<&crate::security::elevation::Elevation>,
    policy: &crate::security::elevation::Policy,
) -> String {
    if !policy.is_active() {
        return String::new();
    }

    let now = chrono::Utc::now();
    let live = elevation.filter(|elevation| elevation.is_live(now));

    let body = match live {
        Some(elevation) => format!(
            r#"<li>
                <div>
                    <span class="aw-row-title">Switched on</span>
                    <span class="aw-row-meta">until {until}</span>
                </div>
                <form method="post" action="/auth/elevate/drop">
                    <input type="hidden" name="csrf_token" value="{csrf}">
                    <input type="hidden" name="redirect" value="{account}">
                    <button type="submit" class="aw-button--danger aw-button--small">Switch off</button>
                </form>
            </li>"#,
            until = page_timestamp_utc(elevation.expires_at),
            csrf = html_attribute(csrf_token),
            account = html_attribute(ACCOUNT_PATH),
        ),
        None => format!(
            r#"<li>
                <div>
                    <span class="aw-row-title">Nothing switched on</span>
                    <span class="aw-row-meta">this session holds only what using a solution needs</span>
                </div>
                <a class="aw-small" href="{elevate}">Switch some on</a>
            </li>"#,
            elevate = html_attribute(&elevate_url(None, Some(ACCOUNT_PATH))),
        ),
    };

    format!(
        r#"<h2>Extra rights</h2>
        <ul class="aw-rows">
            {body}
        </ul>"#
    )
}

// ============================================================================
// Session elevation — switching authority on deliberately, rather than
// carrying it for the life of a session.
//
// `security::elevation` holds the vocabulary and the arithmetic; this is the
// way in. It is `delegate_page` with the nouns changed, and deliberately so:
// a delegation asks "may this app act for me while I am away" and an elevation
// asks "may I act as myself, with more than I usually carry, for the next few
// minutes". Same shape of decision, same shape of page, and they sit beside
// each other on the account page for the same reason the sessions do.
// ============================================================================

/// Where `GET /auth/elevate` is pointed from.
#[derive(Debug, Deserialize)]
pub struct ElevateParams {
    /// Capability names the caller was refused for, comma-separated.
    ///
    /// Used only to pre-tick the bundles that would supply them. A name this
    /// engine does not know is ignored rather than refused: it arrives in a
    /// URL somebody may have edited, and the page's job is to offer the right
    /// boxes rather than to validate a hint.
    pub(super) need: Option<String>,
    /// Where to return to once the decision is made. A path on this engine.
    pub(super) redirect: Option<String>,
    pub(super) error: Option<String>,
    pub(super) notice: Option<String>,
}

/// What `POST /auth/elevate` is given.
#[derive(Debug, Default, Deserialize)]
pub struct ElevateRequest {
    pub(super) csrf_token: Option<String>,
    pub(super) redirect: Option<String>,
    /// The current password, for an account that holds one.
    pub(super) password: Option<String>,
    /// How long to hold it, in minutes. Bounded by the configured ceiling.
    pub(super) minutes: Option<i64>,
}

/// What `POST /auth/elevate/drop` is given.
#[derive(Debug, Default, Deserialize)]
pub struct ElevateDropRequest {
    pub(super) csrf_token: Option<String>,
    pub(super) redirect: Option<String>,
}

/// Which bundles a body ticked.
///
/// Read from the body directly for the reason [`scopes_from_body`] is: a form
/// sends one `grade=` per checkbox and `serde_urlencoded` does not deserialise
/// repeated keys into a `Vec`, so a derived struct would silently see one.
pub(super) fn grades_from_body(
    style: RequestStyle,
    body: &[u8],
) -> Vec<crate::security::elevation::Grade> {
    use crate::security::elevation::Grade;

    let names: Vec<String> = match style {
        RequestStyle::Form => url::form_urlencoded::parse(body)
            .filter(|(key, _)| key == "grade")
            .map(|(_, value)| value.into_owned())
            .collect(),
        RequestStyle::Json => serde_json::from_slice::<serde_json::Value>(body)
            .ok()
            .and_then(|value| {
                value.get("grade").and_then(|grade| {
                    grade.as_array().map(|items| {
                        items
                            .iter()
                            .filter_map(|item| item.as_str().map(str::to_string))
                            .collect()
                    })
                })
            })
            .unwrap_or_default(),
    };

    let mut grades: Vec<Grade> = names.iter().filter_map(|name| Grade::parse(name)).collect();
    grades.sort_unstable();
    grades.dedup();
    grades
}

/// Which bundles would supply the capabilities a refusal named.
///
/// The `need=` hint is capability names, because that is what a refusal
/// carries; the page offers bundles. This is the translation, and it is a
/// containment test rather than a lookup so that a refusal naming two
/// capabilities from one bundle ticks one box.
pub(super) fn grades_supplying(needed: &str) -> Vec<crate::security::elevation::Grade> {
    use crate::security::Capability;
    use crate::security::elevation::Grade;

    let wanted: Vec<Capability> = needed.split(',').filter_map(Capability::parse).collect();

    Grade::all()
        .into_iter()
        .filter(|grade| {
            let carries = grade.capabilities();
            wanted.iter().any(|capability| carries.contains(capability))
        })
        .collect()
}

/// The page a person decides on.
///
/// Reached two ways: from a refusal that carried `need=`, which is the path
/// that matters because a person choosing in context reads the prompt, and
/// from the account page, where somebody about to do a piece of work asks for
/// it up front.
#[utoipa::path(
    get,
    path = "/auth/elevate",
    tags = ["Authentication"],
    params(
        ("need" = Option<String>, Query, description = "Capability names a refusal asked for"),
        ("redirect" = Option<String>, Query, description = "Path on this engine to return to"),
    ),
    responses(
        (status = 200, description = "Elevation page HTML", content_type = "text/html"),
        (status = 302, description = "No session; redirected to the sign-in page"),
        (status = 404, description = "This engine gates nothing, so there is nothing to elevate"),
    )
)]
pub async fn elevate_page(
    State(auth_manager): State<Arc<AuthManager>>,
    Query(params): Query<ElevateParams>,
    headers: HeaderMap,
) -> Response {
    use crate::security::elevation;

    let policy = elevation::configured();
    // An engine gating nothing has no elevation to offer, and a page that
    // granted one would be offering a decision that changes nothing. 404
    // rather than a page saying so, for the reason the management-host guard
    // answers 404: a surface that is not in use should not advertise itself.
    if !policy.is_active() {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }

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

    let here = elevate_url(params.need.as_deref(), params.redirect.as_deref());
    let Some(session) = session else {
        return Redirect::to(&format!(
            "/auth/login?redirect={}",
            urlencoding::encode(&here)
        ))
        .into_response();
    };

    // Whether the account holds a password decides how it proves presence, so
    // a lookup that failed must not read as "there is none" — that would show
    // somebody with a password the form that does not ask for it.
    let username = match crate::auth::local::username_for_user(&session.user_id).await {
        Ok(username) => username,
        Err(e) => {
            tracing::error!("Could not read the credential for an elevation page: {}", e);
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

    let wanted = params
        .need
        .as_deref()
        .map(grades_supplying)
        .unwrap_or_default();

    let checkboxes = policy
        .gated
        .iter()
        .map(|grade| {
            // A bundle the account's roles cannot reach is shown disabled with
            // the reason rather than hidden. "You are not an administrator" is
            // the useful answer; a missing box is a person wondering where it
            // went.
            let reachable = !grade.requires_administrator() || session.is_admin;
            let ticked = reachable && wanted.contains(grade);
            format!(
                r#"<label class="aw-choice">
                    <input type="checkbox" name="grade" value="{value}"{checked}{disabled}>
                    {description}{note}
                </label>"#,
                value = html_attribute(grade.as_str()),
                checked = if ticked { " checked" } else { "" },
                disabled = if reachable { "" } else { " disabled" },
                description = html_escape::encode_text(grade.describe()),
                note = if reachable {
                    String::new()
                } else {
                    r#" <span class="aw-muted">— your account is not an administrator</span>"#
                        .to_string()
                },
            )
        })
        .collect::<Vec<_>>()
        .join("\n                ");

    // How presence is proved. A local account types its password. A federated
    // one has none here, so the only honest proof is a fresh sign-in — and
    // this deliberately does not offer a button that would round-trip to the
    // provider, because without `prompt=login` the provider answers from its
    // own cookie and nobody proves anything. See `docs/SESSION_ELEVATION.md`.
    let window = chrono::Duration::seconds(policy.reauth_window_secs);
    let recently = session
        .reauthenticated_at
        .is_some_and(|at| chrono::Utc::now() - at < window);
    let proof = match (&username, recently) {
        (Some(_), _) => r#"<label class="aw-field">Your password
                <input type="password" name="password" autocomplete="current-password" required>
            </label>"#
            .to_string(),
        (None, true) => r#"<p class="aw-explain">You signed in a moment ago, so that is proof
        enough this time.</p>"#
            .to_string(),
        // The provider can be asked to check them again, so offer that rather
        // than a sign-out: it is one click, it comes back here, and it is a
        // real challenge rather than a round trip answered from a cookie.
        (None, false)
            if auth_manager.can_force_reauthentication(host.as_deref(), &session.provider) =>
        {
            format!(
                r#"<p class="aw-explain">Switching these on needs you to sign in again — that is
        what asking is for.</p>
        <p class="aw-small"><a href="/auth/login/{provider_path}?prompt=login&amp;redirect={back}">Check
        me again with {provider}</a></p>"#,
                provider_path = html_attribute(&session.provider),
                provider = html_escape::encode_text(&session.provider),
                back = html_attribute(&urlencoding::encode(&here)),
            )
        }
        // It cannot, and the engine does not pretend otherwise. A bounce a
        // provider answers from its own session proves nothing, so the only
        // honest instruction is the one that actually re-authenticates.
        (None, false) => format!(
            r#"<p class="aw-explain">This account signs in through {provider}, which the engine
        cannot ask to check you again. <a href="/auth/logout">Sign out and back in</a>, then
        come straight here.</p>"#,
            provider = html_escape::encode_text(&session.provider),
        ),
    };

    let can_submit = username.is_some() || recently;

    let live = session
        .elevation
        .as_ref()
        .filter(|elevation| elevation.is_live(chrono::Utc::now()));
    let current = match live {
        Some(elevation) => format!(
            r#"<p class="aw-explain">This session is already elevated until <strong>{until}</strong>.
        Elevating again replaces it.</p>"#,
            until = page_timestamp_utc(elevation.expires_at),
        ),
        None => String::new(),
    };

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

    let durations = [15_i64, 30, 60, 240]
        .into_iter()
        .filter(|minutes| *minutes <= policy.max_minutes)
        .map(|minutes| {
            format!(
                r#"<option value="{minutes}"{selected}>{minutes} minutes</option>"#,
                minutes = minutes,
                selected = if minutes == 30 { " selected" } else { "" },
            )
        })
        .collect::<Vec<_>>()
        .join("\n                    ");
    // Every step longer than the ceiling was filtered out, which for a tight
    // ceiling leaves nothing. The ceiling itself is always offerable.
    let durations = if durations.is_empty() {
        format!(
            r#"<option value="{m}" selected>{m} minutes</option>"#,
            m = policy.max_minutes
        )
    } else {
        durations
    };

    let body = format!(
        r#"<h1>Switch on more rights</h1>
        {notice_block}
        <p class="aw-identity">Signed in as <strong>{label}</strong></p>
        <p class="aw-explain">Your account may do these things. This session is not doing them
        yet — switch on what you need, for as long as you need it, and it goes back off by
        itself.</p>
        {current}
        <form method="post" action="/auth/elevate">
            <input type="hidden" name="csrf_token" value="{csrf}">
            <input type="hidden" name="redirect" value="{redirect}">
            {checkboxes}
            <label class="aw-field">For
                <select name="minutes">
                    {durations}
                </select>
            </label>
            {proof}
            <button type="submit"{submit_disabled}>Switch on</button>
        </form>
        <p class="aw-small"><a href="{back}">Not now</a></p>"#,
        notice_block = notice_block,
        label = html_escape::encode_text(
            &username
                .clone()
                .or_else(|| session.email.clone())
                .unwrap_or_else(|| session.user_id.clone())
        ),
        current = current,
        csrf = html_attribute(&csrf_token),
        redirect = html_attribute(&safe_redirect_target(params.redirect.as_deref())),
        checkboxes = checkboxes,
        durations = durations,
        proof = proof,
        submit_disabled = if can_submit { "" } else { " disabled" },
        back = html_attribute(&safe_redirect_target(params.redirect.as_deref())),
    );

    let mut response =
        signed_in_page_response("Switch on more rights", Width::Narrow, &body, &nonce);
    // The page names the account it belongs to and carries a CSRF token bound
    // to it. A shared cache holding it would hand one person's to the next.
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    response
}

/// Where the elevation page lives for a given refusal.
///
/// One place builds this URL, because a refusal names it, the account page
/// links to it, and the sign-in redirect has to come back to exactly the same
/// one.
pub fn elevate_url(need: Option<&str>, redirect: Option<&str>) -> String {
    let mut url = "/auth/elevate".to_string();
    let mut separator = '?';
    if let Some(need) = need.filter(|value| !value.trim().is_empty()) {
        url.push(separator);
        url.push_str(&format!("need={}", urlencoding::encode(need)));
        separator = '&';
    }
    if let Some(redirect) = redirect.filter(|value| !value.trim().is_empty()) {
        url.push(separator);
        url.push_str(&format!(
            "redirect={}",
            urlencoding::encode(&safe_redirect_target(Some(redirect)))
        ));
    }
    url
}

/// Switch on what was ticked, for as long as was asked.
#[utoipa::path(
    post,
    path = "/auth/elevate",
    tags = ["Authentication"],
    responses(
        (status = 200, description = "The elevation that was recorded"),
        (status = 302, description = "Form post; redirected back with a notice"),
        (status = 401, description = "No session"),
        (status = 404, description = "This engine gates nothing"),
    )
)]
pub async fn elevate_route(
    State(auth_manager): State<Arc<AuthManager>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, AuthErrorResponse> {
    use crate::security::elevation::{self, Elevation, Method};

    let policy = elevation::configured();
    if !policy.is_active() {
        return Ok((StatusCode::NOT_FOUND, "Not found").into_response());
    }

    let (request, style) = parse_auth_body::<ElevateRequest>(&headers, &body);
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

    let back = elevate_url(None, request.redirect.as_deref());
    let fail = |error: crate::auth::error::AuthError| -> Response {
        Redirect::to(&format!("{}&error={}", back, error_code_for(&error))).into_response()
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
        return Ok(fail(crate::auth::error::AuthError::CsrfValidationFailed));
    }

    let grades = grades_from_body(style, &body);
    // Ticking nothing is a refusal, as it is on the delegation page. Recording
    // an empty elevation would leave a session claiming to be elevated to
    // nothing, which is the opposite of what was just said.
    if grades.is_empty() {
        return Ok(elevation_answer(
            style,
            request.redirect.as_deref(),
            "elevation_declined",
            serde_json::json!({ "elevated": false }),
        ));
    }

    // A bundle the account's roles cannot reach is refused here rather than
    // granted and intersected away. The intersection in `for_session` already
    // makes it harmless; what it does not do is tell anybody, and a button
    // that reports success and changes nothing is worse than a refusal.
    if grades
        .iter()
        .any(|grade| grade.requires_administrator() && !session.is_admin)
    {
        return Ok(fail(crate::auth::error::AuthError::InsufficientPermissions));
    }

    // Proof of presence. A password when the account has one — checked against
    // the credential rather than trusted from the session, for the reason
    // `change_password` gives: a stolen cookie must not be enough.
    let username = crate::auth::local::username_for_user(&session.user_id)
        .await
        .map_err(|e| {
            tracing::error!("Could not read the credential while elevating: {}", e);
            crate::auth::error::AuthError::Internal("could not check the credential".into())
        })?;

    let method = if username.is_some() {
        let password = request.password.as_deref().unwrap_or_default();
        // Per address and per account, exactly as `login_local` is throttled:
        // a guess here is worth more than a sign-in, not less.
        let account = username.clone().unwrap_or_else(|| session.user_id.clone());
        if !auth_manager
            .security_context()
            .check_auth_rate_limit(&ip_addr)
            .await
            || !auth_manager
                .security_context()
                .account_login_allowed(&account)
                .await
        {
            return Ok(fail(crate::auth::error::AuthError::RateLimitExceeded));
        }
        if crate::auth::local::verify_user_password(&session.user_id, password)
            .await
            .is_err()
        {
            auth_manager
                .security_context()
                .record_account_login_failure(&account)
                .await;
            return Ok(fail(crate::auth::error::AuthError::InvalidCredentials));
        }
        Method::Password
    } else {
        // No credential to check, so the only proof is a recent sign-in.
        // Deliberately not a round trip to the provider: without
        // `prompt=login` the provider answers from its own cookie and nobody
        // proves anything, and an engine that called that re-authentication
        // would be claiming something it cannot back.
        let window = chrono::Duration::seconds(policy.reauth_window_secs);
        let fresh = session
            .reauthenticated_at
            .is_some_and(|at| chrono::Utc::now() - at < window);
        if !fresh {
            return Ok(fail(crate::auth::error::AuthError::AuthenticationRequired));
        }
        Method::Provider
    };

    let minutes = request.minutes.unwrap_or(policy.max_minutes);
    let elevation = Elevation::grant(&grades, method, minutes, chrono::Utc::now());

    auth_manager
        .session_manager()
        .set_elevation(&token, Some(elevation.clone()))
        .await
        .map_err(|e| {
            tracing::error!("Could not record an elevation: {}", e);
            crate::auth::error::AuthError::Internal("could not record the elevation".into())
        })?;

    // High, because this is somebody taking authority they were not carrying a
    // moment ago, and it is the line an investigation starts from.
    auth_manager
        .security_context()
        .auditor
        .log_event(
            crate::security::SecurityEvent::new(
                // The vocabulary has no "granted" note — every authorization
                // event in it is a refusal — so this is a system security
                // event rather than a new variant invented for one caller.
                // Naming it is a change to the audit vocabulary, which is a
                // decision of its own and not one to make in passing.
                crate::security::SecurityEventType::SystemSecurityEvent,
                crate::security::SecuritySeverity::High,
                Some(session.user_id.clone()),
            )
            .with_detail("event", "session_elevated")
            .with_detail(
                "grades",
                grades
                    .iter()
                    .map(|grade| grade.as_str())
                    .collect::<Vec<_>>()
                    .join(","),
            )
            .with_detail("method", method.as_str())
            .with_detail("until", elevation.expires_at.to_rfc3339())
            .with_detail("ip_address", &ip_addr),
        )
        .await;

    Ok(elevation_answer(
        style,
        request.redirect.as_deref(),
        "elevation_granted",
        serde_json::json!({
            "elevated": true,
            "grades": grades.iter().map(|grade| grade.as_str()).collect::<Vec<_>>(),
            "expiresAt": elevation.expires_at.to_rfc3339(),
        }),
    ))
}

/// Give it back before it runs out.
///
/// The "exit sudo" half. Cheap, and it is what makes the whole thing feel like
/// a decision rather than a tax: somebody who has finished administering can
/// say so instead of carrying it until the timer runs down.
#[utoipa::path(
    post,
    path = "/auth/elevate/drop",
    tags = ["Authentication"],
    responses(
        (status = 200, description = "The elevation is over"),
        (status = 302, description = "Form post; redirected back with a notice"),
        (status = 401, description = "No session"),
    )
)]
pub async fn elevate_drop_route(
    State(auth_manager): State<Arc<AuthManager>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, AuthErrorResponse> {
    let (request, style) = parse_auth_body::<ElevateDropRequest>(&headers, &body);
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

    // Not gated on the policy being active. An engine that stopped gating
    // something must still let a session hand back what it was carrying, and
    // dropping authority is never the call to refuse.
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
            request.redirect.as_deref().or(Some(ACCOUNT_PATH)),
        ));
    }

    auth_manager
        .session_manager()
        .set_elevation(&token, None)
        .await
        .map_err(|e| {
            tracing::error!("Could not drop an elevation: {}", e);
            crate::auth::error::AuthError::Internal("could not drop the elevation".into())
        })?;

    Ok(elevation_answer(
        style,
        request.redirect.as_deref(),
        "elevation_dropped",
        serde_json::json!({ "elevated": false }),
    ))
}

/// Answer an elevation call the way it was asked: a redirect for a form, JSON
/// for a JSON body. The shape [`delegation_answer`] uses, for the same reason.
pub(super) fn elevation_answer(
    style: RequestStyle,
    redirect: Option<&str>,
    notice: &str,
    body: serde_json::Value,
) -> Response {
    match style {
        RequestStyle::Form => {
            let target = safe_redirect_target(redirect.or(Some(ACCOUNT_PATH)));
            let separator = if target.contains('?') { '&' } else { '?' };
            Redirect::to(&format!("{}{}notice={}", target, separator, notice)).into_response()
        }
        RequestStyle::Json => axum::Json(body).into_response(),
    }
}
