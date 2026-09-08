//! Storing a git token, and the things that must never happen to it.
//!
//! The mapping tests live in `git_pull.rs`. What these are about is narrower
//! and harder to notice going wrong: a credential is the one value in this
//! feature that is worth stealing, and every property below is one that fails
//! silently if it regresses. A token readable from a script, or echoed back by
//! the endpoint that stored it, still passes every test about pulling.

mod common;

use aiwebengine::engine_api::execute_native_mcp_tool;
use aiwebengine::git_credentials;
use aiwebengine::security::UserContext;
use common::{setup_env, test_mutex};
use serde_json::json;

const HOST: &str = "github.com";

fn editor(user: &str) -> UserContext {
    UserContext::editor(user.to_string())
}

/// Give this process an at-rest key.
///
/// `setup_env` configures none, and storing a credential without one is refused
/// on purpose — see `storing_without_an_encryption_key_is_refused`, which is
/// the test that leaves this uncalled. nextest runs each test in its own
/// process, so the two can coexist despite the key being process-wide.
fn with_encryption() {
    use aiwebengine::security::encryption::DataEncryption;
    use std::sync::Arc;
    let key = [7u8; 32];
    aiwebengine::repository::initialize_secret_encryption(Arc::new(DataEncryption::new(&key)));
}

/// Store directly rather than through the endpoint, which would want a live
/// GitHub to verify the token against. What is under test here is the storage,
/// not the verification.
async fn store(user: &str, token: &str) {
    with_encryption();
    git_credentials::store(user, HOST, token, Some("octocat"))
        .await
        .expect("credential should store");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_stored_token_round_trips() {
    let _guard = test_mutex().lock().await;
    setup_env().await;
    with_encryption();

    let user = "cred-round-trip";
    store(user, "ghp_example_token_value").await;

    assert_eq!(
        git_credentials::token_for(user, HOST).await.as_deref(),
        Some("ghp_example_token_value"),
        "the one caller allowed to see a token gets it back"
    );
}

/// The property the whole table exists for. `user_secrets` is the JavaScript
/// `secretStorage`, so a token stored there would be readable by any script the
/// user can run; this table is not that one, and a lookup by the obvious key
/// finds nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_token_is_not_reachable_through_script_secrets() {
    let _guard = test_mutex().lock().await;
    setup_env().await;
    with_encryption();

    let user = "cred-isolation";
    store(user, "ghp_should_not_be_visible").await;

    for key in ["token", "github", "git", HOST] {
        assert_eq!(
            aiwebengine::repository::get_user_secret_item("any-script", user, key),
            None,
            "'{}' must not resolve a git token through secretStorage",
            key
        );
    }
}

/// A listing exists so somebody can tell two credentials apart and decide
/// whether one is still wanted. Nothing in it is the token, and no substring of
/// the token appears anywhere in the response.
#[tokio::test(flavor = "multi_thread")]
async fn a_listing_never_carries_the_token() {
    let _guard = test_mutex().lock().await;
    setup_env().await;
    with_encryption();

    let user = "cred-listing";
    let token = "ghp_secret_material_here";
    store(user, token).await;

    let listed = git_credentials::list(user).await.expect("should list");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].remote_host, HOST);
    assert_eq!(listed[0].account.as_deref(), Some("octocat"));

    let rendered = execute_native_mcp_tool("list_git_credentials", &json!({}), &editor(user))
        .expect("tool should run")
        .to_string();
    assert!(
        !rendered.contains(token),
        "the token must not appear in a listing: {}",
        rendered
    );
    assert!(
        rendered.contains("octocat"),
        "but the account should, so credentials can be told apart: {}",
        rendered
    );
}

/// Every request names the caller and nothing else, so there is no shape of
/// this API that reads somebody else's credential. Worth asserting rather than
/// assuming, because the storage is keyed by user and a lookup that forgot to
/// scope would still return *a* row.
#[tokio::test(flavor = "multi_thread")]
async fn one_account_cannot_reach_another_ones_token() {
    let _guard = test_mutex().lock().await;
    setup_env().await;
    with_encryption();

    store("cred-owner", "ghp_owner_token").await;

    assert_eq!(
        git_credentials::token_for("cred-intruder", HOST).await,
        None,
        "a different account has no credential of its own and inherits none"
    );

    let listed = git_credentials::list("cred-intruder")
        .await
        .expect("should list");
    assert!(listed.is_empty(), "and sees nothing in a listing");
}

#[tokio::test(flavor = "multi_thread")]
async fn replacing_a_token_keeps_when_it_was_last_used() {
    let _guard = test_mutex().lock().await;
    setup_env().await;
    with_encryption();

    let user = "cred-replace";
    store(user, "ghp_first").await;
    git_credentials::mark_used(user, HOST).await;

    let before = git_credentials::list(user).await.expect("should list")[0].last_used_at;
    assert!(before.is_some(), "using it records that it was used");

    store(user, "ghp_second").await;

    let after = git_credentials::list(user).await.expect("should list");
    assert_eq!(
        after[0].last_used_at, before,
        "rotating a token does not make the credential newly unused"
    );
    assert_eq!(
        git_credentials::token_for(user, HOST).await.as_deref(),
        Some("ghp_second"),
        "but the token itself is replaced"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn forgetting_reports_whether_there_was_anything_to_forget() {
    let _guard = test_mutex().lock().await;
    setup_env().await;
    with_encryption();

    let user = "cred-forget";
    store(user, "ghp_forget_me").await;

    assert!(
        git_credentials::forget(user, HOST)
            .await
            .expect("should remove"),
        "there was one"
    );
    assert_eq!(git_credentials::token_for(user, HOST).await, None);
    assert!(
        !git_credentials::forget(user, HOST)
            .await
            .expect("should not fail"),
        "and now there is not — which is not an error"
    );
}

/// A credential that reaches GitHub as a person should not outlive the person.
#[tokio::test(flavor = "multi_thread")]
async fn deleting_an_account_takes_its_credentials_with_it() {
    let _guard = test_mutex().lock().await;
    setup_env().await;
    with_encryption();

    let user = "cred-cascade";
    store(user, "ghp_orphan").await;

    let removed = git_credentials::forget_all(user)
        .await
        .expect("should remove");
    assert_eq!(removed, 1);
    assert_eq!(git_credentials::token_for(user, HOST).await, None);
}

/// The tools are editor-tier, and an anonymous caller holds no capabilities at
/// all — so the answer is a refusal rather than an empty list, which would read
/// as "you have none".
#[tokio::test(flavor = "multi_thread")]
async fn credential_tools_refuse_a_caller_with_no_write_capability() {
    let _guard = test_mutex().lock().await;
    setup_env().await;
    with_encryption();

    for tool in [
        "set_git_credential",
        "list_git_credentials",
        "delete_git_credential",
    ] {
        let response = execute_native_mcp_tool(
            tool,
            &json!({ "token": "ghp_nope" }),
            &UserContext::anonymous(),
        )
        .expect("tool should run")
        .to_string();
        assert!(
            response.contains("Access denied"),
            "{} should refuse an anonymous caller, got: {}",
            tool,
            response
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_allowlist_names_hosts_and_matches_them_loosely() {
    use aiwebengine::config::GitConfig;

    // Empty allows everything the engine supports, which is what a personal
    // install wants and what an older configuration gets.
    let open = GitConfig::default();
    assert!(open.allows(HOST));

    let restricted = GitConfig {
        allowed_remotes: vec!["GitHub.com".to_string(), "git.internal/".to_string()],
    };
    assert!(
        restricted.allows(HOST),
        "case and trailing slash do not matter"
    );
    assert!(restricted.allows("git.internal"));
    assert!(
        !restricted.allows("gitlab.com"),
        "a host it does not name is refused"
    );

    // Naming only another host is how a deployment turns git sync off.
    let disabled = GitConfig {
        allowed_remotes: vec!["git.internal".to_string()],
    };
    assert!(!disabled.allows(HOST));
}

/// The engine stores a script's own secrets in the clear when no key is
/// configured, which is defensible for values a script put there itself. It is
/// not defensible for a credential that reaches outside the engine as a named
/// person, so this one is refused instead — and the message says what to do.
///
/// Deliberately does not call `with_encryption`: the key is process-wide, and
/// nextest gives this test a process of its own.
#[tokio::test(flavor = "multi_thread")]
async fn storing_without_an_encryption_key_is_refused() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let error = git_credentials::store("cred-nokey", HOST, "ghp_plaintext", None)
        .await
        .expect_err("a token must not be stored in the clear");

    assert!(
        error.to_string().contains("secret_encryption_key"),
        "the refusal should name what is missing, got: {}",
        error
    );
    assert_eq!(git_credentials::token_for("cred-nokey", HOST).await, None);
}
