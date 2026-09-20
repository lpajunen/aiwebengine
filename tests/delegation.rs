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

    // The noun says *whose* storage is in scope; the verb says it may be
    // changed. This test is about the first, so it grants both.
    delegation::grant(
        &user_id,
        script_uri,
        &[Scope::PersonalStorage, Scope::Write],
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

/// A scope is what the person ticked, and ticking one must not deliver the
/// other. This is the boundary the consent page describes, and it was
/// decoration until the scopes were enforced at the surfaces they name.
#[tokio::test(flavor = "multi_thread")]
async fn storage_consent_alone_does_not_hand_over_the_persons_secrets() {
    setup_env().await;
    let user_id = a_user("storage-only").await;
    let script_uri = "test://delegation/storage-only";

    repository::upsert_script(
        script_uri,
        r#"
        function work() {
          // Granted: this must work.
          personalStorage.setItem("touched", "yes");
          // Not granted: the person's key must not be visible.
          if (secretStorage.exists("THEIR_KEY")) {
            throw new Error("a storage-only grant reached the person's secrets");
          }
        }
        "#,
    )
    .expect("script should store");

    // A secret that belongs to this person, which the task must not see.
    repository::set_user_secret_item(script_uri, &user_id, "THEIR_KEY", "sk-private")
        .expect("the person's secret should store");

    // The noun says *whose* storage is in scope; the verb says it may be
    // changed. This test is about the first, so it grants both.
    delegation::grant(
        &user_id,
        script_uri,
        &[Scope::PersonalStorage, Scope::Write],
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
        "the task should have succeeded, meaning it saw storage and not secrets"
    );
    assert_eq!(
        repository::get_user_properties_item(script_uri, &user_id, "touched").as_deref(),
        Some("yes"),
        "the scope that *was* granted should still work"
    );
}

/// And the other way round.
#[tokio::test(flavor = "multi_thread")]
async fn secrets_consent_alone_does_not_hand_over_the_persons_storage() {
    setup_env().await;
    let user_id = a_user("secrets-only").await;
    let script_uri = "test://delegation/secrets-only";

    repository::upsert_script(
        script_uri,
        r#"
        function work() {
          // Granted: the person's key is visible to the engine's substitution.
          if (!secretStorage.exists("THEIR_KEY")) {
            throw new Error("a secrets grant did not reach the person's secrets");
          }
          // Not granted: their storage must be unreachable, which reads as
          // nobody being signed in.
          try {
            personalStorage.setItem("touched", "should not happen");
          } catch (e) {
            return;
          }
          throw new Error("a secrets-only grant reached the person's storage");
        }
        "#,
    )
    .expect("script should store");

    repository::set_user_secret_item(script_uri, &user_id, "THEIR_KEY", "sk-private")
        .expect("the person's secret should store");

    delegation::grant(&user_id, script_uri, &[Scope::Secrets], Duration::days(1))
        .await
        .expect("granted");

    let task = tasks::enqueue(personal_task(script_uri, "work", &user_id))
        .await
        .expect("accepted");
    tasks::run_due_now("test-worker").await;

    assert!(
        tasks::get(task.task_id).await.expect("lookup").is_none(),
        "the task should have succeeded, meaning it saw secrets and not storage"
    );
    assert!(
        repository::get_user_properties_item(script_uri, &user_id, "touched").is_none(),
        "nothing should have been written to storage that was not consented to"
    );
}

/// Managing a credential is refused in any delegated execution, whatever was
/// granted. The consent page offers "use the API keys you have given this
/// app", and replacing or deleting one is not using it.
#[tokio::test(flavor = "multi_thread")]
async fn background_work_cannot_change_the_persons_secrets_even_with_every_scope() {
    setup_env().await;
    let user_id = a_user("no-manage").await;
    let script_uri = "test://delegation/no-manage";

    repository::upsert_script(
        script_uri,
        r#"
        function work() {
          secretStorage.setSecret("THEIR_KEY", "replaced-by-the-agent");
          secretStorage.removeSecret("OTHER_KEY");
        }
        "#,
    )
    .expect("script should store");

    repository::set_user_secret_item(script_uri, &user_id, "THEIR_KEY", "sk-original")
        .expect("stored");
    repository::set_user_secret_item(script_uri, &user_id, "OTHER_KEY", "sk-other")
        .expect("stored");

    delegation::grant(&user_id, script_uri, &Scope::all(), Duration::days(1))
        .await
        .expect("granted");

    let _task = tasks::enqueue(personal_task(script_uri, "work", &user_id))
        .await
        .expect("accepted");
    tasks::run_due_now("test-worker").await;

    assert_eq!(
        repository::get_user_secret_item(script_uri, &user_id, "THEIR_KEY").as_deref(),
        Some("sk-original"),
        "background work must not replace somebody's stored credential"
    );
    assert!(
        repository::get_user_secret_item(script_uri, &user_id, "OTHER_KEY").is_some(),
        "background work must not delete somebody's stored credential"
    );
}

/// The property that keeps this change invisible to everything that is not
/// delegated: a person acting for themselves is narrowed by nothing.
#[tokio::test(flavor = "multi_thread")]
async fn an_ordinary_request_is_narrowed_by_no_scope() {
    setup_env().await;

    let script_uri = "test://delegation/undelegated-request";
    repository::upsert_script(
        script_uri,
        r#"
        function handler(context) {
          personalStorage.setItem("touched", "by the request");
          const manage = secretStorage.setSecret("MINE", "sk-set-by-me");
          return { status: 200, body: manage, contentType: "text/plain" };
        }
        "#,
    )
    .expect("script should store");

    let user_id = a_user("in-person").await;
    let auth = aiwebengine::auth::JsAuthContext::authenticated(
        user_id.clone(),
        None,
        None,
        "test".to_string(),
        false,
        false,
    );

    let response = tokio::task::spawn_blocking({
        let user_id = user_id.clone();
        move || {
            js_engine::execute_script_for_request_secure(js_engine::RequestExecutionParams {
                script_uri: script_uri.to_string(),
                handler_name: "handler".to_string(),
                path: "/x".to_string(),
                method: "GET".to_string(),
                query_params: None,
                url: None,
                form_data: None,
                raw_body: None,
                headers: Default::default(),
                user_context: aiwebengine::security::UserContext::authenticated(user_id),
                auth_context: Some(auth),
                route_params: None,
                uploaded_files: None,
                request_id: None,
                route_pattern: None,
            })
        }
    })
    .await
    .expect("no panic")
    .expect("the handler should answer");

    let body = String::from_utf8_lossy(&response.body).to_string();
    assert_eq!(response.status, 200);
    assert!(
        !body.starts_with("Error:"),
        "a person acting for themselves should still manage their own secrets: {}",
        body
    );
    assert_eq!(
        repository::get_user_properties_item(script_uri, &user_id, "touched").as_deref(),
        Some("by the request"),
        "an ordinary request should still reach its own storage"
    );
}

/// The verb, end to end: a real delegated task, running under a grant that
/// did not say "change things", is refused at every write it tries and left
/// with the reads it needs.
///
/// This is the "plan approved in advance" the vocabulary could not express —
/// a person authorising an app to go away and work out what to do, without
/// authorising it to do the thing. It is enforced under the JavaScript, at
/// the same gate every other caller meets, so the handler cannot arrange its
/// way past it.
#[tokio::test(flavor = "multi_thread")]
async fn a_read_only_grant_stops_a_delegated_task_from_writing() {
    setup_env().await;
    let user_id = a_user("read-only").await;
    let script_uri = "test://delegation/read-only";

    repository::upsert_script(
        script_uri,
        r#"
        function work(context) {
          // Reading is the floor: without it there would be nothing to
          // consent to. Personal storage is in scope because the noun was
          // granted, and readable because reading is always granted.
          const seen = personalStorage.getItem("note");
          if (seen !== "left earlier") {
            throw new Error("a read-only grant should still read: " + seen);
          }

          // Every write there is, each refused in the way that surface
          // reports refusals.
          try {
            personalStorage.setItem("note", "changed");
            throw new Error("storage was writable");
          } catch (e) {
            if (String(e.message).indexOf("write_storage") < 0) { throw e; }
          }

          const wrote = database.insert("notes", JSON.stringify({ body: "x" })).json();
          if (!wrote.error || wrote.error.indexOf("write_script_data") < 0) {
            throw new Error("the database was writable: " + JSON.stringify(wrote));
          }

          try {
            scriptTasks.enqueue({ handler: "work", payload: {} });
            throw new Error("the queue was reachable");
          } catch (e) {
            if (String(e.message).indexOf("enqueue_tasks") < 0) { throw e; }
          }

          console.log("read-only run finished");
        }
        "#,
    )
    .expect("script should store");

    repository::set_user_properties_item(script_uri, &user_id, "note", "left earlier")
        .expect("the person's note should store");

    // The noun without the verb: reach my data, do not change it.
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

    let outcome = tasks::get(task.task_id).await.expect("lookup");
    assert!(
        outcome.is_none(),
        "every refusal should have been the expected one: {:?}",
        outcome.and_then(|task| task.last_error)
    );

    assert_eq!(
        repository::get_user_properties_item(script_uri, &user_id, "note").as_deref(),
        Some("left earlier"),
        "the refused write must not have landed"
    );
}

/// `Utc` is used by the lapse test through sqlx; this keeps the import honest.
#[allow(dead_code)]
fn _uses_utc() -> chrono::DateTime<Utc> {
    Utc::now()
}
