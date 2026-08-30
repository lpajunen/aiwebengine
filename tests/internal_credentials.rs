//! Tests for identities the engine authenticates itself.
//!
//! Guests and local username-and-password accounts exist so a solution's users
//! do not have to hand a real-world identity to Google, Microsoft or Apple, and
//! so a personal install can have a real logged-in owner without anyone
//! registering an OAuth client. These tests cover the parts that decide whether
//! that is safe: what tier such an account lands in, that a guest keeps its
//! `user_id` when it claims a name, and that the claim path cannot be used to
//! take over an account that already has one.

mod common;

use aiwebengine::auth::local::{
    self, GUEST_PROVIDER, LOCAL_PROVIDER, attach_credential, verify_login,
};
use aiwebengine::user_repository::{self, UserRole};
use common::{setup_env, should_skip_integration_tests};

/// Password long enough for the default configured minimum.
const PASSWORD: &str = "a-perfectly-fine-password";

/// A username that is unique per run and still inside the 32-character limit.
fn unique(label: &str) -> String {
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    format!("{}{}", label, &suffix[..12])
}

async fn create_guest() -> String {
    user_repository::upsert_internal_user(
        Some("Guest".to_string()),
        GUEST_PROVIDER.to_string(),
        uuid::Uuid::new_v4().to_string(),
    )
    .await
    .expect("guest should be created")
}

/// An internal identity is an ordinary user of a solution and nothing more.
/// `auth.bootstrap_admins` matches on email address, and these accounts have
/// none — but the guarantee worth pinning is the positive one: they land in the
/// authenticated tier, with no editor or administrator role.
#[tokio::test(flavor = "multi_thread")]
async fn an_internal_identity_lands_in_the_authenticated_tier() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let user_id = create_guest().await;
    let user = user_repository::get_user_async(&user_id)
        .await
        .expect("guest should be readable");

    assert_eq!(
        user.email, None,
        "a guest has no address, and the schema should say so rather than inventing one"
    );
    assert!(user.roles.contains(&UserRole::Authenticated));
    assert!(
        !user.roles.contains(&UserRole::Editor),
        "using a solution is not authoring one"
    );
    assert!(
        !user.roles.contains(&UserRole::Administrator),
        "an account anyone can mint must never arrive as an administrator"
    );
}

/// The reason a guest is worth offering: claiming a name keeps everything the
/// account already had, because it is the same account.
#[tokio::test(flavor = "multi_thread")]
async fn claiming_a_guest_account_keeps_its_user_id() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let user_id = create_guest().await;
    let username = unique("player");

    attach_credential(&user_id, &username, PASSWORD, 12)
        .await
        .expect("a guest should be able to claim a name");

    let signed_in = verify_login(&username, PASSWORD)
        .await
        .expect("the claimed credential should sign in");

    assert_eq!(
        signed_in, user_id,
        "signing in must land on the account that was claimed, not a new one"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_username_can_only_be_claimed_once() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let username = unique("contested");
    let first = create_guest().await;
    let second = create_guest().await;

    attach_credential(&first, &username, PASSWORD, 12)
        .await
        .expect("the first claim should succeed");

    let result = attach_credential(&second, &username, PASSWORD, 12).await;
    assert!(
        result.is_err(),
        "a second account must not be able to take a name already in use"
    );

    assert_eq!(
        verify_login(&username, PASSWORD)
            .await
            .expect("the original credential should still work"),
        first,
        "and the name must still belong to whoever claimed it first"
    );
}

/// One credential per account. Without this, the claim path is a password
/// reset that needs no old password — anyone reaching an authenticated session
/// could overwrite the credential protecting it.
#[tokio::test(flavor = "multi_thread")]
async fn an_account_with_a_credential_cannot_be_claimed_again() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let user_id = create_guest().await;
    let original = unique("settled");
    attach_credential(&user_id, &original, PASSWORD, 12)
        .await
        .expect("the first claim should succeed");

    let result = attach_credential(&user_id, &unique("second"), "a-different-password", 12).await;
    assert!(
        result.is_err(),
        "claiming must not overwrite a credential that already exists"
    );

    assert_eq!(
        verify_login(&original, PASSWORD)
            .await
            .expect("the original credential should still work"),
        user_id
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_wrong_password_and_an_unknown_username_answer_the_same_way() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let user_id = create_guest().await;
    let username = unique("known");
    attach_credential(&user_id, &username, PASSWORD, 12)
        .await
        .expect("claim should succeed");

    let wrong_password = verify_login(&username, "not-the-password")
        .await
        .expect_err("a wrong password must not sign in");
    let unknown_user = verify_login(&unique("nobody"), PASSWORD)
        .await
        .expect_err("an unknown username must not sign in");

    assert_eq!(
        wrong_password.to_string(),
        unknown_user.to_string(),
        "telling the two apart tells an attacker which usernames exist"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_username_is_matched_regardless_of_how_it_is_typed() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let user_id = create_guest().await;
    let username = unique("MixedCase");
    attach_credential(&user_id, &username, PASSWORD, 12)
        .await
        .expect("claim should succeed");

    for spelling in [
        username.to_lowercase(),
        username.to_uppercase(),
        format!("  {}  ", username),
    ] {
        assert_eq!(
            verify_login(&spelling, PASSWORD)
                .await
                .expect("every spelling should reach the same account"),
            user_id,
            "{:?} should sign in",
            spelling
        );
    }
}

/// A local account is found by the provider pair the same way a federated one
/// is, which is what lets both share the session path.
#[tokio::test(flavor = "multi_thread")]
async fn a_local_account_is_recorded_under_the_local_provider() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let username = local::normalize_username(&unique("Registered"));
    let user_id = user_repository::upsert_internal_user(
        Some("Registered Player".to_string()),
        LOCAL_PROVIDER.to_string(),
        username.clone(),
    )
    .await
    .expect("account should be created");

    let found = user_repository::find_user_by_provider(LOCAL_PROVIDER, &username)
        .expect("lookup should succeed")
        .expect("the account should be found by its provider pair");

    assert_eq!(found.id, user_id);
    assert_eq!(found.email, None);
}
