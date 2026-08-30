//! Tests for the boundary between using a solution and authoring one.
//!
//! The engine has four principals: anonymous, authenticated, editor and
//! administrator. The middle two are the ones that used to be collapsed —
//! every signed-in user held `WriteScripts`, so anyone with an account could
//! deploy server-side JavaScript. These tests pin the tiers apart:
//!
//! - an authenticated user gets what *serving their requests* needs and
//!   nothing that edits the solution,
//! - an editor authors the scripts they own, bounded by ownership,
//! - only an administrator reaches what it does not own.

mod common;

use aiwebengine::engine_api::{delete_script_authorized, upsert_script_authorized};
use aiwebengine::repository;
use aiwebengine::security::UserContext;
use common::{setup_env, should_skip_integration_tests};

fn unique_uri(label: &str) -> String {
    format!("test://editor-tier/{}-{}", label, uuid::Uuid::new_v4())
}

/// The regression this whole change exists for: a signed-in user of a solution
/// must not be able to deploy a script. The owner check in
/// `upsert_script_authorized` only covers scripts that already exist, so the
/// capability is the only thing standing between a player and a new script.
#[tokio::test(flavor = "multi_thread")]
async fn authenticated_user_cannot_create_a_script() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let uri = unique_uri("player");
    let result = upsert_script_authorized(
        &UserContext::authenticated("player".to_string()),
        &uri,
        "function init() {}",
        None,
    );

    assert!(
        result.is_err(),
        "a user of a solution must not be able to deploy one"
    );
    assert!(
        repository::fetch_script(&uri).is_none(),
        "and nothing should have been written"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn editor_can_create_and_delete_its_own_script() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let uri = unique_uri("owned");
    let editor = UserContext::editor("author".to_string());

    assert!(
        upsert_script_authorized(&editor, &uri, "function init() {}", None).is_ok(),
        "an editor authors scripts"
    );
    assert!(repository::fetch_script(&uri).is_some());

    assert!(
        delete_script_authorized(&editor, &uri, None),
        "and may remove what it owns"
    );
    assert!(repository::fetch_script(&uri).is_none());
}

/// `DeleteScripts` says the caller deletes scripts; ownership says which ones.
/// Without the ownership check that pairing is missing, one editor can delete
/// every script in the engine.
#[tokio::test(flavor = "multi_thread")]
async fn editor_cannot_delete_a_script_it_does_not_own() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let uri = unique_uri("someone-elses");
    let owner = UserContext::editor("first-author".to_string());
    assert!(upsert_script_authorized(&owner, &uri, "function init() {}", None).is_ok());

    let intruder = UserContext::editor("second-author".to_string());
    assert!(
        !delete_script_authorized(&intruder, &uri, None),
        "an editor must not delete another author's script"
    );
    assert!(
        repository::fetch_script(&uri).is_some(),
        "and the script must survive the attempt"
    );

    // The administrator tier is what reaches across ownership.
    assert!(
        delete_script_authorized(&UserContext::admin("root".to_string()), &uri, None),
        "an administrator deletes what it does not own"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn editor_cannot_modify_a_script_it_does_not_own() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let uri = unique_uri("guarded");
    let owner = UserContext::editor("first-author".to_string());
    let original = "function init() { /* original */ }";
    assert!(upsert_script_authorized(&owner, &uri, original, None).is_ok());

    let intruder = UserContext::editor("second-author".to_string());
    assert!(
        upsert_script_authorized(&intruder, &uri, "function init() { /* replaced */ }", None)
            .is_err(),
        "an editor must not overwrite another author's script"
    );

    let stored = repository::fetch_script(&uri).expect("script should still exist");
    assert!(
        stored.contains("original"),
        "the original content must be intact"
    );
}
