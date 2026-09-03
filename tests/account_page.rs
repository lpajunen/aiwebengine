//! Tests for the account page — what a signed-in person can do about their own
//! way in.
//!
//! The sign-in page could not hold any of this. Everything here needs a session,
//! and changing a password needs the current one; someone looking at a sign-in
//! page has neither. So the endpoint that changes a password shipped with no
//! page that submits it, and the endpoint that attaches a first credential
//! shipped with no control at all.
//!
//! The part worth pinning hardest is the token. This page's forms act on a
//! session that already exists, so they carry a token bound to it: an unbound
//! one is something anybody can fetch from `/auth/login` with no browser and no
//! account, which would leave `SameSite=Lax` as the only thing between the
//! password form and a cross-site POST.

mod common;

use aiwebengine::auth::config::InternalAuthConfig;
use aiwebengine::auth::local::GUEST_PROVIDER;
use aiwebengine::auth::routes::render_account_forms;
use common::{AdminServer, should_skip_integration_tests};

/// Password long enough for the configured minimum in the test server.
const PASSWORD: &str = "a-perfectly-fine-password";
const NEW_PASSWORD: &str = "an-entirely-different-password";

fn config(enabled: bool) -> InternalAuthConfig {
    InternalAuthConfig {
        enabled,
        allow_registration: true,
        allow_guests: true,
        bootstrap_admin_usernames: Vec::new(),
        min_password_length: 12,
    }
}

fn unique(label: &str) -> String {
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    format!("{}{}", label, &suffix[..12])
}

// ---------------------------------------------------------------------------
// What the page offers
// ---------------------------------------------------------------------------

/// Every form here posts to an endpoint that refuses when internal credentials
/// are off. Offering a control that cannot work is worse than offering none.
#[test]
fn nothing_is_offered_when_the_engine_holds_no_credentials() {
    assert_eq!(
        render_account_forms(&config(false), "token", None, "google"),
        ""
    );
    assert_eq!(
        render_account_forms(&config(false), "token", Some("player"), "local"),
        ""
    );
}

#[test]
fn an_account_with_a_password_is_offered_a_change() {
    let html = render_account_forms(&config(true), "token", Some("player"), "local");

    assert!(html.contains(r#"action="/auth/local/password""#));
    assert!(html.contains(r#"name="current_password""#));
    assert!(html.contains(r#"name="new_password""#));
    assert!(
        !html.contains("/auth/local/claim"),
        "an account with a credential must not be offered a way to attach a second one"
    );
    assert!(
        html.contains(r#"autocomplete="current-password""#)
            && html.contains(r#"autocomplete="new-password""#),
        "a password manager should be able to tell the two fields apart"
    );
    assert!(
        html.contains(r#"minlength="12""#),
        "the browser should refuse a short password before it crosses the network"
    );
}

/// An account with no credential gets the claim form instead — the control the
/// endpoint has been waiting for since it shipped.
#[test]
fn an_account_without_a_password_is_offered_one() {
    let html = render_account_forms(&config(true), "token", None, GUEST_PROVIDER);

    assert!(html.contains(r#"action="/auth/local/claim""#));
    assert!(html.contains(r#"name="username""#));
    assert!(
        !html.contains("/auth/local/password"),
        "there is no current password to present"
    );
    assert!(
        !html.contains(r#"name="current_password""#),
        "there is no current password to present"
    );
}

/// Configuration cannot lower the floor the engine enforces, so the form must
/// not advertise a lower one either.
#[test]
fn the_password_field_never_advertises_less_than_the_floor() {
    let mut low = config(true);
    low.min_password_length = 1;
    let html = render_account_forms(&low, "token", Some("player"), "local");
    assert!(html.contains(r#"minlength="8""#));
}

#[test]
fn every_form_carries_a_csrf_token() {
    for username in [Some("player"), None] {
        let html = render_account_forms(&config(true), "csrf-token-value", username, "local");
        assert_eq!(
            html.matches(r#"name="csrf_token" value="csrf-token-value""#)
                .count(),
            html.matches("<form").count(),
            "each form needs its own token field"
        );
    }
}

/// A failed submission has to come back here to be read. The redirect target
/// each form carries is what tells the endpoint which page it was submitted
/// from.
#[test]
fn both_forms_come_back_to_this_page() {
    for username in [Some("player"), None] {
        let html = render_account_forms(&config(true), "token", username, "local");
        assert!(
            html.contains(r#"name="redirect" value="/auth/account?notice="#),
            "the form should land back on the account page"
        );
    }
}

// ---------------------------------------------------------------------------
// The page over HTTP
// ---------------------------------------------------------------------------

/// Register a fresh account and return its username and session cookie.
///
/// Its own account every time: the test administrator's password is fixed and
/// shared by every test in the process, so changing it would sign the rest of
/// the suite out.
async fn register_account(
    engine: &AdminServer,
    http: &reqwest::Client,
) -> anyhow::Result<(String, String)> {
    let username = unique("acct");
    let response = http
        .post(engine.url("/auth/local/register"))
        .json(&serde_json::json!({ "username": username, "password": PASSWORD }))
        .send()
        .await?;

    anyhow::ensure!(
        response.status().is_success(),
        "registering {} failed: {}",
        username,
        response.status()
    );

    let cookie = response
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("registration issued no session"))?;

    Ok((username, cookie))
}

/// Sign an existing account in, for the second session a password change has to
/// reach.
async fn sign_in(
    engine: &AdminServer,
    http: &reqwest::Client,
    username: &str,
    password: &str,
) -> Option<String> {
    let response = http
        .post(engine.url("/auth/local/login"))
        .json(&serde_json::json!({ "username": username, "password": password }))
        .send()
        .await
        .ok()?;

    if !response.status().is_success() {
        return None;
    }

    response
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::to_string)
}

/// The token the page wrote into its forms.
fn csrf_token_from(html: &str) -> String {
    let marker = r#"name="csrf_token" value=""#;
    let start = html.find(marker).expect("the page should carry a token") + marker.len();
    let rest = &html[start..];
    let end = rest.find('"').expect("the token should be quoted");
    rest[..end].to_string()
}

fn location_of(response: &reqwest::Response) -> String {
    response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_signed_out_caller_is_sent_to_sign_in_and_back() {
    if should_skip_integration_tests() {
        return;
    }
    let engine = AdminServer::start().await.expect("server failed to start");

    let response = engine
        .anonymous()
        .get(engine.url("/auth/account"))
        .send()
        .await
        .expect("the account page should answer");

    assert!(response.status().is_redirection());
    assert_eq!(
        location_of(&response),
        "/auth/login?redirect=%2Fauth%2Faccount",
        "someone whose session aged out should come back here after signing in"
    );

    engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_page_names_the_account_and_is_never_cached() {
    if should_skip_integration_tests() {
        return;
    }
    let engine = AdminServer::start().await.expect("server failed to start");
    let http = engine.anonymous().clone();
    let (username, cookie) = register_account(&engine, &http)
        .await
        .expect("registration should succeed");

    let response = http
        .get(engine.url("/auth/account"))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .expect("the account page should answer");

    assert_eq!(response.status(), 200);
    assert_eq!(
        response
            .headers()
            .get(reqwest::header::CACHE_CONTROL)
            .and_then(|value| value.to_str().ok()),
        Some("no-store"),
        "the page names one account; a shared cache would hand it to the next person"
    );

    let html = response.text().await.expect("the page should have a body");
    assert!(
        html.contains(&username),
        "the page should say who it is for"
    );
    assert!(html.contains(r#"action="/auth/local/password""#));

    engine.shutdown().await;
}

/// The round trip the whole page exists for, including the half that is not
/// about the new password: every other session the account had ends.
#[tokio::test(flavor = "multi_thread")]
async fn changing_a_password_works_and_ends_the_other_sessions() {
    if should_skip_integration_tests() {
        return;
    }
    let engine = AdminServer::start().await.expect("server failed to start");
    let http = engine.anonymous().clone();
    let (username, cookie) = register_account(&engine, &http)
        .await
        .expect("registration should succeed");

    // A second browser, which the change has to reach.
    let elsewhere = sign_in(&engine, &http, &username, PASSWORD)
        .await
        .expect("the account should sign in");

    let page = http
        .get(engine.url("/auth/account"))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .expect("the account page should answer")
        .text()
        .await
        .expect("the page should have a body");
    let token = csrf_token_from(&page);

    let response = http
        .post(engine.url("/auth/local/password"))
        .header(reqwest::header::COOKIE, &cookie)
        .form(&[
            ("csrf_token", token.as_str()),
            ("current_password", PASSWORD),
            ("new_password", NEW_PASSWORD),
            ("redirect", "/auth/account?notice=password"),
        ])
        .send()
        .await
        .expect("the form should be accepted");

    assert!(response.status().is_redirection());
    assert_eq!(location_of(&response), "/auth/account?notice=password");

    let fresh = response
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::to_string)
        .expect("changing a password should not sign you out of the browser you did it in");

    let still_here = http
        .get(engine.url("/auth/account"))
        .header(reqwest::header::COOKIE, &fresh)
        .send()
        .await
        .expect("the account page should answer");
    assert_eq!(still_here.status(), 200);

    let other_browser = http
        .get(engine.url("/auth/account"))
        .header(reqwest::header::COOKIE, &elsewhere)
        .send()
        .await
        .expect("the account page should answer");
    assert!(
        other_browser.status().is_redirection(),
        "a password is changed because the old one may be known; a session minted \
         under it must not keep working"
    );

    assert!(
        sign_in(&engine, &http, &username, PASSWORD).await.is_none(),
        "the old password should no longer sign in"
    );
    assert!(
        sign_in(&engine, &http, &username, NEW_PASSWORD)
            .await
            .is_some(),
        "the new one should"
    );

    engine.shutdown().await;
}

/// A wrong current password is a message to read on the page you were on, not a
/// trip to a sign-in page saying your username and password do not match.
#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_current_password_comes_back_to_this_page() {
    if should_skip_integration_tests() {
        return;
    }
    let engine = AdminServer::start().await.expect("server failed to start");
    let http = engine.anonymous().clone();
    let (_, cookie) = register_account(&engine, &http)
        .await
        .expect("registration should succeed");

    let page = http
        .get(engine.url("/auth/account"))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .expect("the account page should answer")
        .text()
        .await
        .expect("the page should have a body");
    let token = csrf_token_from(&page);

    let response = http
        .post(engine.url("/auth/local/password"))
        .header(reqwest::header::COOKIE, &cookie)
        .form(&[
            ("csrf_token", token.as_str()),
            ("current_password", "not-the-current-password"),
            ("new_password", NEW_PASSWORD),
            ("redirect", "/auth/account?notice=password"),
        ])
        .send()
        .await
        .expect("the form should be answered");

    assert!(response.status().is_redirection());
    assert_eq!(location_of(&response), "/auth/account?error=credentials");

    engine.shutdown().await;
}

/// The token has to have been issued to this account. One from the sign-in page
/// is bound to nobody, and anybody can fetch one — so it is refused, and the
/// refusal reads as an expired form rather than as JSON, because that is what
/// it looks like to the one person who will ever see it honestly.
#[tokio::test(flavor = "multi_thread")]
async fn a_token_from_the_sign_in_page_cannot_change_a_password() {
    if should_skip_integration_tests() {
        return;
    }
    let engine = AdminServer::start().await.expect("server failed to start");
    let http = engine.anonymous().clone();
    let (username, cookie) = register_account(&engine, &http)
        .await
        .expect("registration should succeed");

    let unbound = csrf_token_from(
        &http
            .get(engine.url("/auth/login"))
            .send()
            .await
            .expect("the sign-in page should answer")
            .text()
            .await
            .expect("the page should have a body"),
    );

    let response = http
        .post(engine.url("/auth/local/password"))
        .header(reqwest::header::COOKIE, &cookie)
        .form(&[
            ("csrf_token", unbound.as_str()),
            ("current_password", PASSWORD),
            ("new_password", NEW_PASSWORD),
            ("redirect", "/auth/account?notice=password"),
        ])
        .send()
        .await
        .expect("the form should be answered");

    assert_eq!(
        location_of(&response),
        "/auth/account?error=csrf",
        "a token bound to nobody must not be enough to change a password"
    );
    assert!(
        sign_in(&engine, &http, &username, PASSWORD).await.is_some(),
        "the password should be untouched"
    );

    engine.shutdown().await;
}

/// The other thing the cookie-clobbering fix protects, kept here because it is
/// the same mechanism: the session-refresh middleware re-sends the cookie on
/// the way out to slide its `Max-Age` forward, and it must not do that over a
/// cookie the handler wrote. Signing out writes an empty one with `Max-Age=0`,
/// and renewing it instead would leave the browser holding a session cookie it
/// was just told to drop.
#[tokio::test(flavor = "multi_thread")]
async fn signing_out_clears_the_cookie_rather_than_renewing_it() {
    if should_skip_integration_tests() {
        return;
    }
    let engine = AdminServer::start().await.expect("server failed to start");
    let http = engine.anonymous().clone();
    let (_, cookie) = register_account(&engine, &http)
        .await
        .expect("registration should succeed");

    let response = http
        .get(engine.url("/auth/logout"))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .expect("logout should answer");

    let set_cookie = response
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();

    assert!(
        set_cookie.contains("Max-Age=0"),
        "signing out should expire the cookie, not renew it: {}",
        set_cookie
    );
    assert!(
        !set_cookie.contains(cookie.split('=').nth(1).unwrap_or("nothing")),
        "and it should not hand the token back"
    );

    engine.shutdown().await;
}
