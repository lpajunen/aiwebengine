//! Acting as somebody who is not here.
//!
//! What these cover is the boundary rather than the plumbing: that a task
//! cannot act as a person without a live grant, that withdrawing one reaches
//! work already queued, and that a delegated run gets the person's own storage
//! and secrets without getting their authority to author anything.

mod common;

use aiwebengine::delegation::{self, Scope};
use aiwebengine::tasks::{self, NewTask};
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
        run_as: Some(user_id.to_string()),
        lane: None,
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
        run_as: None,
        lane: None,
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

/// Every scope that is not a management one leaves the cap where it was, so a
/// delegated task belonging to an administrator is not *by itself* a way to
/// get an administrative context.
///
/// This used to hold for `Scope::all()` unconditionally. `Scope::Author` and
/// `Scope::Administer` are the deliberate exception — a person may now consent
/// to exactly this, on a page that says so in those words — and the invariant
/// that replaced the old one is that nothing else confers it and no amount of
/// asking does. Both halves are covered:
/// `an_administrator_can_delegate_administering_and_nobody_else_can` has the
/// other one.
#[tokio::test(flavor = "multi_thread")]
async fn ordinary_scopes_are_not_a_way_to_administer_even_for_an_administrator() {
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
    // Everything a delegation could ask for before the management scopes
    // existed, granted at once, by somebody who holds every role.
    let ordinary_scopes = [Scope::PersonalStorage, Scope::Secrets, Scope::Write];
    delegation::grant(&user_id, script_uri, &ordinary_scopes, Duration::days(1))
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

// ============================================================================
// Work started by an inbound message
// ============================================================================
//
// An inbound webhook arrives with nobody signed in, so there is no session,
// so there is no person — which is what put every channel an agent might
// live on out of reach. What closes that is a link: the person says which
// sender may set their delegated work going, and the script names the sender
// rather than the person.
//
// These cover the boundary that decision creates. The engine cannot verify
// that a message really came from a sender — that is the script's job, and
// the documentation says so — so what it *can* enforce is everything else:
// an unlinked sender reaches nobody, a linked sender reaches exactly one
// person, a grant is still required, and withdrawing either half stops it.

/// A webhook handler, with nobody signed in, running work as the person a
/// message came from. The whole point of the feature.
#[tokio::test(flavor = "multi_thread")]
async fn a_linked_sender_can_start_work_with_nobody_signed_in() {
    setup_env().await;
    let user_id = a_user("linked").await;
    let script_uri = "test://delegation/linked";

    repository::upsert_script(
        script_uri,
        r#"
        function work(context) {
          personalStorage.setItem("heard", context.meta.task.payload.text);
        }
        "#,
    )
    .expect("script should store");

    delegation::grant(
        &user_id,
        script_uri,
        &[Scope::PersonalStorage, Scope::Write],
        Duration::days(1),
    )
    .await
    .expect("granted");
    delegation::bind_channel(&user_id, script_uri, "telegram", "12345")
        .await
        .expect("linked");

    // What the webhook handler would do: resolve the sender, enqueue as them.
    let resolved = delegation::resolve_channel(script_uri, "telegram", "12345")
        .await
        .expect("lookup")
        .expect("the sender is linked");
    assert_eq!(resolved, user_id);

    let task = tasks::enqueue(NewTask {
        payload: json!({ "text": "hello from the chat" }),
        // Nobody asked as themselves: this is the shape `enqueueFrom` builds.
        enqueued_by: None,
        ..personal_task(script_uri, "work", &user_id)
    })
    .await
    .expect("accepted");
    tasks::run_due_now("test-worker").await;

    assert!(
        tasks::get(task.task_id).await.expect("lookup").is_none(),
        "the task should have succeeded"
    );
    assert_eq!(
        repository::get_user_properties_item(script_uri, &user_id, "heard").as_deref(),
        Some("hello from the chat"),
        "the work should have run as the person the message came from"
    );
}

/// The refusal that matters most: a sender nobody linked is nobody. This is
/// what stops a script naming a person it has no message from, and what
/// makes a buggy handler unable to reach past the sender it was given.
#[tokio::test(flavor = "multi_thread")]
async fn an_unlinked_sender_resolves_to_nobody() {
    setup_env().await;
    let user_id = a_user("unlinked").await;
    let script_uri = "test://delegation/unlinked";

    // A live grant, and no link. A grant alone must not be enough — it says
    // the app may act while they are away, not that anyone who can reach the
    // app's routes may choose when.
    delegation::grant(&user_id, script_uri, &Scope::all(), Duration::days(1))
        .await
        .expect("granted");

    assert_eq!(
        delegation::resolve_channel(script_uri, "telegram", "12345")
            .await
            .expect("lookup"),
        None,
        "a grant without a link reaches nobody"
    );
}

/// A link is per script. Being reachable on one solution says nothing about
/// another, exactly as the grant does not carry across.
#[tokio::test(flavor = "multi_thread")]
async fn a_link_does_not_carry_to_another_script() {
    setup_env().await;
    let user_id = a_user("per-script").await;
    let linked = "test://delegation/link-here";
    let other = "test://delegation/link-elsewhere";

    delegation::bind_channel(&user_id, linked, "telegram", "12345")
        .await
        .expect("linked");

    assert_eq!(
        delegation::resolve_channel(other, "telegram", "12345")
            .await
            .expect("lookup"),
        None,
    );
}

/// Two accounts cannot claim one sender on one script: the engine would have
/// no way to say which of them a message is from. The second is refused
/// rather than allowed to replace the first, because silently moving a
/// binding is a takeover.
#[tokio::test(flavor = "multi_thread")]
async fn a_sender_belongs_to_at_most_one_account_per_script() {
    setup_env().await;
    let first = a_user("claimant-one").await;
    let second = a_user("claimant-two").await;
    let script_uri = "test://delegation/contested";

    assert!(
        delegation::bind_channel(&first, script_uri, "telegram", "12345")
            .await
            .expect("the first claim should be recorded")
    );

    assert_eq!(
        delegation::bind_channel(&second, script_uri, "telegram", "12345")
            .await
            .unwrap_err(),
        delegation::ChannelRefusal::TakenByAnother,
    );

    assert_eq!(
        delegation::resolve_channel(script_uri, "telegram", "12345")
            .await
            .expect("lookup")
            .as_deref(),
        Some(first.as_str()),
        "the first claim stands"
    );
}

/// Re-linking your own sender is not an error. A person who authorises the
/// same app twice from the same chat should not see a failure.
#[tokio::test(flavor = "multi_thread")]
async fn linking_your_own_sender_again_is_not_an_error() {
    setup_env().await;
    let user_id = a_user("relink").await;
    let script_uri = "test://delegation/relink";

    assert!(
        delegation::bind_channel(&user_id, script_uri, "telegram", "12345")
            .await
            .expect("first")
    );
    assert!(
        !delegation::bind_channel(&user_id, script_uri, "telegram", "12345")
            .await
            .expect("second"),
        "nothing new was written, and that is not a failure"
    );
}

/// The channel is folded and the sender is not, end to end through the
/// store — so a handler that reports `Telegram` finds what was linked as
/// `telegram`, and one that reports a differently-cased Slack id does not.
#[tokio::test(flavor = "multi_thread")]
async fn a_link_is_found_by_a_folded_channel_and_an_exact_sender() {
    setup_env().await;
    let user_id = a_user("folding").await;
    let script_uri = "test://delegation/folding";

    delegation::bind_channel(&user_id, script_uri, "Telegram", "U123aBc")
        .await
        .expect("linked");

    let (channel, identity) = delegation::normalize_channel("TELEGRAM", "U123aBc").expect("usable");
    assert_eq!(
        delegation::resolve_channel(script_uri, &channel, &identity)
            .await
            .expect("lookup")
            .as_deref(),
        Some(user_id.as_str()),
    );

    let (channel, identity) = delegation::normalize_channel("telegram", "u123abc").expect("usable");
    assert_eq!(
        delegation::resolve_channel(script_uri, &channel, &identity)
            .await
            .expect("lookup"),
        None,
        "a different sender is a different sender"
    );
}

/// Withdrawing the grant takes the links with it. A link that outlived its
/// grant would come back to life the next time that person authorised the
/// script for some unrelated reason.
#[tokio::test(flavor = "multi_thread")]
async fn withdrawing_a_grant_unlinks_its_senders() {
    setup_env().await;
    let user_id = a_user("withdraw-links").await;
    let script_uri = "test://delegation/withdraw-links";

    delegation::grant(&user_id, script_uri, &Scope::all(), Duration::days(1))
        .await
        .expect("granted");
    delegation::bind_channel(&user_id, script_uri, "telegram", "12345")
        .await
        .expect("linked");

    delegation::revoke(&user_id, script_uri)
        .await
        .expect("withdrawn");

    assert_eq!(
        delegation::resolve_channel(script_uri, "telegram", "12345")
            .await
            .expect("lookup"),
        None,
        "the sender should no longer reach anybody"
    );
    assert!(
        delegation::list_channels_for_user(&user_id)
            .await
            .expect("listing")
            .is_empty()
    );
}

/// And "sign out everywhere" reaches them too, for the reason it reaches
/// the grants: this runs when an account's roles change, its realm narrows,
/// or it is deleted, and a session list cannot show a sender that can start
/// background work.
#[tokio::test(flavor = "multi_thread")]
async fn revoking_everything_unlinks_every_sender() {
    setup_env().await;
    let user_id = a_user("revoke-all-links").await;

    for script_uri in ["test://delegation/all-a", "test://delegation/all-b"] {
        delegation::grant(&user_id, script_uri, &Scope::all(), Duration::days(1))
            .await
            .expect("granted");
        delegation::bind_channel(&user_id, script_uri, "telegram", "12345")
            .await
            .expect("linked");
    }

    delegation::revoke_all(&user_id).await.expect("revoked");

    assert!(
        delegation::list_channels_for_user(&user_id)
            .await
            .expect("listing")
            .is_empty()
    );
}

/// Unlinking one sender leaves the grant alone. Somebody who changed phone
/// number wants that and not a withdrawal, and the two are separate buttons
/// on the account page because they are separate decisions.
#[tokio::test(flavor = "multi_thread")]
async fn unlinking_a_sender_leaves_the_grant_standing() {
    setup_env().await;
    let user_id = a_user("unlink-one").await;
    let script_uri = "test://delegation/unlink-one";

    delegation::grant(&user_id, script_uri, &Scope::all(), Duration::days(1))
        .await
        .expect("granted");
    delegation::bind_channel(&user_id, script_uri, "telegram", "old")
        .await
        .expect("linked");
    delegation::bind_channel(&user_id, script_uri, "telegram", "new")
        .await
        .expect("linked");

    assert!(
        delegation::unbind_channel(&user_id, script_uri, "telegram", "old")
            .await
            .expect("unlinked")
    );

    assert_eq!(
        delegation::resolve_channel(script_uri, "telegram", "old")
            .await
            .expect("lookup"),
        None
    );
    assert_eq!(
        delegation::resolve_channel(script_uri, "telegram", "new")
            .await
            .expect("lookup")
            .as_deref(),
        Some(user_id.as_str()),
        "the other sender is untouched"
    );
    assert!(
        delegation::get(&user_id, script_uri)
            .await
            .expect("lookup")
            .is_some(),
        "the grant itself should still stand"
    );
}

/// One account cannot unlink another's sender, however the call is spelled:
/// the delete is scoped to the caller in the statement rather than by a
/// check before it.
#[tokio::test(flavor = "multi_thread")]
async fn unlinking_is_scoped_to_your_own_links() {
    setup_env().await;
    let owner = a_user("link-owner").await;
    let outsider = a_user("link-outsider").await;
    let script_uri = "test://delegation/link-scoped";

    delegation::bind_channel(&owner, script_uri, "telegram", "12345")
        .await
        .expect("linked");

    assert!(
        !delegation::unbind_channel(&outsider, script_uri, "telegram", "12345")
            .await
            .expect("the statement should run and match nothing")
    );
    assert_eq!(
        delegation::resolve_channel(script_uri, "telegram", "12345")
            .await
            .expect("lookup")
            .as_deref(),
        Some(owner.as_str()),
    );
}

/// The JavaScript surface, driven the way a webhook drives it: an anonymous
/// request with no session at all, which is the context every one of these
/// arrives in.
///
/// Three things in one handler, because they are the three answers a bot
/// needs and getting any of them wrong is the whole feature: an unknown
/// sender gets a link to send them, a known one gets work queued as the
/// person, and neither needed anybody to be signed in.
#[tokio::test(flavor = "multi_thread")]
async fn a_webhook_with_no_session_reaches_the_person_a_message_came_from() {
    setup_env().await;
    let user_id = a_user("webhook").await;
    let script_uri = "test://delegation/webhook";

    repository::upsert_script(
        script_uri,
        r#"
        function hook(context) {
          const stranger = personalTasks.sender({
            channel: "telegram", identity: "99999",
          });
          const known = personalTasks.sender({
            channel: "telegram", identity: "12345",
          });
          // What a bot replies to an unknown sender with — into that
          // sender's own chat, which is what proves they own it.
          const invite = personalTasks.inviteLink({
            channel: "telegram", identity: "99999",
          });

          let queued = null;
          let refused = null;
          try {
            queued = personalTasks.enqueueFrom({
              channel: "telegram",
              identity: "12345",
              handler: "runTurn",
              payload: { text: "from the chat" },
            }).taskId;
          } catch (e) {
            refused = String(e.message || e);
          }

          let strangerRefused = null;
          try {
            personalTasks.enqueueFrom({
              channel: "telegram", identity: "99999", handler: "runTurn",
            });
          } catch (e) {
            strangerRefused = String(e.message || e);
          }

          return {
            status: 200,
            contentType: "application/json",
            body: JSON.stringify({
              strangerLinked: stranger.linked,
              strangerLink: invite.linkUrl,
              knownGranted: known.granted,
              queued: queued !== null,
              refused,
              strangerRefused,
            }),
          };
        }

        function runTurn(context) {
          personalStorage.setItem("turn", context.meta.task.payload.text);
        }
        "#,
    )
    .expect("script should store");

    delegation::grant(
        &user_id,
        script_uri,
        &[Scope::PersonalStorage, Scope::Write],
        Duration::days(1),
    )
    .await
    .expect("granted");
    delegation::bind_channel(&user_id, script_uri, "telegram", "12345")
        .await
        .expect("linked");

    let response = tokio::task::spawn_blocking(move || {
        js_engine::execute_script_for_request_secure(js_engine::RequestExecutionParams {
            script_uri: script_uri.to_string(),
            handler_name: "hook".to_string(),
            path: "/hooks/telegram".to_string(),
            method: "POST".to_string(),
            query_params: None,
            url: None,
            form_data: None,
            raw_body: None,
            headers: Default::default(),
            // Nobody signed in. This is the context the whole feature is for.
            user_context: aiwebengine::security::UserContext::anonymous(),
            auth_context: None,
            route_params: None,
            uploaded_files: None,
            request_id: None,
            route_pattern: None,
        })
    })
    .await
    .expect("no panic")
    .expect("the handler should answer");

    let body: serde_json::Value =
        serde_json::from_slice(&response.body).expect("the handler answers JSON");

    assert_eq!(
        body["refused"],
        json!(null),
        "the linked sender should work"
    );
    assert_eq!(body["queued"], json!(true));
    assert_eq!(body["knownGranted"], json!(true));

    // The unknown one gets a page to send them to rather than a person.
    assert_eq!(body["strangerLinked"], json!(false));
    let invitation = body["strangerLink"].as_str().unwrap_or_default();
    assert!(
        invitation.starts_with("/auth/delegate?link=lnk_"),
        "an unknown sender should be answered with an invitation: {}",
        invitation
    );
    // The sender is not in the URL, and that is the point: a link that named
    // it would be one anybody could construct for anybody.
    assert!(
        !invitation.contains("99999"),
        "an invitation must not name the sender it is for: {}",
        invitation
    );
    assert!(
        body["strangerRefused"]
            .as_str()
            .unwrap_or_default()
            .contains("nobody has linked that sender"),
        "an unknown sender must not reach anybody: {}",
        body["strangerRefused"]
    );

    // And the queued work really does act as the person.
    tasks::run_due_now("test-worker").await;
    assert_eq!(
        repository::get_user_properties_item(script_uri, &user_id, "turn").as_deref(),
        Some("from the chat"),
        "the turn should have run as the person the message came from"
    );
}

/// The invitation is what stops a stranger linking somebody else's sender,
/// and the harm it prevents is interception rather than squatting: bind a
/// victim's chat id before they do, and every message they send that bot is
/// processed as *your* turn, with their text landing in your storage.
///
/// So the only way to the consent page for a sender is a token a script
/// minted in reply to a message from it — which means being able to read
/// that sender's messages.
#[tokio::test(flavor = "multi_thread")]
async fn an_invitation_is_needed_and_is_spent_once() {
    setup_env().await;
    let script_uri = "test://delegation/invitation";

    let url = delegation::invite_link(script_uri, "telegram", "12345")
        .await
        .expect("minted");
    let token = url
        .strip_prefix("/auth/delegate?link=")
        .expect("the URL carries the token")
        .to_string();

    // Reading it does not spend it: the sign-in redirect, the back button
    // and a reload all reach the page before anybody has agreed to anything.
    for _ in 0..3 {
        assert_eq!(
            delegation::peek_invite(&token).await,
            Some((
                script_uri.to_string(),
                "telegram".to_string(),
                "12345".to_string()
            )),
        );
    }

    assert_eq!(
        delegation::spend_invite(&token).await,
        Some((
            script_uri.to_string(),
            "telegram".to_string(),
            "12345".to_string()
        )),
    );

    // Single use. A link forwarded on after somebody used it links nothing,
    // and two browsers racing on one cannot both bind.
    assert_eq!(delegation::spend_invite(&token).await, None);
    assert_eq!(delegation::peek_invite(&token).await, None);
}

/// A guessed token is nothing. The sender is not in the URL, so there is
/// nothing to construct one from.
#[tokio::test(flavor = "multi_thread")]
async fn an_invented_invitation_names_nothing() {
    setup_env().await;
    assert_eq!(delegation::peek_invite("lnk_not-a-real-token").await, None);
    assert_eq!(delegation::spend_invite("").await, None);
}

/// Minting again invalidates the link already sent. That is what somebody
/// re-requesting a link expects, and it bounds the table at one live row per
/// sender rather than one per message.
#[tokio::test(flavor = "multi_thread")]
async fn minting_again_replaces_the_link_already_outstanding() {
    setup_env().await;
    let script_uri = "test://delegation/remint";

    let first = delegation::invite_link(script_uri, "telegram", "12345")
        .await
        .expect("minted");
    let second = delegation::invite_link(script_uri, "telegram", "12345")
        .await
        .expect("minted again");
    assert_ne!(first, second, "each invitation is its own secret");

    let stale = first
        .strip_prefix("/auth/delegate?link=")
        .expect("the URL carries the token");
    assert_eq!(
        delegation::peek_invite(stale).await,
        None,
        "the earlier link should have stopped working"
    );

    let live = second
        .strip_prefix("/auth/delegate?link=")
        .expect("the URL carries the token");
    assert!(delegation::peek_invite(live).await.is_some());
}

/// An invitation names its own script, so one minted by a chatty solution
/// cannot be redeemed against a different one by editing the form.
#[tokio::test(flavor = "multi_thread")]
async fn an_invitation_is_bound_to_the_script_that_minted_it() {
    setup_env().await;
    let minted_by = "test://delegation/invite-mine";

    let url = delegation::invite_link(minted_by, "telegram", "12345")
        .await
        .expect("minted");
    let token = url
        .strip_prefix("/auth/delegate?link=")
        .expect("the URL carries the token");

    let (for_script, _, _) = delegation::peek_invite(token).await.expect("readable");
    assert_eq!(
        for_script, minted_by,
        "the consent page compares this against the script it is showing"
    );
}

/// A link is not a grant. Somebody who unlinked their chat still has the
/// app authorised, and somebody whose authorisation lapsed cannot be
/// triggered however well-known their sender is.
#[tokio::test(flavor = "multi_thread")]
async fn a_link_without_a_live_grant_starts_nothing() {
    setup_env().await;
    let user_id = a_user("lapsed-link").await;
    let script_uri = "test://delegation/lapsed-link";

    repository::upsert_script(
        script_uri,
        r#"
        function hook(context) {
          try {
            personalTasks.enqueueFrom({
              channel: "telegram", identity: "12345", handler: "runTurn",
            });
            return { status: 200, body: "queued", contentType: "text/plain" };
          } catch (e) {
            return { status: 200, body: String(e.message || e), contentType: "text/plain" };
          }
        }
        function runTurn() {}
        "#,
    )
    .expect("script should store");

    // Linked, and authorised until a minute ago.
    delegation::bind_channel(&user_id, script_uri, "telegram", "12345")
        .await
        .expect("linked");
    delegation::grant(&user_id, script_uri, &Scope::all(), Duration::minutes(-1))
        .await
        .expect("granted");

    let response = tokio::task::spawn_blocking(move || {
        js_engine::execute_script_for_request_secure(js_engine::RequestExecutionParams {
            script_uri: script_uri.to_string(),
            handler_name: "hook".to_string(),
            path: "/hooks/telegram".to_string(),
            method: "POST".to_string(),
            query_params: None,
            url: None,
            form_data: None,
            raw_body: None,
            headers: Default::default(),
            user_context: aiwebengine::security::UserContext::anonymous(),
            auth_context: None,
            route_params: None,
            uploaded_files: None,
            request_id: None,
            route_pattern: None,
        })
    })
    .await
    .expect("no panic")
    .expect("the handler should answer");

    let body = String::from_utf8_lossy(&response.body).to_string();
    assert!(
        body.contains("has expired"),
        "a lapsed grant must not be startable by a linked sender: {}",
        body
    );
}

/// `Utc` is used by the lapse test through sqlx; this keeps the import honest.
#[allow(dead_code)]
fn _uses_utc() -> chrono::DateTime<Utc> {
    Utc::now()
}

/// Administering the engine while the person is away, which the cap used to
/// refuse outright.
///
/// The change this covers is the one thing `delegation.rs` never allowed: a
/// delegated run reaching past `authenticated`. It reaches only as far as the
/// account's own roles, and only because the person ticked the box — so this
/// walks the whole path, storing roles on a real account and resolving a real
/// grant, rather than asserting about `context_for` in isolation.
#[tokio::test(flavor = "multi_thread")]
async fn an_administrator_can_delegate_administering_and_nobody_else_can() {
    setup_env().await;

    let script_uri = "test://delegation/administer";
    let administrator = a_user("administers").await;
    let ordinary = a_user("ordinary").await;

    aiwebengine::user_repository::update_user_roles(
        &administrator,
        vec![aiwebengine::user_repository::UserRole::Administrator],
    )
    .expect("the role should be stored");

    for user in [&administrator, &ordinary] {
        delegation::grant(user, script_uri, &[Scope::Administer], Duration::days(1))
            .await
            .expect("the grant should be recorded");
    }

    let elevated = delegation::resolve(&administrator, script_uri)
        .await
        .expect("a live grant should resolve");
    assert!(
        elevated
            .user_context
            .has_capability(&aiwebengine::security::Capability::AdministerEngine),
        "an administrator who consented should be able to administer"
    );

    let refused = delegation::resolve(&ordinary, script_uri)
        .await
        .expect("a live grant should resolve");
    assert!(
        !refused
            .user_context
            .has_capability(&aiwebengine::security::Capability::AdministerEngine),
        "consenting is not a promotion: an ordinary account grants nothing here"
    );
}

/// Taking the role away ends the delegation outright.
///
/// Two things stop it, and the belt is the interesting one.
/// `update_user_roles` calls `security::delete_sessions_for_user`, which
/// withdraws every delegation the account had — so a demotion does not wait
/// for a grant to lapse, and this is what the test observes.
///
/// Underneath that, `resolve` reads the roles as they stand when the task
/// runs rather than as they were when the grant was recorded, so a role change
/// reaching the account by any path that did *not* revoke would still take
/// effect. That one is covered by `consenting_to_more_than_you_hold_grants_nothing`
/// in `delegation.rs`, which can describe roles the repository never held.
#[tokio::test(flavor = "multi_thread")]
async fn losing_the_role_ends_the_delegated_authority() {
    setup_env().await;

    let script_uri = "test://delegation/demoted";
    let user = a_user("demoted").await;

    aiwebengine::user_repository::update_user_roles(
        &user,
        vec![aiwebengine::user_repository::UserRole::Administrator],
    )
    .expect("the role should be stored");

    delegation::grant(&user, script_uri, &[Scope::Administer], Duration::days(1))
        .await
        .expect("the grant should be recorded");

    assert!(
        delegation::resolve(&user, script_uri)
            .await
            .expect("resolves")
            .user_context
            .has_capability(&aiwebengine::security::Capability::AdministerEngine),
        "the grant should work while the role is held"
    );

    aiwebengine::user_repository::update_user_roles(&user, vec![])
        .expect("the role should be removed");

    assert!(
        matches!(
            delegation::resolve(&user, script_uri).await,
            Err(delegation::Refusal::NoGrant)
        ),
        "changing what an account may do should withdraw what it delegated"
    );
}

/// An editor's delegated authoring reaches their own scripts and stops at
/// administering, which is the split the two scopes exist to draw.
#[tokio::test(flavor = "multi_thread")]
async fn delegated_authoring_stops_short_of_administering() {
    setup_env().await;

    let script_uri = "test://delegation/authors";
    let user = a_user("authors").await;

    aiwebengine::user_repository::update_user_roles(
        &user,
        vec![aiwebengine::user_repository::UserRole::Editor],
    )
    .expect("the role should be stored");

    delegation::grant(&user, script_uri, &[Scope::Author], Duration::days(1))
        .await
        .expect("the grant should be recorded");

    let context = delegation::resolve(&user, script_uri)
        .await
        .expect("resolves")
        .user_context;

    assert!(context.has_capability(&aiwebengine::security::Capability::WriteScripts));
    assert!(!context.has_capability(&aiwebengine::security::Capability::AdministerEngine));
}
