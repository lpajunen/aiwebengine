//! A person's secret for an app, entered on the engine's own page.
//!
//! An app links here with `secretStorage.entryUrl(name)` rather than taking
//! the value in its own form, so the value goes straight to `user_secrets`
//! and no script ever holds it. A run still reaches it only as
//! `{{secret:...}}` in a `fetch`.
//!
//! The page asks only for a name the app declared in its `init()`
//! (`crate::declared_secrets`), shown under the app's own label, so a link
//! cannot make it ask for anything the app did not. It never shows a stored
//! value back: it says whether one is stored, and lets the person replace or
//! remove it.

use super::*;
use crate::auth::AuthManager;
use crate::engine_page::Width;
use crate::repository::Repository;
use crate::security::client_ip;
use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize, Default)]
pub struct SecretParams {
    pub(super) script: Option<String>,
    pub(super) name: Option<String>,
    /// Where to send the person once they are done. Only a path on this
    /// engine.
    pub(super) redirect: Option<String>,
    /// What the last post did, as one of a fixed set of codes.
    pub(super) notice: Option<String>,
}

/// What a person posted from the secret page.
#[derive(Debug, Deserialize, Default)]
pub struct SecretRequest {
    pub script: Option<String>,
    pub name: Option<String>,
    pub value: Option<String>,
    /// Present to remove the stored value rather than store one.
    pub remove: Option<String>,
    pub csrf_token: Option<String>,
    pub redirect: Option<String>,
}

/// The app and the secret a request names, if the app declared it: the
/// script, the name, and the app's label for it.
fn declared(script: Option<&str>, name: Option<&str>) -> Option<(String, String, String)> {
    let script = script.map(str::trim).filter(|s| !s.is_empty())?;
    let name = name.map(str::trim).filter(|s| !s.is_empty())?;
    let label = crate::declared_secrets::label_of(script, name)?;
    Some((script.to_string(), name.to_string(), label))
}

/// The page itself, with `back` carried on, after a post that did `notice`.
fn page_url(script: &str, name: &str, back: Option<&str>, notice: &str) -> String {
    let mut url = crate::declared_secrets::entry_url(script, name);
    if let Some(back) = back {
        url.push_str(&format!("&redirect={}", urlencoding::encode(back)));
    }
    if !notice.is_empty() {
        url.push_str(&format!("&notice={}", notice));
    }
    url
}

/// What a notice code says. Fixed, so nothing a caller sends reaches the page.
fn notice_text(code: &str) -> Option<(&'static str, &'static str)> {
    match code {
        "saved" => Some(("aw-notice--ok", "Stored.")),
        "removed" => Some(("aw-notice--ok", "Removed.")),
        "empty" => Some(("aw-notice--warn", "Enter the value first.")),
        "too_long" => Some(("aw-notice--warn", "That value is too long.")),
        "expired" => Some((
            "aw-notice--warn",
            "The form had expired. Nothing was changed; try again.",
        )),
        _ => None,
    }
}

const NOT_ASKED: &str = "This app does not ask for that key.";

/// Ask somebody for one of their keys for an app, or let them change it.
#[utoipa::path(
    get,
    path = "/auth/secret",
    tags = ["Authentication"],
    params(
        ("script" = String, Query, description = "URI of the app asking"),
        ("name" = String, Query, description = "The secret's name, as the app declared it"),
        ("redirect" = Option<String>, Query, description = "Path on this engine to return to"),
    ),
    responses(
        (status = 200, description = "Secret page HTML", content_type = "text/html"),
        (status = 302, description = "No session; redirected to the sign-in page"),
        (status = 404, description = "The app does not declare that secret"),
    )
)]
pub async fn secret_page(
    State(auth_manager): State<Arc<AuthManager>>,
    Query(params): Query<SecretParams>,
    headers: HeaderMap,
) -> Response {
    let Some((script, name, label)) = declared(params.script.as_deref(), params.name.as_deref())
    else {
        return (StatusCode::NOT_FOUND, NOT_ASKED).into_response();
    };
    let back = params
        .redirect
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(|value| safe_redirect_target(Some(value)));

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
        // Back here after signing in, with the way back carried through.
        let here = page_url(&script, &name, back.as_deref(), "");
        return Redirect::to(&format!(
            "/auth/login?redirect={}",
            urlencoding::encode(&here)
        ))
        .into_response();
    };

    let nonce = crate::security::generate_nonce();
    // Bound to the person: this form writes to their account.
    let csrf_token = auth_manager
        .security_context()
        .csrf
        .generate_token(Some(session.user_id.clone()))
        .await
        .token;

    // Whether one is stored, and nothing more: the value is read only to
    // learn that it exists, and dropped.
    let stored = crate::repository::get_repository()
        .get_user_secret(&script, &session.user_id, &name)
        .await
        .ok()
        .flatten()
        .is_some();

    let notice = params
        .notice
        .as_deref()
        .and_then(notice_text)
        .map(|(class, text)| format!(r#"<p class="aw-notice {class}" role="status">{text}</p>"#))
        .unwrap_or_default();

    let hidden = format!(
        r#"<input type="hidden" name="csrf_token" value="{csrf}">
            <input type="hidden" name="script" value="{script}">
            <input type="hidden" name="name" value="{name}">
            <input type="hidden" name="redirect" value="{redirect}">"#,
        csrf = html_attribute(&csrf_token),
        script = html_attribute(&script),
        name = html_attribute(&name),
        redirect = html_attribute(back.as_deref().unwrap_or("")),
    );

    let remove = if stored {
        format!(
            r#"<form method="post" action="/auth/secret">
            {hidden}
            <input type="hidden" name="remove" value="1">
            <button type="submit" class="aw-button--danger aw-button--small">Remove it</button>
        </form>"#
        )
    } else {
        String::new()
    };

    let body = format!(
        r#"<h1>{label}</h1>
        <p class="aw-identity"><strong>{script_text}</strong> asks for your
            <strong>{label}</strong>.</p>
        <p class="aw-explain">It is kept against your account, encrypted, for this app only.
        Only this app's requests use it, sent where the app sends them. No app can read it back,
        and this page never shows it again.</p>
        {notice}
        <p class="aw-explain">{state}</p>
        <form method="post" action="/auth/secret" class="aw-form">
            {hidden}
            <label class="aw-field">{label}
                <input type="password" name="value" autocomplete="off" spellcheck="false" required>
            </label>
            <button type="submit">{save}</button>
        </form>
        {remove}
        <p class="aw-small"><a href="{back_href}">{back_label}</a></p>"#,
        label = html_escape::encode_text(&label),
        script_text = html_escape::encode_text(&script),
        state = if stored {
            "You have one stored. Entering a new one replaces it."
        } else {
            "None is stored yet."
        },
        save = if stored { "Replace" } else { "Store" },
        back_href = html_attribute(back.as_deref().unwrap_or(ACCOUNT_PATH)),
        back_label = if back.is_some() {
            "Go back"
        } else {
            "Back to your account"
        },
    );

    signed_in_page_response(&label, Width::Narrow, &body, &nonce)
}

/// Store, replace or remove one of the person's secrets for an app.
///
/// A form post only, with a CSRF token bound to the person: this writes a
/// credential to their account, and a JSON body would be a way to do that
/// without the page.
#[utoipa::path(
    post,
    path = "/auth/secret",
    tags = ["Authentication"],
    responses(
        (status = 303, description = "Stored or removed; redirected back"),
        (status = 400, description = "Not a form post"),
        (status = 401, description = "No session"),
        (status = 404, description = "The app does not declare that secret"),
    )
)]
pub async fn secret_route(
    State(auth_manager): State<Arc<AuthManager>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, AuthErrorResponse> {
    let (request, style) = parse_auth_body::<SecretRequest>(&headers, &body);
    if style != RequestStyle::Form {
        return Ok((StatusCode::BAD_REQUEST, "Use the page's form").into_response());
    }
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

    let Some((script, name, _)) = declared(request.script.as_deref(), request.name.as_deref())
    else {
        return Ok((StatusCode::NOT_FOUND, NOT_ASKED).into_response());
    };
    let back = request
        .redirect
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .map(|value| safe_redirect_target(Some(value)));
    let again = |notice: &str| {
        Redirect::to(&page_url(&script, &name, back.as_deref(), notice)).into_response()
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
        return Ok(again("expired"));
    }

    let repo = crate::repository::get_repository();
    let action = if request.remove.is_some() {
        repo.remove_user_secret(&script, &session.user_id, &name)
            .await
            .map_err(|e| {
                tracing::error!("Could not remove a person's secret: {}", e);
                crate::auth::error::AuthError::Internal("could not remove the key".into())
            })?;
        "remove_user_secret"
    } else {
        let value = request.value.as_deref().map(str::trim).unwrap_or_default();
        if value.is_empty() {
            return Ok(again("empty"));
        }
        if value.len() > crate::repository::MAX_STORAGE_VALUE_BYTES {
            return Ok(again("too_long"));
        }
        repo.set_user_secret(&script, &session.user_id, &name, value)
            .await
            .map_err(|e| {
                tracing::error!("Could not store a person's secret: {}", e);
                crate::auth::error::AuthError::Internal("could not store the key".into())
            })?;
        "store_user_secret"
    };

    // Which key, never what it is.
    let event = crate::security::audit::SecurityEvent::by(
        crate::security::audit::SecurityEventType::SystemSecurityEvent,
        crate::security::audit::SecuritySeverity::Low,
        crate::security::audit::Actor::of_session(&session),
    )
    .with_resource("user_secret".to_string())
    .with_action(action.to_string())
    .with_detail("script_uri", &script)
    .with_detail("name", &name);
    let pool = crate::database::get_global_database().map(|db| db.pool().clone());
    crate::security::audit::SecurityAuditor::new(pool)
        .log_event(event)
        .await;

    let removed = action == "remove_user_secret";
    Ok(match back.as_deref() {
        Some(target) => Redirect::to(target).into_response(),
        None => again(if removed { "removed" } else { "saved" }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_declared_secret_is_asked_for() {
        crate::declared_secrets::declare("secret-page-test", "KEY", "A key").unwrap();
        assert_eq!(
            declared(Some("secret-page-test"), Some(" KEY ")),
            Some((
                "secret-page-test".to_string(),
                "KEY".to_string(),
                "A key".to_string()
            ))
        );
        assert_eq!(declared(Some("secret-page-test"), Some("OTHER")), None);
        assert_eq!(declared(Some("another-script"), Some("KEY")), None);
        assert_eq!(declared(None, Some("KEY")), None);
    }

    #[test]
    fn a_notice_is_one_of_a_fixed_few() {
        assert!(notice_text("saved").is_some());
        assert!(notice_text("<script>").is_none());
    }

    #[test]
    fn the_page_carries_the_way_back() {
        assert_eq!(
            page_url("app", "KEY", Some("/app?tab=agents"), "saved"),
            "/auth/secret?script=app&name=KEY&redirect=%2Fapp%3Ftab%3Dagents&notice=saved"
        );
    }
}
