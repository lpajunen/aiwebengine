//! Tests for recovery codes — the way back into an account whose password
//! nobody remembers, for someone who is not the operator.
//!
//! `--set-password` needs the machine the engine runs on. That answers a
//! personal install and a deployment with an operator on hand, and it answers
//! nothing at all for a person who forgot their password on a solution they
//! merely use. These accounts carry no verified address by design, so the reset
//! link every other system sends has nowhere to go; a code written down ahead
//! of time is what stands in for it.
//!
//! What the tests are about, in order of how much they matter: a code is a
//! second credential, so it has to be single-use, bound to its account, and
//! taken away when the set is reissued.

mod common;

use aiwebengine::auth::config::InternalAuthConfig;
use aiwebengine::auth::routes::{LoginForm, render_account_forms, render_internal_auth_forms};
use common::{AdminServer, should_skip_integration_tests};

const PASSWORD: &str = "a-perfectly-fine-password";
const RECOVERED_PASSWORD: &str = "a-password-set-by-recovery";

fn config(recovery: bool) -> InternalAuthConfig {
    InternalAuthConfig {
        enabled: true,
        allow_registration: true,
        allow_guests: false,
        bootstrap_admin_usernames: Vec::new(),
        allow_recovery_codes: recovery,
        min_password_length: 12,
    }
}

fn unique(label: &str) -> String {
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    format!("{}{}", label, &suffix[..12])
}

// ---------------------------------------------------------------------------
// What the pages offer
// ---------------------------------------------------------------------------

/// The switch is off by default, and a control that posts to an endpoint which
/// would refuse is worse than no control.
#[test]
fn nothing_points_at_recovery_when_it_is_off() {
    let html = render_internal_auth_forms(
        &config(false),
        "token",
        "/after",
        "%2Fafter",
        LoginForm::SignIn,
    );
    assert!(!html.contains("recover=1"));

    let recovery_form = render_internal_auth_forms(
        &config(false),
        "token",
        "/after",
        "%2Fafter",
        LoginForm::Recover,
    );
    assert!(
        !recovery_form.contains("/auth/local/recover"),
        "asking for the form by hand must not conjure one either"
    );

    assert!(
        !render_account_forms(&config(false), "token", Some("player"), "local", Some(10))
            .contains("/auth/local/recovery_codes"),
        "nor should the account page offer to issue a set"
    );
}

/// Someone who has forgotten their password finds recovery from the sign-in
/// form, because that is where they are when they find out.
#[test]
fn the_sign_in_form_points_at_recovery() {
    let html = render_internal_auth_forms(
        &config(true),
        "token",
        "/after",
        "%2Fafter",
        LoginForm::SignIn,
    );
    assert!(html.contains("recover=1"));
}

/// Recovery is not a sign-in: the code is spent and the new password chosen in
/// the same act, so an account is never reachable by a code already shown to
/// work.
#[test]
fn the_recovery_form_takes_a_code_and_a_new_password() {
    let html = render_internal_auth_forms(
        &config(true),
        "token",
        "/after",
        "%2Fafter",
        LoginForm::Recover,
    );

    assert!(html.contains(r#"action="/auth/local/recover""#));
    assert!(html.contains(r#"name="username""#));
    assert!(html.contains(r#"name="code""#));
    assert!(html.contains(r#"name="new_password""#));
    assert!(html.contains(r#"minlength="12""#));
    assert!(
        html.contains(r#"name="csrf_token""#),
        "the form is forgeable from any page on the internet without one"
    );
    assert!(
        !html.contains("/auth/local/login"),
        "the sign-in form asks for the password this person does not have"
    );
}

/// The account page reports what can honestly be reported about codes stored as
/// hashes: how many are left.
#[test]
fn the_account_page_counts_the_codes_and_asks_for_the_password() {
    let html = render_account_forms(&config(true), "token", Some("player"), "local", Some(7));

    assert!(html.contains(r#"action="/auth/local/recovery_codes""#));
    assert!(html.contains("7 unused recovery codes"));
    assert!(
        html.contains(r#"name="current_password""#),
        "a stolen session must not be able to mint a way in that outlives a password change"
    );
    assert!(
        html.contains(r#"id="recovery_current_password""#),
        "the page has two current-password fields and they cannot share an id"
    );
}

#[test]
fn an_account_with_no_codes_is_told_what_that_means() {
    let html = render_account_forms(&config(true), "token", Some("player"), "local", Some(0));
    assert!(html.contains("no recovery codes"));
}

/// A code sets a password, so an account with no password cannot hold one — and
/// for a guest, whose account has no way in by design, a code would be one.
#[test]
fn an_account_with_no_password_is_offered_no_codes() {
    let html = render_account_forms(&config(true), "token", None, "guest", None);
    assert!(!html.contains("/auth/local/recovery_codes"));
}

// ---------------------------------------------------------------------------
// The flow over HTTP
// ---------------------------------------------------------------------------

async fn register_account(
    engine: &AdminServer,
    http: &reqwest::Client,
) -> anyhow::Result<(String, String)> {
    let username = unique("rec");
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

/// Ask for a set of codes as an API caller would, and get the only copy.
async fn issue_codes(
    engine: &AdminServer,
    http: &reqwest::Client,
    cookie: &str,
    password: &str,
) -> anyhow::Result<Vec<String>> {
    let response = http
        .post(engine.url("/auth/local/recovery_codes"))
        .header(reqwest::header::COOKIE, cookie)
        .json(&serde_json::json!({ "current_password": password }))
        .send()
        .await?;

    anyhow::ensure!(
        response.status().is_success(),
        "issuing codes failed: {}",
        response.status()
    );

    let body: serde_json::Value = response.json().await?;
    Ok(body["codes"]
        .as_array()
        .map(|codes| {
            codes
                .iter()
                .filter_map(|code| code.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default())
}

async fn recover(
    engine: &AdminServer,
    http: &reqwest::Client,
    username: &str,
    code: &str,
    new_password: &str,
) -> reqwest::Response {
    http.post(engine.url("/auth/local/recover"))
        .json(&serde_json::json!({
            "username": username,
            "code": code,
            "new_password": new_password,
        }))
        .send()
        .await
        .expect("the recovery endpoint should answer")
}

async fn signs_in(
    engine: &AdminServer,
    http: &reqwest::Client,
    username: &str,
    password: &str,
) -> bool {
    http.post(engine.url("/auth/local/login"))
        .json(&serde_json::json!({ "username": username, "password": password }))
        .send()
        .await
        .map(|response| response.status().is_success())
        .unwrap_or(false)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_set_is_ten_distinct_codes_and_needs_the_current_password() {
    if should_skip_integration_tests() {
        return;
    }
    let engine = AdminServer::start().await.expect("server failed to start");
    let http = engine.anonymous().clone();
    let (_, cookie) = register_account(&engine, &http)
        .await
        .expect("registration should succeed");

    let refused = http
        .post(engine.url("/auth/local/recovery_codes"))
        .header(reqwest::header::COOKIE, &cookie)
        .json(&serde_json::json!({ "current_password": "not-the-password" }))
        .send()
        .await
        .expect("the endpoint should answer");
    assert_eq!(
        refused.status(),
        401,
        "a session alone must not be enough to mint a second way in"
    );

    let codes = issue_codes(&engine, &http, &cookie, PASSWORD)
        .await
        .expect("issuing should succeed");

    assert_eq!(codes.len(), 10);
    let distinct: std::collections::HashSet<&String> = codes.iter().collect();
    assert_eq!(distinct.len(), 10, "each code should be its own");

    engine.shutdown().await;
}

/// The whole point: a code the account was given ahead of time sets a new
/// password and signs the person in.
#[tokio::test(flavor = "multi_thread")]
async fn a_code_sets_a_new_password_and_signs_in() {
    if should_skip_integration_tests() {
        return;
    }
    let engine = AdminServer::start().await.expect("server failed to start");
    let http = engine.anonymous().clone();
    let (username, cookie) = register_account(&engine, &http)
        .await
        .expect("registration should succeed");
    let codes = issue_codes(&engine, &http, &cookie, PASSWORD)
        .await
        .expect("issuing should succeed");

    let response = recover(&engine, &http, &username, &codes[0], RECOVERED_PASSWORD).await;
    assert_eq!(response.status(), 200);
    assert!(
        response.headers().contains_key(reqwest::header::SET_COOKIE),
        "recovery is how somebody gets back in, so it signs them in"
    );

    assert!(signs_in(&engine, &http, &username, RECOVERED_PASSWORD).await);
    assert!(
        !signs_in(&engine, &http, &username, PASSWORD).await,
        "the password that was forgotten should be gone"
    );

    // The sessions that existed when recovery ran may be exactly the ones the
    // person is trying to be rid of.
    let old_session = http
        .get(engine.url("/auth/account"))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .expect("the account page should answer");
    assert!(old_session.status().is_redirection());

    engine.shutdown().await;
}

/// A code is a credential written on paper. Once spent it is worth nothing, or
/// a copy of the paper is worth as much as the original forever.
#[tokio::test(flavor = "multi_thread")]
async fn a_code_works_once() {
    if should_skip_integration_tests() {
        return;
    }
    let engine = AdminServer::start().await.expect("server failed to start");
    let http = engine.anonymous().clone();
    let (username, cookie) = register_account(&engine, &http)
        .await
        .expect("registration should succeed");
    let codes = issue_codes(&engine, &http, &cookie, PASSWORD)
        .await
        .expect("issuing should succeed");

    assert_eq!(
        recover(&engine, &http, &username, &codes[0], RECOVERED_PASSWORD)
            .await
            .status(),
        200
    );

    let again = recover(
        &engine,
        &http,
        &username,
        &codes[0],
        "another-password-entirely",
    )
    .await;
    assert_eq!(again.status(), 401);
    assert!(
        signs_in(&engine, &http, &username, RECOVERED_PASSWORD).await,
        "and the spent code should not have changed the password a second time"
    );

    // The rest of the set is still a set: ten codes are ten emergencies.
    assert_eq!(
        recover(
            &engine,
            &http,
            &username,
            &codes[1],
            "a-third-password-here"
        )
        .await
        .status(),
        200
    );

    engine.shutdown().await;
}

/// A code identifies an account. Presented against another one it is worth
/// nothing, and the answer says nothing about which account it did belong to.
#[tokio::test(flavor = "multi_thread")]
async fn a_code_belongs_to_one_account() {
    if should_skip_integration_tests() {
        return;
    }
    let engine = AdminServer::start().await.expect("server failed to start");
    let http = engine.anonymous().clone();

    let (_, mine) = register_account(&engine, &http)
        .await
        .expect("registration should succeed");
    let (their_username, _) = register_account(&engine, &http)
        .await
        .expect("registration should succeed");
    let my_codes = issue_codes(&engine, &http, &mine, PASSWORD)
        .await
        .expect("issuing should succeed");

    let response = recover(
        &engine,
        &http,
        &their_username,
        &my_codes[0],
        RECOVERED_PASSWORD,
    )
    .await;
    assert_eq!(response.status(), 401);
    assert!(
        signs_in(&engine, &http, &their_username, PASSWORD).await,
        "their password should be untouched"
    );

    engine.shutdown().await;
}

/// Reissuing because the old codes were seen by somebody has to actually take
/// them away.
#[tokio::test(flavor = "multi_thread")]
async fn issuing_a_set_replaces_the_one_before_it() {
    if should_skip_integration_tests() {
        return;
    }
    let engine = AdminServer::start().await.expect("server failed to start");
    let http = engine.anonymous().clone();
    let (username, cookie) = register_account(&engine, &http)
        .await
        .expect("registration should succeed");

    let first = issue_codes(&engine, &http, &cookie, PASSWORD)
        .await
        .expect("issuing should succeed");
    let second = issue_codes(&engine, &http, &cookie, PASSWORD)
        .await
        .expect("reissuing should succeed");

    assert_eq!(
        recover(&engine, &http, &username, &first[0], RECOVERED_PASSWORD)
            .await
            .status(),
        401,
        "a code from the replaced set is no longer a code"
    );
    assert_eq!(
        recover(&engine, &http, &username, &second[0], RECOVERED_PASSWORD)
            .await
            .status(),
        200
    );

    engine.shutdown().await;
}

/// A code is transcribed by hand and typed back months later. What is compared
/// is what is left after the decoration.
#[tokio::test(flavor = "multi_thread")]
async fn a_code_is_accepted_however_it_was_written_down() {
    if should_skip_integration_tests() {
        return;
    }
    let engine = AdminServer::start().await.expect("server failed to start");
    let http = engine.anonymous().clone();
    let (username, cookie) = register_account(&engine, &http)
        .await
        .expect("registration should succeed");
    let codes = issue_codes(&engine, &http, &cookie, PASSWORD)
        .await
        .expect("issuing should succeed");

    let typed = format!("  {}  ", codes[0].to_uppercase().replace('-', " "));
    assert_eq!(
        recover(&engine, &http, &username, &typed, RECOVERED_PASSWORD)
            .await
            .status(),
        200
    );

    engine.shutdown().await;
}

/// The count on the account page is the only thing the engine can honestly say
/// about codes it stores as hashes, so it should be right.
#[tokio::test(flavor = "multi_thread")]
async fn the_account_page_reports_what_is_left() {
    if should_skip_integration_tests() {
        return;
    }
    let engine = AdminServer::start().await.expect("server failed to start");
    let http = engine.anonymous().clone();
    let (username, cookie) = register_account(&engine, &http)
        .await
        .expect("registration should succeed");

    let before = http
        .get(engine.url("/auth/account"))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .expect("the account page should answer")
        .text()
        .await
        .expect("the page should have a body");
    assert!(before.contains("no recovery codes"));

    let codes = issue_codes(&engine, &http, &cookie, PASSWORD)
        .await
        .expect("issuing should succeed");
    recover(&engine, &http, &username, &codes[0], RECOVERED_PASSWORD).await;

    let fresh = http
        .post(engine.url("/auth/local/login"))
        .json(&serde_json::json!({ "username": username, "password": RECOVERED_PASSWORD }))
        .send()
        .await
        .expect("signing in should answer")
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::to_string)
        .expect("the new password should sign in");

    let after = http
        .get(engine.url("/auth/account"))
        .header(reqwest::header::COOKIE, &fresh)
        .send()
        .await
        .expect("the account page should answer")
        .text()
        .await
        .expect("the page should have a body");
    assert!(
        after.contains("9 unused recovery codes"),
        "one was spent, nine are left"
    );

    engine.shutdown().await;
}

/// The switch is off by default, and off means the endpoints refuse — not that
/// the page merely stops mentioning them.
#[tokio::test(flavor = "multi_thread")]
async fn the_endpoints_refuse_when_recovery_is_off() {
    if should_skip_integration_tests() {
        return;
    }
    let engine = AdminServer::start_customized(|config| {
        if let Some(auth) = config.auth.as_mut() {
            auth.internal.allow_recovery_codes = false;
        }
    })
    .await
    .expect("server failed to start");
    let http = engine.anonymous().clone();
    let (username, cookie) = register_account(&engine, &http)
        .await
        .expect("registration should succeed");

    let issuing = http
        .post(engine.url("/auth/local/recovery_codes"))
        .header(reqwest::header::COOKIE, &cookie)
        .json(&serde_json::json!({ "current_password": PASSWORD }))
        .send()
        .await
        .expect("the endpoint should answer");
    assert_eq!(issuing.status(), 403);

    let redeeming = recover(
        &engine,
        &http,
        &username,
        "any-code-at-all",
        RECOVERED_PASSWORD,
    )
    .await;
    assert_eq!(redeeming.status(), 403);

    let page = http
        .get(engine.url("/auth/login"))
        .send()
        .await
        .expect("the sign-in page should answer")
        .text()
        .await
        .expect("the page should have a body");
    assert!(!page.contains("recover=1"));

    engine.shutdown().await;
}
