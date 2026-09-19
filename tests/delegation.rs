//! Acting as somebody who is not here.
//!
//! What these cover is the boundary rather than the plumbing: that a task
//! cannot act as a person without a live grant, that withdrawing one reaches
//! work already queued, and that a delegated run gets the person's own storage
//! and secrets without getting their authority to author anything.

mod common;

use aiwebengine::delegation::{self, Scope};
use aiwebengine::tasks::{self, NewTask, TaskKind};
use aiwebengine::{js_engine, repository};
use chrono::{Duration, Utc};
use common::setup_env;
use serde_json::json;

async fn a_user(label: &str) -> String {
    aiwebengine::user_repository::upsert_internal_user(
        Some(format!("delegation test {}", label)),
        "test".to_string(),
        format!("{}-{}", label, uuid::Uuid::new_v4()),
        "localhost".to_string(),
    )
    .await
    .expect("the account should be created")
}

fn personal_task(script_uri: &str, handler: &str, user_id: &str) -> NewTask {
    NewTask {
        script_uri: script_uri.to_string(),
        handler_name: handler.to_string(),
        payload: json!({}),
        run_at: None,
        max_attempts: Some(3),
        enqueued_by: Some(user_id.to_string()),
        kind: TaskKind::Task,
        run_as: Some(user_id.to_string()),
    }
}

/// The whole point: without consent there is no acting as anybody, and the
/// task says so rather than quietly running as the script.
#[tokio::test(flavor = "multi_thread")]
async fn a_task_cannot_act_as_someone_who_has_not_authorised_it() {
    setup_env().await;
    let user_id = a_user("ungranted").await;

    let script_uri = "test://delegation/ungranted";
    repository::upsert_script(script_uri, "function work() {}").expect("script should store");

    let task = tasks::enqueue(personal_task(script_uri, "work", &user_id))
        .await
        .expect("the task should be accepted");

    tasks::run_due_now("test-worker").await;

    let found = tasks::get(task.task_id)
        .await
        .expect("lookup")
        .expect("an abandoned task keeps its row");

    assert_eq!(found.state, "failed", "it must not have run");
    assert!(
        found
            .last_error
            .is_some_and(|e| e.contains("has not authorised")),
        "the row should say why"
    );
}

/// A refusal that will not clear by waiting must not spend attempts retrying.
/// The engine repeatedly asking to act as somebody who said no is the wrong
/// behaviour even though each attempt is individually harmless.
#[tokio::test(flavor = "multi_thread")]
async fn an_unauthorised_task_is_abandoned_rather_than_retried() {
    setup_env().await;
    let user_id = a_user("no-retry").await;

    let script_uri = "test://delegation/no-retry";
    repository::upsert_script(script_uri, "function work() {}").expect("script should store");

    let task = tasks::enqueue(personal_task(script_uri, "work", &user_id))
        .await
        .expect("accepted");
    tasks::run_due_now("test-worker").await;

    let found = tasks::get(task.task_id)
        .await
        .expect("lookup")
        .expect("row");
    assert_eq!(found.state, "failed");
    assert_eq!(
        found.attempts, 0,
        "no attempt was spent, because there was nothing to attempt"
    );
}

/// A grant that has lapsed is not a grant. There is no "forever" to choose, so
/// this is the state every delegation eventually reaches.
#[tokio::test(flavor = "multi_thread")]
async fn a_lapsed_grant_does_not_authorise_anything() {
    setup_env().await;
    let user_id = a_user("lapsed").await;
    let script_uri = "test://delegation/lapsed";
    repository::upsert_script(script_uri, "function work() {}").expect("script should store");

    delegation::grant(&user_id, script_uri, &Scope::all(), Duration::days(1))
        .await
        .expect("granted");

    // Wind the expiry back the way time would.
    sqlx::query(
        "UPDATE script_delegations SET expires_at = NOW() - INTERVAL '1 hour' WHERE user_id = $1",
    )
    .bind(&user_id)
    .execute(&common::test_pool().await)
    .await
    .expect("the grant should lapse");

    assert_eq!(
        delegation::resolve(&user_id, script_uri).await.unwrap_err(),
        delegation::Refusal::Expired
    );
}

/// Withdrawing has to reach work that is already queued, or the person has
/// revoked something they can still see happening.
#[tokio::test(flavor = "multi_thread")]
async fn withdrawing_a_grant_cancels_what_it_had_queued() {
    setup_env().await;
    let user_id = a_user("withdraw").await;
    let script_uri = "test://delegation/withdraw";
    repository::upsert_script(script_uri, "function work() {}").expect("script should store");

    delegation::grant(&user_id, script_uri, &Scope::all(), Duration::days(1))
        .await
        .expect("granted");

    let task = tasks::enqueue(personal_task(script_uri, "work", &user_id))
        .await
        .expect("accepted");

    assert!(
        delegation::revoke(&user_id, script_uri)
            .await
            .expect("revoke should run"),
        "the grant should have been withdrawn"
    );

    let found = tasks::get(task.task_id)
        .await
        .expect("lookup")
        .expect("row");
    assert_eq!(
        found.state, "cancelled",
        "queued work should not outlive the authorisation it was queued under"
    );
}

/// Signing out everywhere has to include the thing a session list cannot show.
#[tokio::test(flavor = "multi_thread")]
async fn ending_every_session_also_ends_every_delegation() {
    setup_env().await;
    let user_id = a_user("signout").await;
    let script_uri = "test://delegation/signout";
    repository::upsert_script(script_uri, "function work() {}").expect("script should store");

    delegation::grant(&user_id, script_uri, &Scope::all(), Duration::days(1))
        .await
        .expect("granted");

    aiwebengine::security::delete_sessions_for_user(&common::test_pool().await, &user_id)
        .await
        .expect("sign-out should run");

    assert!(
        delegation::get(&user_id, script_uri)
            .await
            .expect("lookup")
            .is_none(),
        "a delegation should not survive the account being signed out everywhere"
    );
}

/// A grant is per script. Authorising one solution says nothing about another.
#[tokio::test(flavor = "multi_thread")]
async fn a_grant_for_one_script_does_not_authorise_a_different_one() {
    setup_env().await;
    let user_id = a_user("scoped").await;
    let granted = "test://delegation/granted-script";
    let other = "test://delegation/other-script";
    repository::upsert_script(granted, "function work() {}").expect("script should store");
    repository::upsert_script(other, "function work() {}").expect("script should store");

    delegation::grant(&user_id, granted, &Scope::all(), Duration::days(1))
        .await
        .expect("granted");

    assert!(delegation::resolve(&user_id, granted).await.is_ok());
    assert_eq!(
        delegation::resolve(&user_id, other).await.unwrap_err(),
        delegation::Refusal::NoGrant,
        "a grant must not carry across to another script"
    );
}

/// The point of delegating: the person's own storage, which is unreachable
/// from an ordinary background task.
#[tokio::test(flavor = "multi_thread")]
async fn a_delegated_task_reaches_the_persons_own_storage() {
    setup_env().await;
    let user_id = a_user("storage").await;
    let script_uri = "test://delegation/storage";
    repository::upsert_script(
        script_uri,
        r#"
        function work(context) {
          personalStorage.setItem("touched", "by the task");
          console.log("stored for " + context.request.auth.userId);
        }
        "#,
    )
    .expect("script should store");
    repository::clear_log_messages(script_uri).expect("logs should clear");

    delegation::grant(
        &user_id,
        script_uri,
        &[Scope::PersonalStorage],
        Duration::days(1),
    )
    .await
    .expect("granted");

    let task = tasks::enqueue(personal_task(script_uri, "work", &user_id))
        .await
        .expect("accepted");
    tasks::run_due_now("test-worker").await;

    assert!(
        tasks::get(task.task_id).await.expect("lookup").is_none(),
        "the task should have succeeded"
    );

    let stored = repository::get_user_properties_item(script_uri, &user_id, "touched");
    assert_eq!(
        stored.as_deref(),
        Some("by the task"),
        "the task should have written to that person's own storage"
    );
}

/// An ordinary task has no person, so personal storage is not reachable from
/// one. This is the behaviour delegation exists to change, and it must stay
/// true of everything that has not been delegated.
#[tokio::test(flavor = "multi_thread")]
async fn an_undelegated_task_reaches_nobodys_storage() {
    setup_env().await;
    let script_uri = "test://delegation/no-person";
    repository::upsert_script(
        script_uri,
        r#"
        function work() {
          personalStorage.setItem("touched", "should not happen");
        }
        "#,
    )
    .expect("script should store");

    let task = tasks::enqueue(NewTask {
        script_uri: script_uri.to_string(),
        handler_name: "work".to_string(),
        payload: json!({}),
        run_at: None,
        max_attempts: Some(1),
        enqueued_by: None,
        kind: TaskKind::Task,
        run_as: None,
    })
    .await
    .expect("accepted");

    tasks::run_due_now("test-worker").await;

    let found = tasks::get(task.task_id)
        .await
        .expect("lookup")
        .expect("row");
    assert_eq!(
        found.state, "failed",
        "personal storage with no person should throw, not silently write somewhere"
    );
}

/// The tier is capped however much the person holds, so a delegated task
/// belonging to an administrator is not a way to get an administrative one.
#[tokio::test(flavor = "multi_thread")]
async fn a_delegated_task_is_not_an_administrator_even_for_an_administrator() {
    setup_env().await;
    let user_id = a_user("admin-delegate").await;
    let for_roles = user_id.clone();
    tokio::task::spawn_blocking(move || {
        aiwebengine::user_repository::update_user_roles(
            &for_roles,
            vec![aiwebengine::user_repository::UserRole::Administrator],
        )
    })
    .await
    .expect("no panic")
    .expect("roles should be set");

    let script_uri = "test://delegation/admin";
    repository::upsert_script(script_uri, "function work() {}").expect("script should store");
    delegation::grant(&user_id, script_uri, &Scope::all(), Duration::days(1))
        .await
        .expect("granted");

    let delegated = delegation::resolve(&user_id, script_uri)
        .await
        .expect("the grant is live");

    use aiwebengine::security::Capability;
    for forbidden in [
        Capability::AdministerEngine,
        Capability::WriteScripts,
        Capability::DeleteScripts,
    ] {
        assert!(
            !delegated.user_context.capabilities.contains(&forbidden),
            "background work must not hold {:?} however much the person holds",
            forbidden
        );
    }
}

/// Consent replaces rather than merges: the page shows what is being asked for
/// and the person agrees to that, so a narrower second grant is narrower.
#[tokio::test(flavor = "multi_thread")]
async fn granting_again_replaces_what_was_granted_before() {
    setup_env().await;
    let user_id = a_user("replace").await;
    let script_uri = "test://delegation/replace";

    delegation::grant(&user_id, script_uri, &Scope::all(), Duration::days(30))
        .await
        .expect("granted");
    delegation::grant(
        &user_id,
        script_uri,
        &[Scope::PersonalStorage],
        Duration::days(30),
    )
    .await
    .expect("granted again");

    let grant = delegation::get(&user_id, script_uri)
        .await
        .expect("lookup")
        .expect("there is a grant");

    assert!(grant.allows(Scope::PersonalStorage));
    assert!(
        !grant.allows(Scope::Secrets),
        "consenting again to less must not leave the earlier, wider grant standing"
    );
}

/// A delegated run is recorded as such, so the queue listing can say what is
/// acting as whom.
#[tokio::test(flavor = "multi_thread")]
async fn the_queue_records_who_a_task_acts_as() {
    setup_env().await;
    let user_id = a_user("recorded").await;
    let script_uri = "test://delegation/recorded";
    repository::upsert_script(script_uri, "function work() {}").expect("script should store");
    delegation::grant(&user_id, script_uri, &Scope::all(), Duration::days(1))
        .await
        .expect("granted");

    let task = tasks::enqueue(personal_task(script_uri, "work", &user_id))
        .await
        .expect("accepted");

    assert_eq!(task.run_as.as_deref(), Some(user_id.as_str()));

    let listed = tasks::list(script_uri, 10).await.expect("listing");
    assert!(
        listed
            .iter()
            .any(|t| t.task_id == task.task_id && t.run_as.as_deref() == Some(user_id.as_str())),
        "the listing should say who the task acts as"
    );
}

/// Unused import guard: `js_engine` is reached through the worker rather than
/// directly here, and this keeps the dependency honest if that changes.
#[allow(dead_code)]
fn _uses_js_engine() {
    let _ = js_engine::execute_task_handler;
}

/// `Utc` is used by the lapse test through sqlx; this keeps the import honest.
#[allow(dead_code)]
fn _uses_utc() -> chrono::DateTime<Utc> {
    Utc::now()
}
