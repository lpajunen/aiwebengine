//! Tests for seeing and ending the sessions an account holds.
//!
//! Before this, the only thing a person could do about a session they did not
//! recognise was change their password — which ends every session they have,
//! including the four they wanted to keep, and which they cannot do at all if
//! they signed in through a provider. Everything needed to do better was
//! already stored; nothing read it.
//!
//! The two properties worth pinning hardest are the ones that would turn a
//! convenience into a hole: the listing must never hand back a session token,
//! since `sessions.session_id` *is* the token and a list of a person's tokens
//! would escalate one stolen session into all of them; and an id must only ever
//! name a session belonging to the account that asked.

mod common;

use common::{AdminServer, should_skip_integration_tests};

const PASSWORD: &str = "a-perfectly-fine-password";

fn unique(label: &str) -> String {
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    format!("{}{}", label, &suffix[..12])
}

async fn register_account(
    engine: &AdminServer,
    http: &reqwest::Client,
) -> anyhow::Result<(String, String)> {
    let username = unique("sess");
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

    Ok((username, cookie_from(&response).expect("a session")))
}

fn cookie_from(response: &reqwest::Response) -> Option<String> {
    response
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::to_string)
}

async fn sign_in(engine: &AdminServer, http: &reqwest::Client, username: &str) -> Option<String> {
    let response = http
        .post(engine.url("/auth/local/login"))
        .json(&serde_json::json!({ "username": username, "password": PASSWORD }))
        .send()
        .await
        .ok()?;

    response
        .status()
        .is_success()
        .then(|| cookie_from(&response))?
}

async fn list_sessions(
    engine: &AdminServer,
    http: &reqwest::Client,
    cookie: &str,
) -> serde_json::Value {
    let response = http
        .get(engine.url("/auth/sessions"))
        .header(reqwest::header::COOKIE, cookie)
        .send()
        .await
        .expect("the listing should answer");
    assert_eq!(response.status(), 200);
    response.json().await.expect("a JSON body")
}

/// Whether a cookie still reaches something that requires a session.
async fn still_valid(engine: &AdminServer, http: &reqwest::Client, cookie: &str) -> bool {
    http.get(engine.url("/auth/account"))
        .header(reqwest::header::COOKIE, cookie)
        .send()
        .await
        .map(|response| response.status() == 200)
        .unwrap_or(false)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_listing_shows_every_session_and_marks_this_one() {
    if should_skip_integration_tests() {
        return;
    }
    let engine = AdminServer::start().await.expect("server failed to start");
    let http = engine.anonymous().clone();
    let (username, first) = register_account(&engine, &http)
        .await
        .expect("registration should succeed");
    let _second = sign_in(&engine, &http, &username)
        .await
        .expect("a second sign-in should succeed");

    let body = list_sessions(&engine, &http, &first).await;
    let sessions = body["sessions"].as_array().expect("an array");

    assert_eq!(sessions.len(), 2, "both sign-ins should be listed");
    assert_eq!(
        sessions
            .iter()
            .filter(|session| session["current"] == serde_json::json!(true))
            .count(),
        1,
        "exactly one of them is the session that asked"
    );
    assert!(
        sessions
            .iter()
            .all(|session| session["ip_addr"].is_string() && session["created_at"].is_string()),
        "a session is recognised by where it came from and when it started"
    );

    engine.shutdown().await;
}

/// `sessions.session_id` is the session token. A listing that returned it would
/// turn one stolen session into every session the account has.
#[tokio::test(flavor = "multi_thread")]
async fn a_listing_never_hands_back_a_token() {
    if should_skip_integration_tests() {
        return;
    }
    let engine = AdminServer::start().await.expect("server failed to start");
    let http = engine.anonymous().clone();
    let (username, first) = register_account(&engine, &http)
        .await
        .expect("registration should succeed");
    let second = sign_in(&engine, &http, &username)
        .await
        .expect("a second sign-in should succeed");

    let body = list_sessions(&engine, &http, &first).await;
    let rendered = body.to_string();

    for cookie in [&first, &second] {
        let token = cookie.split('=').nth(1).expect("a cookie value");
        assert!(
            !rendered.contains(token),
            "the listing must not contain a session token"
        );
    }
    assert!(
        !rendered.contains("session_id"),
        "nor the field that holds one"
    );

    engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn one_session_can_be_ended_without_touching_the_others() {
    if should_skip_integration_tests() {
        return;
    }
    let engine = AdminServer::start().await.expect("server failed to start");
    let http = engine.anonymous().clone();
    let (username, mine) = register_account(&engine, &http)
        .await
        .expect("registration should succeed");
    let elsewhere = sign_in(&engine, &http, &username)
        .await
        .expect("a second sign-in should succeed");

    let body = list_sessions(&engine, &http, &mine).await;
    let other = body["sessions"]
        .as_array()
        .expect("an array")
        .iter()
        .find(|session| session["current"] == serde_json::json!(false))
        .expect("the other session should be listed")["id"]
        .as_str()
        .expect("an id")
        .to_string();

    let response = http
        .post(engine.url("/auth/sessions/revoke"))
        .header(reqwest::header::COOKIE, &mine)
        .json(&serde_json::json!({ "session": other }))
        .send()
        .await
        .expect("the revoke endpoint should answer");
    assert_eq!(response.status(), 200);

    assert!(
        !still_valid(&engine, &http, &elsewhere).await,
        "the session that was ended should be gone"
    );
    assert!(
        still_valid(&engine, &http, &mine).await,
        "and the one that ended it should not be"
    );

    engine.shutdown().await;
}

/// An id is a surrogate key, not an authorization. Naming somebody else's
/// session has to be the same answer as naming one that does not exist.
#[tokio::test(flavor = "multi_thread")]
async fn an_id_only_names_your_own_session() {
    if should_skip_integration_tests() {
        return;
    }
    let engine = AdminServer::start().await.expect("server failed to start");
    let http = engine.anonymous().clone();
    let (_, mine) = register_account(&engine, &http)
        .await
        .expect("registration should succeed");
    let (_, theirs) = register_account(&engine, &http)
        .await
        .expect("registration should succeed");

    let their_session = list_sessions(&engine, &http, &theirs).await["sessions"][0]["id"]
        .as_str()
        .expect("an id")
        .to_string();

    let response = http
        .post(engine.url("/auth/sessions/revoke"))
        .header(reqwest::header::COOKIE, &mine)
        .json(&serde_json::json!({ "session": their_session }))
        .send()
        .await
        .expect("the revoke endpoint should answer");

    assert_eq!(
        response.status(),
        404,
        "their session is not one of mine to end, and my own session is fine"
    );
    assert!(
        still_valid(&engine, &http, &theirs).await,
        "and it should still be theirs"
    );

    engine.shutdown().await;
}

/// The control for a lost device: it does not need somebody to know which row
/// is the phone, and it does not sign them out of the browser they are using.
#[tokio::test(flavor = "multi_thread")]
async fn everything_else_can_be_ended_at_once() {
    if should_skip_integration_tests() {
        return;
    }
    let engine = AdminServer::start().await.expect("server failed to start");
    let http = engine.anonymous().clone();
    let (username, mine) = register_account(&engine, &http)
        .await
        .expect("registration should succeed");
    let phone = sign_in(&engine, &http, &username)
        .await
        .expect("a second sign-in should succeed");

    let response = http
        .post(engine.url("/auth/sessions/revoke"))
        .header(reqwest::header::COOKIE, &mine)
        .json(&serde_json::json!({}))
        .send()
        .await
        .expect("the revoke endpoint should answer");
    assert_eq!(response.status(), 200);

    assert!(!still_valid(&engine, &http, &phone).await);
    assert!(still_valid(&engine, &http, &mine).await);

    let body = list_sessions(&engine, &http, &mine).await;
    assert_eq!(
        body["sessions"].as_array().expect("an array").len(),
        1,
        "one session left, and it is this one"
    );

    engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_caller_with_no_session_is_told_so() {
    if should_skip_integration_tests() {
        return;
    }
    let engine = AdminServer::start().await.expect("server failed to start");
    let http = engine.anonymous().clone();

    assert_eq!(
        http.get(engine.url("/auth/sessions"))
            .send()
            .await
            .expect("the listing should answer")
            .status(),
        401
    );
    assert_eq!(
        http.post(engine.url("/auth/sessions/revoke"))
            .json(&serde_json::json!({}))
            .send()
            .await
            .expect("the revoke endpoint should answer")
            .status(),
        401
    );

    engine.shutdown().await;
}

/// The page is where this is actually used, and its buttons carry a token bound
/// to the session — an unbound one is something anybody can fetch from
/// `/auth/login` with no account at all.
#[tokio::test(flavor = "multi_thread")]
async fn the_account_page_lists_them_and_offers_the_controls() {
    if should_skip_integration_tests() {
        return;
    }
    let engine = AdminServer::start().await.expect("server failed to start");
    let http = engine.anonymous().clone();
    let (username, mine) = register_account(&engine, &http)
        .await
        .expect("registration should succeed");
    let _phone = sign_in(&engine, &http, &username)
        .await
        .expect("a second sign-in should succeed");

    let page = http
        .get(engine.url("/auth/account"))
        .header(reqwest::header::COOKIE, &mine)
        .send()
        .await
        .expect("the account page should answer")
        .text()
        .await
        .expect("a body");

    assert!(page.contains("Where you are signed in"));
    assert!(page.contains("This session"), "one row is this one");
    assert!(page.contains(r#"action="/auth/sessions/revoke""#));
    assert!(
        page.contains("Sign out everywhere else"),
        "offered because there is something else to end"
    );

    // An unbound token, of the kind the sign-in page hands to anybody.
    let login = http
        .get(engine.url("/auth/login"))
        .send()
        .await
        .expect("the sign-in page should answer")
        .text()
        .await
        .expect("a body");
    let marker = r#"name="csrf_token" value=""#;
    let start = login.find(marker).expect("a token") + marker.len();
    let unbound = &login[start..start + login[start..].find('"').expect("a quote")];

    let refused = http
        .post(engine.url("/auth/sessions/revoke"))
        .header(reqwest::header::COOKIE, &mine)
        .header(
            reqwest::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        )
        .body(format!("csrf_token={}", unbound))
        .send()
        .await
        .expect("the revoke endpoint should answer");

    assert_eq!(
        location_of(&refused),
        "/auth/account?error=csrf",
        "a form token bound to nobody must not end anybody's sessions"
    );

    engine.shutdown().await;
}

fn location_of(response: &reqwest::Response) -> String {
    response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}
