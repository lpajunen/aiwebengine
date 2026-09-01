//! Giving an install an owner, and taking one away.
//!
//! Reaching the administrator tier used to require an administrator: roles are
//! granted through `/engine/user_roles`, which is guarded by `AdministerEngine`,
//! and `auth.bootstrap_admins` matches an email address that only a provider can
//! verify. On a laptop with no OAuth client that is a circle with no way in, and
//! the only workaround was development mode — which grants engine administration
//! to *anonymous* callers on every interface the engine binds.
//!
//! The other half is that a role, once granted, is stamped into a session and
//! read from there. Taking it away therefore has to reach the sessions that
//! already exist, or revoking an administrator revokes nothing they are holding.

mod common;

use aiwebengine::auth::local::{self, LOCAL_PROVIDER};
use aiwebengine::security::{CreateSessionParams, SecureSessionManager, SecurityAuditor};
use aiwebengine::user_repository::{self, GLOBAL_REALM, UserRole};
use common::{setup_env, should_skip_integration_tests};
use std::sync::Arc;

const PASSWORD: &str = "a-perfectly-fine-password";
const REALM: &str = "install.example.com";

fn unique(label: &str) -> String {
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    format!("{}{}", label, &suffix[..12])
}

/// Create a local account the way registration does, without needing an
/// `AuthManager`: a user row, then a credential against it.
async fn create_local_account(username: &str) -> String {
    let user_id = user_repository::upsert_internal_user(
        None,
        LOCAL_PROVIDER.to_string(),
        username.to_string(),
        REALM.to_string(),
    )
    .await
    .expect("account should be created");

    local::attach_credential(&user_id, username, PASSWORD, 12)
        .await
        .expect("credential should attach");

    user_id
}

/// The way in. An operator writes a username into the configuration file —
/// the same authority `bootstrap_admins` already runs on — and that account
/// administers the engine.
#[tokio::test(flavor = "multi_thread")]
async fn a_configured_username_becomes_an_administrator() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let username = unique("owner");
    user_repository::set_bootstrap_admin_usernames(vec![username.to_uppercase()]);

    let user_id = create_local_account(&username).await;
    user_repository::apply_bootstrap_admin_username(&user_id, &username)
        .await
        .expect("the configured role should apply");

    let user = user_repository::get_user_async(&user_id)
        .await
        .expect("account should be readable");

    assert!(
        user.roles.contains(&UserRole::Administrator),
        "an account the operator named is an administrator"
    );
    assert_eq!(
        user.realm, GLOBAL_REALM,
        "and a principal on every host, or the management host refuses the \
         account that exists to administer it"
    );
}

/// The case that matters more: someone installs the engine, makes an account,
/// and writes their username into the config afterwards. An upsert preserves
/// the roles on a row it finds, so applying this only at creation would leave
/// the declaration applying to everyone except the person who made it.
#[tokio::test(flavor = "multi_thread")]
async fn a_username_named_after_the_account_exists_is_still_granted() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let username = unique("later");
    let user_id = create_local_account(&username).await;

    let before = user_repository::get_user_async(&user_id)
        .await
        .expect("account should be readable");
    assert!(
        !before.roles.contains(&UserRole::Administrator),
        "nothing about registering earns the tier"
    );

    user_repository::set_bootstrap_admin_usernames(vec![username.clone()]);
    user_repository::apply_bootstrap_admin_username(&user_id, &username)
        .await
        .expect("the configured role should apply");

    let after = user_repository::get_user_async(&user_id)
        .await
        .expect("account should be readable");
    assert!(after.roles.contains(&UserRole::Administrator));
}

/// Nobody else. A username that is not in the list gets nothing, which is the
/// whole reason this is safe to run on every sign-in.
#[tokio::test(flavor = "multi_thread")]
async fn an_unnamed_account_is_left_alone() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    user_repository::set_bootstrap_admin_usernames(vec![unique("someone-else")]);

    let username = unique("ordinary");
    let user_id = create_local_account(&username).await;
    user_repository::apply_bootstrap_admin_username(&user_id, &username)
        .await
        .expect("call should succeed and do nothing");

    let user = user_repository::get_user_async(&user_id)
        .await
        .expect("account should be readable");

    assert!(!user.roles.contains(&UserRole::Administrator));
    assert_eq!(
        user.realm, REALM,
        "and is still a principal only where it was created"
    );
}

/// A password nobody can change is the state a personal install was left in:
/// the credential is the only way in, and `attach_credential` refuses once one
/// exists.
#[tokio::test(flavor = "multi_thread")]
async fn a_password_can_be_changed_with_the_one_it_replaces() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let username = unique("rotate");
    let user_id = create_local_account(&username).await;

    assert!(
        local::change_password(&user_id, "not-the-password", "a-brand-new-password", 12)
            .await
            .is_err(),
        "holding a session is not authorization to replace the credential"
    );

    local::change_password(&user_id, PASSWORD, "a-brand-new-password", 12)
        .await
        .expect("the current password authorizes the change");

    assert!(
        local::verify_login(&username, "a-brand-new-password")
            .await
            .is_ok(),
        "the new password signs in"
    );
    assert!(
        local::verify_login(&username, PASSWORD).await.is_err(),
        "and the old one does not"
    );
}

/// A new password must satisfy the same rules as the first one, or rotation is
/// a way around them.
#[tokio::test(flavor = "multi_thread")]
async fn a_new_password_is_held_to_the_same_rules() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let username = unique("weak");
    let user_id = create_local_account(&username).await;

    assert!(
        local::change_password(&user_id, PASSWORD, "short", 12)
            .await
            .is_err()
    );
    assert!(
        local::verify_login(&username, PASSWORD).await.is_ok(),
        "and the account keeps the password it had"
    );
}

/// There is no password to change on an identity that has none, and inventing
/// one would be a way to take over a guest or a federated account from whatever
/// session happened to be open.
#[tokio::test(flavor = "multi_thread")]
async fn an_account_without_a_credential_cannot_have_one_set_this_way() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let guest = user_repository::upsert_internal_user(
        None,
        aiwebengine::auth::local::GUEST_PROVIDER.to_string(),
        uuid::Uuid::new_v4().to_string(),
        REALM.to_string(),
    )
    .await
    .expect("guest should be created");

    assert!(
        local::change_password(&guest, "", "a-brand-new-password", 12)
            .await
            .is_err()
    );
}

const IP: &str = "192.168.1.1";
const UA: &str = "test-agent";

async fn session_manager() -> SecureSessionManager {
    let pool = aiwebengine::database::get_global_database()
        .expect("the suite's database should be up")
        .pool()
        .clone();
    let key: [u8; 32] = rand::random();
    let auditor = Arc::new(SecurityAuditor::new(Some(pool.clone())));
    SecureSessionManager::new(pool, &key, 3600, 86400 * 30, 3, auditor)
        .expect("session manager should build")
}

fn session_params(user_id: &str) -> CreateSessionParams {
    CreateSessionParams {
        user_id: user_id.to_string(),
        provider: LOCAL_PROVIDER.to_string(),
        email: None,
        name: None,
        is_admin: true,
        is_editor: false,
        ip_addr: IP.to_string(),
        user_agent: UA.to_string(),
        refresh_token: None,
        audience: None,
        realm: REALM.to_string(),
    }
}

/// Revoking a role has to reach what was already issued. Roles are read from
/// the repository once, when a session is minted, and every consumer reads that
/// stamped copy — so without this, an administrator whose role is taken away
/// keeps administering until the session ages out, up to thirty days later.
#[tokio::test(flavor = "multi_thread")]
async fn changing_a_role_ends_the_sessions_that_carry_the_old_one() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let username = unique("demoted");
    let user_id = create_local_account(&username).await;
    let manager = session_manager().await;

    let session = manager
        .create_session(session_params(&user_id))
        .await
        .expect("session should be created");

    assert!(
        manager
            .validate_session(&session.token, IP, UA, REALM)
            .await
            .is_ok(),
        "the session works before the role changes"
    );

    let user_id_for_grant = user_id.clone();
    tokio::task::spawn_blocking(move || {
        user_repository::add_user_role(&user_id_for_grant, UserRole::Editor)
    })
    .await
    .expect("task should run")
    .expect("role should be granted");

    assert!(
        manager
            .validate_session(&session.token, IP, UA, REALM)
            .await
            .is_err(),
        "and not after: the session carries roles that are no longer current"
    );
}

/// The same statement made directly, for the paths that make it — a password
/// change, an account deleted.
#[tokio::test(flavor = "multi_thread")]
async fn every_session_a_user_holds_can_be_ended_at_once() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let username = unique("revoked");
    let user_id = create_local_account(&username).await;
    let manager = session_manager().await;

    let first = manager
        .create_session(session_params(&user_id))
        .await
        .expect("session should be created");
    let second = manager
        .create_session(session_params(&user_id))
        .await
        .expect("second session should be created");

    let ended = manager
        .invalidate_all_sessions_for_user(&user_id)
        .await
        .expect("revocation should succeed");
    assert_eq!(ended, 2, "both of them, not just the one being used");

    for token in [first.token, second.token] {
        assert!(
            manager
                .validate_session(&token, IP, UA, REALM)
                .await
                .is_err()
        );
    }
}
