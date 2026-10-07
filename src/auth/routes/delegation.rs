//! Consenting to background work: the delegate page and its routes.

use super::*;
use crate::auth::AuthManager;
use crate::engine_page::Width;
use crate::security::client_ip;
use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use std::sync::Arc;

pub(super) fn render_delegations(
    csrf_token: &str,
    grants: &[crate::delegation::Grant],
    links: &[crate::delegation::ChannelIdentity],
) -> String {
    if grants.is_empty() {
        return String::new();
    }

    let now = chrono::Utc::now();
    let rows = grants
        .iter()
        .map(|grant| {
            // The consent page cannot produce this — ticking nothing there
            // is a refusal, and withdraws instead — so an empty row means a
            // grant recorded some other way. "Nothing" would still be the
            // wrong word for it: reading is the floor of a delegation, and
            // what this row actually authorises is a run that can read and
            // change nothing.
            let scopes = if grant.scopes.is_empty() {
                "read only".to_string()
            } else {
                grant
                    .scopes
                    .iter()
                    .map(|scope| html_escape::encode_text(scope.describe()).to_string())
                    .collect::<Vec<_>>()
                    .join("; ")
            };

            // A lapsed grant is shown rather than hidden: it is still a row
            // somebody may want gone, and "it expired" is information.
            let when = if grant.is_live(now) {
                format!("until {}", page_timestamp_utc(grant.expires_at))
            } else {
                format!("expired {}", page_timestamp_utc(grant.expires_at))
            };

            // The senders that can set this one going, each unlinkable on
            // its own. Shown under the grant rather than in a list of their
            // own because a link means nothing without the grant above it,
            // and reading them apart would invite withdrawing the wrong one.
            let senders = links
                .iter()
                .filter(|link| link.script_uri == grant.script_uri)
                .map(|link| {
                    format!(
                        r#"<li>
                    <div>
                        <span class="aw-row-title">{identity}</span>
                        <span class="aw-row-meta">can start this on {channel}</span>
                    </div>
                    <form method="post" action="/auth/delegations/unlink">
                        <input type="hidden" name="csrf_token" value="{csrf}">
                        <input type="hidden" name="script" value="{script_value}">
                        <input type="hidden" name="channel" value="{channel_value}">
                        <input type="hidden" name="identity" value="{identity_value}">
                        <button type="submit" class="aw-button--danger aw-button--small">Unlink</button>
                    </form>
                </li>"#,
                        identity = html_escape::encode_text(&link.identity),
                        channel = html_escape::encode_text(&link.channel),
                        identity_value = html_attribute(&link.identity),
                        channel_value = html_attribute(&link.channel),
                        script_value = html_attribute(&link.script_uri),
                        csrf = html_attribute(csrf_token),
                    )
                })
                .collect::<Vec<_>>()
                .join("\n                ");

            let senders = if senders.is_empty() {
                String::new()
            } else {
                format!(
                    "\n            <ul class=\"aw-rows\">\n                {}\n            </ul>",
                    senders
                )
            };

            format!(
                r#"<li>
                <div>
                    <span class="aw-row-title">{script}</span>
                    <span class="aw-row-meta">{scopes} · {when}</span>
                </div>
                <form method="post" action="/auth/delegations/revoke">
                    <input type="hidden" name="csrf_token" value="{csrf}">
                    <input type="hidden" name="script" value="{script_value}">
                    <button type="submit" class="aw-button--danger aw-button--small">Withdraw</button>
                </form>
            </li>{senders}"#,
                script = html_escape::encode_text(&grant.script_uri),
                script_value = html_attribute(&grant.script_uri),
                scopes = scopes,
                when = when,
                csrf = html_attribute(csrf_token),
                senders = senders,
            )
        })
        .collect::<Vec<_>>()
        .join("\n            ");

    format!(
        r#"<h2>Apps acting for you</h2>
        <p class="aw-explain">These can work on your behalf while you are away. Withdrawing one stops
        it, cancels whatever it had queued, and unlinks every sender that could start it.</p>
        <ul class="aw-rows">
            {rows}
        </ul>"#,
        rows = rows,
    )
}

#[derive(Deserialize, Default)]
pub struct DelegateParams {
    pub(super) script: Option<String>,
    /// Where to send the person once they have decided. Only a path on this
    /// engine, so the form cannot be used to bounce somebody off-site.
    pub(super) redirect: Option<String>,
    /// An invitation to link a sender, minted by a script in reply to a
    /// message it received (`delegation::invite_link`).
    ///
    /// A token rather than the sender itself, and that is the security of
    /// the whole scheme: `?channel=telegram&identity=12345` would be a URL
    /// anybody could construct for anybody, and linking somebody else's
    /// sender before they do intercepts their messages. The token is
    /// delivered into the sender's own chat, so reaching this page for a
    /// sender means being able to read that sender's messages.
    ///
    /// It rides along with the grant so that the person approves "this app,
    /// these scopes, triggered by this sender" as one decision rather than
    /// being asked a second question whose stakes they cannot judge.
    pub(super) link: Option<String>,
}

/// Ask somebody to authorise a script to act for them.
///
/// The page states what is being asked for in the person's terms, not the
/// developer's, and every scope shown is one the engine actually gates on —
/// a name nothing checks would be a promise nobody keeps.
#[utoipa::path(
    get,
    path = "/auth/delegate",
    tags = ["Authentication"],
    params(
        ("script" = String, Query, description = "URI of the script asking"),
        ("redirect" = Option<String>, Query, description = "Path on this engine to return to"),
    ),
    responses(
        (status = 200, description = "Consent page HTML", content_type = "text/html"),
        (status = 302, description = "No session; redirected to the sign-in page"),
        (status = 400, description = "Missing or unusable parameter"),
    )
)]
pub async fn delegate_page(
    State(auth_manager): State<Arc<AuthManager>>,
    Query(params): Query<DelegateParams>,
    headers: HeaderMap,
) -> Response {
    let config = auth_manager.config();
    let ip_addr = client_ip::from_headers(&headers);
    let user_agent = client_ip::user_agent_from_headers(&headers);
    let host = get_request_host(&headers);

    // The invitation, read before anything else, because it is what names
    // the script when the URL does not. A link a bot sends into a chat
    // carries only the token — the sender is deliberately not in it — so
    // insisting on `?script=` here would refuse every real invitation.
    //
    // Reading never spends: the sign-in redirect, the back button and a
    // reload all reach this before anybody has agreed to anything.
    let invitation = match params.link.as_deref() {
        Some(token) => crate::delegation::peek_invite(token).await,
        None => None,
    };

    // The query string wins when it names one, so the page shows what the
    // URL asked for; the invitation fills in when it does not.
    let script = match params.script.clone().filter(|s| !s.trim().is_empty()) {
        Some(named) => named,
        None => match invitation.as_ref() {
            Some((for_script, _, _)) => for_script.clone(),
            None => {
                return (StatusCode::BAD_REQUEST, "A script must be named").into_response();
            }
        },
    };

    // Which makes this the place the two are held to each other: an
    // invitation naming a different script than the page is showing is
    // dropped rather than honoured. The token carries its own script
    // precisely so that editing the query string cannot point one at a
    // different solution.
    let invitation = invitation.filter(|(for_script, _, _)| *for_script == script);

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
        // Back here after signing in, so the script's link works for somebody
        // whose session has aged out.
        // Carrying the invitation through the sign-in, so somebody arriving
        // from a chat with no session does not lose what they came to link.
        // The token is not spent by being shown, so it survives the round
        // trip.
        let here = match params.link.as_deref() {
            Some(token) => crate::delegation::link_url(token),
            None => crate::delegation::consent_url(&script),
        };
        return Redirect::to(&format!(
            "/auth/login?redirect={}",
            urlencoding::encode(&here)
        ))
        .into_response();
    };

    let nonce = crate::security::generate_nonce();
    // Bound to the user: an unbound token is one anybody can fetch with no
    // browser and no account, and this form is a grant of authority.
    let csrf_token = auth_manager
        .security_context()
        .csrf
        .generate_token(Some(session.user_id.clone()))
        .await
        .token;

    let existing = crate::delegation::get(&session.user_id, &script)
        .await
        .ok()
        .flatten();

    let checkboxes = crate::delegation::Scope::all()
        .iter()
        .map(|scope| {
            let already = existing.as_ref().is_some_and(|grant| grant.allows(*scope));
            // A scope the account's roles cannot confer is shown disabled
            // with the reason rather than hidden — the elevation page's rule,
            // and the same argument: "your account is not an administrator"
            // is the useful answer, where a missing checkbox is somebody
            // wondering what the app asked for.
            let reachable = match scope.required_role() {
                Some(crate::user_repository::UserRole::Administrator) => session.is_admin,
                Some(crate::user_repository::UserRole::Editor) => {
                    session.is_editor || session.is_admin
                }
                _ => true,
            };
            let note = match (reachable, scope.required_role()) {
                (true, _) => String::new(),
                (false, Some(crate::user_repository::UserRole::Administrator)) => {
                    r#" <span class="aw-muted">— your account is not an administrator</span>"#
                        .to_string()
                }
                (false, _) => {
                    r#" <span class="aw-muted">— your account cannot author scripts</span>"#
                        .to_string()
                }
            };
            format!(
                r#"<label class="aw-choice">
                    <input type="checkbox" name="scope" value="{value}"{checked}{disabled}>
                    {description}{note}
                </label>"#,
                value = html_attribute(scope.as_str()),
                checked = if already && reachable { " checked" } else { "" },
                disabled = if reachable { "" } else { " disabled" },
                description = html_escape::encode_text(scope.describe()),
                note = note,
            )
        })
        .collect::<Vec<_>>()
        .join("\n                ");

    // The sender, when one came along, and who already holds it.
    //
    // A pair already linked to somebody else is shown rather than silently
    // dropped: the person is about to press a button that will not do what
    // the page implies, and "another account has this" is the only useful
    // thing to say.
    let sender = invitation.map(|(_, channel, identity)| (channel, identity));
    let sender_owner = match &sender {
        Some((channel, identity)) => crate::delegation::resolve_channel(&script, channel, identity)
            .await
            .ok()
            .flatten(),
        None => None,
    };

    let (sender_note, sender_fields) = match &sender {
        None => (String::new(), String::new()),
        // Linked to somebody else: say so, and carry no fields, so pressing
        // the button records the grant and attempts no link.
        Some(_)
            if sender_owner
                .as_deref()
                .is_some_and(|owner| owner != session.user_id) =>
        {
            (
                r#"<p class="aw-explain">Another account has already linked this sender to this app,
        so it cannot be linked to yours. Authorising below still works; messages from that
        sender will not reach you.</p>"#
                    .to_string(),
                String::new(),
            )
        }
        Some((channel, identity)) => (
            format!(
                r#"<p class="aw-explain">It will also be able to start work for you when
        <strong>{identity}</strong> messages it on <strong>{channel}</strong>. Only that sender,
        and only this app.</p>"#,
                identity = html_escape::encode_text(identity),
                channel = html_escape::encode_text(channel),
            ),
            format!(
                r#"<input type="hidden" name="link" value="{link}">"#,
                link = html_attribute(params.link.as_deref().unwrap_or_default()),
            ),
        ),
    };

    let body = format!(
        r#"<h1>Authorise an app</h1>
        <p class="aw-identity"><strong>{script}</strong> is asking to work on your behalf
            while you are away.</p>
        <p class="aw-explain">It can already do these things while you are using it. This lets it
        carry on after you close the page — for example to finish something long, or to check
        for you on a schedule. You can withdraw it at any time from your account page.</p>
        {sender_note}
        <form method="post" action="/auth/delegate">
            <input type="hidden" name="csrf_token" value="{csrf}">
            <input type="hidden" name="script" value="{script_value}">
            <input type="hidden" name="redirect" value="{redirect}">
            {sender_fields}
            {checkboxes}
            <label class="aw-field">Stop after
                <select name="days">
                    <option value="1">1 day</option>
                    <option value="7">7 days</option>
                    <option value="30" selected>30 days</option>
                    <option value="90">90 days</option>
                </select>
            </label>
            <button type="submit">Authorise</button>
        </form>
        <p class="aw-small"><a href="/auth/account">Not now — back to your account</a></p>"#,
        script = html_escape::encode_text(&script),
        script_value = html_attribute(&script),
        redirect = html_attribute(params.redirect.as_deref().unwrap_or(ACCOUNT_PATH)),
        csrf = html_attribute(&csrf_token),
        checkboxes = checkboxes,
        sender_note = sender_note,
        sender_fields = sender_fields,
    );

    signed_in_page_response("Authorise an app", Width::Narrow, &body, &nonce)
}

/// What a person posted from the consent page.
///
/// The ticked scopes are deliberately *not* a field here. A form sends one
/// `scope=` per checkbox, and `serde_urlencoded` does not deserialise repeated
/// keys into a `Vec` — it fails the whole body, which `parse_auth_body`'s
/// `unwrap_or_default()` then turns into an empty request. The symptom was a
/// consent form that answered 303 and recorded nothing, because the *script
/// name* had been lost along with the scopes.
///
/// So the scopes are read from the body directly by [`scopes_from_body`], and
/// leaving them out of this struct is what keeps the rest of it parseable.
#[derive(Debug, Deserialize, Default)]
pub struct DelegateRequest {
    pub script: Option<String>,
    pub days: Option<i64>,
    pub csrf_token: Option<String>,
    pub redirect: Option<String>,
    /// The invitation the consent page carried, when somebody arrived from a
    /// chat. Spent here, so it links once and no more.
    pub link: Option<String>,
}

/// Which scopes the person ticked.
///
/// Handles both shapes the endpoint accepts: a form's repeated `scope=` pairs,
/// and a JSON body's `"scope": [...]`. Unknown names are dropped rather than
/// carried, because a scope nothing gates on would be a promise the engine
/// does not keep.
///
/// An empty answer is a refusal, not a grant of nothing — see the caller.
pub(super) fn scopes_from_body(style: RequestStyle, body: &[u8]) -> Vec<crate::delegation::Scope> {
    let names: Vec<String> = match style {
        RequestStyle::Form => url::form_urlencoded::parse(body)
            .filter(|(key, _)| key == "scope")
            .map(|(_, value)| value.into_owned())
            .collect(),
        RequestStyle::Json => serde_json::from_slice::<serde_json::Value>(body)
            .ok()
            .and_then(|value| {
                value.get("scope").and_then(|scope| {
                    scope.as_array().map(|items| {
                        items
                            .iter()
                            .filter_map(|item| item.as_str().map(str::to_string))
                            .collect()
                    })
                })
            })
            .unwrap_or_default(),
    };

    let mut scopes: Vec<crate::delegation::Scope> = names
        .iter()
        .filter_map(|name| crate::delegation::Scope::parse(name))
        .collect();
    scopes.sort();
    scopes.dedup();
    scopes
}

/// What a person posted to withdraw one.
#[derive(Debug, Deserialize, Default)]
pub struct RevokeDelegationRequest {
    pub script: Option<String>,
    pub csrf_token: Option<String>,
    pub redirect: Option<String>,
}

/// What a person posted to unlink one sender, leaving the grant alone.
///
/// Separate from withdrawing, because they are different decisions: "stop
/// this app acting for me" and "stop *that* sender being able to start it".
/// Somebody who changed phone number wants the second and not the first.
#[derive(Debug, Deserialize, Default)]
pub struct UnlinkSenderRequest {
    pub script: Option<String>,
    pub channel: Option<String>,
    pub identity: Option<String>,
    pub csrf_token: Option<String>,
    pub redirect: Option<String>,
}

/// Record that this person authorises a script to act for them.
///
/// The three things that make this a grant rather than a setting: it is posted
/// by the person themselves from a page that said what was being asked, its
/// CSRF token is bound to them specifically, and it carries an expiry there is
/// no way to opt out of.
#[utoipa::path(
    post,
    path = "/auth/delegate",
    tags = ["Authentication"],
    responses(
        (status = 200, description = "The grant that was recorded"),
        (status = 302, description = "Form post; redirected back with a notice"),
        (status = 401, description = "No session"),
    )
)]
pub async fn delegate_route(
    State(auth_manager): State<Arc<AuthManager>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, AuthErrorResponse> {
    let (request, style) = parse_auth_body::<DelegateRequest>(&headers, &body);
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

    let Some(script) = request
        .script
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    else {
        return Ok(redirect_to_form_with_error(
            &crate::auth::error::AuthError::Internal("no script was named".to_string()),
            request.redirect.as_deref().or(Some(ACCOUNT_PATH)),
        ));
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
        return Ok(redirect_to_form_with_error(
            &crate::auth::error::AuthError::CsrfValidationFailed,
            Some(&crate::delegation::consent_url(script)),
        ));
    }

    let scopes = scopes_from_body(style, &body);

    // Ticking nothing is a refusal. Recording an empty grant would leave a row
    // saying this script may act for them, which is the opposite of what they
    // just said.
    if scopes.is_empty() {
        let removed = crate::delegation::revoke(&session.user_id, script)
            .await
            .unwrap_or(false);
        return Ok(delegation_answer(
            style,
            request.redirect.as_deref(),
            if removed {
                "delegation_withdrawn"
            } else {
                "delegation_declined"
            },
            serde_json::json!({ "granted": false, "script": script }),
        ));
    }

    let duration = crate::delegation::bounded_duration(request.days);
    let grant = crate::delegation::grant(&session.user_id, script, &scopes, duration)
        .await
        .map_err(|e| {
            tracing::error!("Could not record a delegation: {}", e);
            crate::auth::error::AuthError::Internal("could not record the authorisation".into())
        })?;

    // The link, if one came with it. After the grant rather than before,
    // because a link without a grant authorises nothing and would be a row
    // saying a sender may trigger work that does not exist.
    //
    // A refusal here does not fail the request. The person came to authorise
    // an app and that happened; the pair being taken by another account is
    // something the page already warned about, and turning it into a 500
    // would throw away a grant they meant to give. It is reported in the
    // answer instead.
    let linked = match request.link.as_deref() {
        // Spent here and nowhere else. Single use, so a link forwarded on
        // after it has been used links nothing, and the delete that reads it
        // is what stops two browsers racing on the same one.
        Some(token) => match crate::delegation::spend_invite(token).await {
            // An invitation for a different script than the form names is
            // refused rather than honoured: the token carries its own script
            // precisely so the two cannot be made to disagree.
            Some((for_script, channel, identity)) if for_script == script => {
                match crate::delegation::bind_channel(&session.user_id, script, &channel, &identity)
                    .await
                {
                    Ok(_) => true,
                    Err(refusal) => {
                        tracing::warn!(
                            script = %script,
                            "Could not link a sender alongside a delegation: {}",
                            refusal
                        );
                        false
                    }
                }
            }
            _ => false,
        },
        None => false,
    };

    Ok(delegation_answer(
        style,
        request.redirect.as_deref(),
        "delegation_granted",
        serde_json::json!({
            "granted": true,
            "linked": linked,
            "script": grant.script_uri,
            "scopes": grant.scopes.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
            "expiresAt": grant.expires_at.to_rfc3339(),
        }),
    ))
}

/// Withdraw one, and cancel what it had queued.
#[utoipa::path(
    post,
    path = "/auth/delegations/revoke",
    tags = ["Authentication"],
    responses(
        (status = 200, description = "Whether anything was withdrawn"),
        (status = 302, description = "Form post; redirected back with a notice"),
        (status = 401, description = "No session"),
    )
)]
pub async fn revoke_delegation_route(
    State(auth_manager): State<Arc<AuthManager>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, AuthErrorResponse> {
    let (request, style) = parse_auth_body::<RevokeDelegationRequest>(&headers, &body);
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
            request.redirect.as_deref().or(Some(ACCOUNT_PATH)),
        ));
    }

    // Scoped to the caller's own user id in the statement rather than by a
    // check before it, the way the session listing is: another account's grant
    // is not addressable from here at all.
    let withdrawn = match request
        .script
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(script) => crate::delegation::revoke(&session.user_id, script)
            .await
            .unwrap_or(false),
        None => {
            crate::delegation::revoke_all(&session.user_id)
                .await
                .unwrap_or(0)
                > 0
        }
    };

    Ok(delegation_answer(
        style,
        request.redirect.as_deref(),
        if withdrawn {
            "delegation_withdrawn"
        } else {
            "delegation_missing"
        },
        serde_json::json!({ "withdrawn": withdrawn }),
    ))
}

/// Stop one sender being able to start this person's delegated work, leaving
/// the grant itself alone.
#[utoipa::path(
    post,
    path = "/auth/delegations/unlink",
    tags = ["Authentication"],
    responses(
        (status = 200, description = "Whether anything was unlinked"),
        (status = 302, description = "Form post; redirected back with a notice"),
        (status = 401, description = "No session"),
    )
)]
pub async fn unlink_sender_route(
    State(auth_manager): State<Arc<AuthManager>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, AuthErrorResponse> {
    let (request, style) = parse_auth_body::<UnlinkSenderRequest>(&headers, &body);
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
            request.redirect.as_deref().or(Some(ACCOUNT_PATH)),
        ));
    }

    let script = request
        .script
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());

    // Scoped to the caller's own user id inside the statement, as the grant
    // revocation is: another account's link is not addressable from here
    // however this is called.
    // Unlinking names the pair directly rather than through an invitation,
    // and needs no proof of owning the sender: the delete is scoped to the
    // caller's own user id, so the worst a made-up pair does is match
    // nothing. Requiring a token here would mean somebody could only unlink
    // a sender they could still receive messages from, which is backwards —
    // losing access to the chat is the commonest reason to want this.
    let sender = match (request.channel.as_deref(), request.identity.as_deref()) {
        (Some(channel), Some(identity)) => {
            crate::delegation::normalize_channel(channel, identity).ok()
        }
        _ => None,
    };

    let unlinked = match (script, sender) {
        (Some(script), Some((channel, identity))) => {
            crate::delegation::unbind_channel(&session.user_id, script, &channel, &identity)
                .await
                .unwrap_or(false)
        }
        _ => false,
    };

    Ok(delegation_answer(
        style,
        request.redirect.as_deref(),
        if unlinked {
            "sender_unlinked"
        } else {
            "delegation_missing"
        },
        serde_json::json!({ "unlinked": unlinked }),
    ))
}

/// A browser gets the page back with a notice; an API caller gets the facts.
pub(super) fn delegation_answer(
    style: RequestStyle,
    redirect: Option<&str>,
    notice: &str,
    body: serde_json::Value,
) -> Response {
    if style == RequestStyle::Form {
        let target = match redirect {
            Some(value) if !value.trim().is_empty() => safe_redirect_target(Some(value)),
            _ => format!("{}?notice={}", ACCOUNT_PATH, notice),
        };
        return Redirect::to(&target).into_response();
    }
    Json(body).into_response()
}

#[cfg(test)]
mod delegate_body_tests {
    use super::*;
    use crate::delegation::Scope;

    /// The bug this covers: a browser sends one `scope=` per ticked checkbox,
    /// and `serde_urlencoded` cannot deserialise repeated keys into a `Vec` —
    /// it fails the *whole* body, which `parse_auth_body`'s
    /// `unwrap_or_default()` turns into an empty request. The consent form
    /// therefore answered 303 and recorded nothing, having lost the script
    /// name along with the scopes.
    ///
    /// Every test of delegation called `delegation::grant` directly, so none
    /// of them went near the form. This one is the form.
    #[test]
    fn a_form_sends_one_scope_field_per_ticked_box() {
        let body = b"csrf_token=t&script=x&scope=personal_storage&scope=secrets&days=30";
        assert_eq!(
            scopes_from_body(RequestStyle::Form, body),
            vec![Scope::PersonalStorage, Scope::Secrets]
        );
    }

    /// And the scalars beside them still parse, which is the half that was
    /// actually lost.
    #[test]
    fn the_rest_of_the_form_survives_the_repeated_field() {
        let body = b"csrf_token=t&script=x&scope=personal_storage&scope=secrets&days=30";
        let parsed: DelegateRequest =
            serde_urlencoded::from_bytes(body).expect("the body should still parse");
        assert_eq!(parsed.script.as_deref(), Some("x"));
        assert_eq!(parsed.csrf_token.as_deref(), Some("t"));
        assert_eq!(parsed.days, Some(30));
    }

    #[test]
    fn one_ticked_box_grants_one_scope() {
        assert_eq!(
            scopes_from_body(RequestStyle::Form, b"script=x&scope=secrets"),
            vec![Scope::Secrets]
        );
    }

    /// Ticking nothing sends no field at all, and that is a refusal rather
    /// than a grant of nothing — the caller turns it into a withdrawal.
    #[test]
    fn ticking_nothing_sends_nothing() {
        assert!(scopes_from_body(RequestStyle::Form, b"script=x&days=30").is_empty());
    }

    /// A name nothing gates on is dropped rather than stored.
    #[test]
    fn an_unknown_scope_never_becomes_a_grant() {
        assert!(scopes_from_body(RequestStyle::Form, b"scope=administer_everything").is_empty());
    }

    #[test]
    fn a_repeated_tick_grants_it_once() {
        assert_eq!(
            scopes_from_body(RequestStyle::Form, b"scope=secrets&scope=secrets"),
            vec![Scope::Secrets]
        );
    }

    /// The endpoint takes JSON too, where the same field is an array.
    #[test]
    fn json_sends_the_scopes_as_an_array() {
        let body = br#"{"script":"x","scope":["secrets","personal_storage"]}"#;
        assert_eq!(
            scopes_from_body(RequestStyle::Json, body),
            vec![Scope::PersonalStorage, Scope::Secrets]
        );
    }

    #[test]
    fn a_json_body_with_no_scopes_is_a_refusal_too() {
        assert!(scopes_from_body(RequestStyle::Json, br#"{"script":"x"}"#).is_empty());
    }

    /// The verb posts like any other box. Worth its own test because it is
    /// the one scope whose absence has to mean something — a form that
    /// dropped it would silently record read-only where the person ticked
    /// "change things", and a form that invented it would do the reverse.
    #[test]
    fn the_write_box_posts_like_any_other() {
        assert_eq!(
            scopes_from_body(
                RequestStyle::Form,
                b"csrf_token=t&script=x&scope=personal_storage&scope=write&days=30",
            ),
            vec![Scope::PersonalStorage, Scope::Write]
        );

        // Not ticked is read-only, not absent.
        assert_eq!(
            scopes_from_body(RequestStyle::Form, b"script=x&scope=personal_storage"),
            vec![Scope::PersonalStorage]
        );
    }

    /// Every scope the consent page renders is one the endpoint can read
    /// back. The page is built from `Scope::all()` and the endpoint parses by
    /// name, so a value whose `as_str` and `parse` disagreed would render a
    /// checkbox that silently did nothing when ticked.
    #[test]
    fn every_box_the_page_renders_round_trips_through_the_endpoint() {
        for scope in Scope::all() {
            let body = format!("script=x&scope={}", scope.as_str());
            assert_eq!(
                scopes_from_body(RequestStyle::Form, body.as_bytes()),
                vec![scope],
                "the page renders {:?} but the endpoint does not read it back",
                scope
            );
        }
    }
}
