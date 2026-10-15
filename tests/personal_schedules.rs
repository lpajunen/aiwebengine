//! Schedules that belong to a person: declared by them, run as them, and
//! bounded by their grant.
//!
//! What these cover is what makes a personal schedule different from a task
//! chain or a script's own schedule: saying it again replaces it, a due slot
//! becomes exactly one delegated task, it never stacks behind itself, and it
//! follows the grant — deleted on withdrawal, paused on expiry, resumed on
//! renewal.

mod common;

use aiwebengine::delegation::{self, Scope};
use aiwebengine::schedules::{self, ScheduleSpec};
use aiwebengine::tasks::{self, NewTask};
use aiwebengine::{js_engine, repository};
use chrono::{Duration, Utc};
use common::setup_env;
use serde_json::{Value, json};
use sqlx::Row;

async fn a_user(label: &str) -> String {
    aiwebengine::user_repository::upsert_internal_user(
        Some(format!("schedule test {}", label)),
        "test".to_string(),
        format!("{}-{}", label, uuid::Uuid::new_v4()),
        "localhost".to_string(),
    )
    .await
    .expect("the account should be created")
}

async fn granted(user_id: &str, script_uri: &str) {
    delegation::grant(user_id, script_uri, &Scope::all(), Duration::days(30))
        .await
        .expect("granted");
}

/// Declare straight through the module, as the global does once it has
/// established who is asking.
async fn declare(
    user_id: &str,
    script_uri: &str,
    spec: Value,
) -> Result<schedules::Schedule, schedules::ScheduleError> {
    declare_reporting(user_id, script_uri, spec)
        .await
        .map(|declared| declared.schedule)
}

/// As [`declare`], with whether anything was written.
async fn declare_reporting(
    user_id: &str,
    script_uri: &str,
    spec: Value,
) -> Result<schedules::Declared, schedules::ScheduleError> {
    let limits = schedules::limits();
    let spec = ScheduleSpec::from_json(&spec, limits.min_interval_ms)?;
    let pool = common::test_pool().await;
    let mut conn = pool.acquire().await.expect("a connection");
    let lane = format!("person:{}", user_id);
    schedules::declare_on(&mut conn, user_id, script_uri, &spec, Some(&lane), &limits).await
}

fn hourly(key: &str, handler: &str) -> Value {
    json!({ "key": key, "handler": handler, "intervalMinutes": 60 })
}

/// Move a schedule's next slot into the past, the way time would.
async fn make_due(script_uri: &str, ago: Duration) {
    sqlx::query("UPDATE schedules SET next_run_at = $2 WHERE script_uri = $1")
        .bind(script_uri)
        .bind(Utc::now() - ago)
        .execute(&common::test_pool().await)
        .await
        .expect("the schedule should become due");
}

async fn tasks_for(script_uri: &str) -> Vec<tasks::Task> {
    tasks::list(script_uri, 100).await.expect("tasks list")
}

async fn schedule_row(script_uri: &str, key: &str) -> Option<sqlx::postgres::PgRow> {
    sqlx::query(
        "SELECT state, paused_reason, consecutive_failures, next_run_at, interval_ms, cron, \
         handler_name, last_task_id, updated_at FROM schedules \
         WHERE script_uri = $1 AND schedule_key = $2",
    )
    .bind(script_uri)
    .bind(key)
    .fetch_optional(&common::test_pool().await)
    .await
    .expect("lookup")
}

async fn schedule_count(script_uri: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM schedules WHERE script_uri = $1")
        .bind(script_uri)
        .fetch_one(&common::test_pool().await)
        .await
        .expect("count")
}

/// Run a handler as `user_id` in their own request, and answer its body.
async fn request_as(script_uri: &str, user_id: &str, handler: &str) -> String {
    let auth = aiwebengine::auth::JsAuthContext::authenticated(
        user_id.to_string(),
        None,
        None,
        "test".to_string(),
        false,
        false,
    );
    let script_uri = script_uri.to_string();
    let handler = handler.to_string();
    let user_id = user_id.to_string();
    let response = tokio::task::spawn_blocking(move || {
        js_engine::execute_script_for_request_secure(js_engine::RequestExecutionParams {
            script_uri,
            handler_name: handler,
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
    })
    .await
    .expect("no panic")
    .expect("the handler should answer");
    String::from_utf8_lossy(&response.body).to_string()
}

/// Run a fired task to the end, skipping its retry backoff.
async fn run_to_the_end(task_id: uuid::Uuid) -> Option<tasks::Task> {
    for _ in 0..5 {
        sqlx::query("UPDATE script_tasks SET run_at = NOW() WHERE task_id = $1")
            .bind(task_id)
            .execute(&common::test_pool().await)
            .await
            .expect("the retry should become due");
        tasks::run_due_now("test-worker").await;
        match tasks::get(task_id).await.expect("lookup") {
            Some(task) if task.state == "pending" => continue,
            other => return other,
        }
    }
    tasks::get(task_id).await.expect("lookup")
}

/// The JavaScript surface, end to end: declaring twice under one key leaves
/// one row with the second spec, and `schedules()` reads it back.
#[tokio::test(flavor = "multi_thread")]
async fn declaring_twice_under_one_key_replaces_the_first() {
    setup_env().await;
    let user_id = a_user("restate").await;
    let script_uri = "test://schedules/restate";
    repository::upsert_script(
        script_uri,
        r#"
        function first() {
          personalTasks.schedule({ key: "digest", handler: "work", intervalMinutes: 60 });
          return { status: 200, body: "ok" };
        }
        function second() {
          const s = personalTasks.schedule({
            key: "digest", handler: "work", cron: "0 7 * * *", timeZone: "Europe/Helsinki",
          });
          const all = personalTasks.schedules();
          return { status: 200, body: JSON.stringify({ s, all }) };
        }
        function work() {}
        "#,
    )
    .expect("script should store");
    granted(&user_id, script_uri).await;

    assert_eq!(request_as(script_uri, &user_id, "first").await, "ok");
    let body: Value =
        serde_json::from_str(&request_as(script_uri, &user_id, "second").await).expect("json");

    assert_eq!(schedule_count(script_uri).await, 1);
    assert_eq!(body["s"]["cron"], "0 7 * * *");
    assert_eq!(body["s"]["timeZone"], "Europe/Helsinki");
    assert_eq!(body["s"]["intervalMinutes"], Value::Null);
    assert_eq!(body["all"].as_array().map(Vec::len), Some(1));
    assert!(
        body["s"].get("userId").is_none(),
        "a script is never handed an account id"
    );
}

/// Restating the same cadence keeps the next slot, so a script that calls
/// `schedule` on every visit does not push an interval forward for ever.
#[tokio::test(flavor = "multi_thread")]
async fn restating_the_same_cadence_keeps_the_next_slot() {
    setup_env().await;
    let user_id = a_user("keep-slot").await;
    let script_uri = "test://schedules/keep-slot";
    repository::upsert_script(script_uri, "function work() {}").expect("script should store");

    let first = declare(&user_id, script_uri, hourly("k", "work"))
        .await
        .expect("declared");
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    let again = declare(&user_id, script_uri, hourly("k", "work"))
        .await
        .expect("declared again");
    assert_eq!(first.next_run_at, again.next_run_at);

    let changed = declare(
        &user_id,
        script_uri,
        json!({ "key": "k", "handler": "work", "intervalMinutes": 120 }),
    )
    .await
    .expect("declared with a new cadence");
    assert!(changed.next_run_at > first.next_run_at);
}

/// Saying exactly what is already running writes nothing — no row update, so
/// no audit line per visit — while any real change is written.
#[tokio::test(flavor = "multi_thread")]
async fn an_identical_restatement_writes_nothing() {
    setup_env().await;
    let user_id = a_user("restate-noop").await;
    let script_uri = "test://schedules/restate-noop";
    repository::upsert_script(script_uri, "function work() {}").expect("script should store");
    let updated_at = |row: sqlx::postgres::PgRow| row.get::<chrono::DateTime<Utc>, _>("updated_at");
    let changed = |spec: Value| {
        let user_id = user_id.clone();
        async move {
            declare_reporting(&user_id, script_uri, spec)
                .await
                .expect("declared")
                .changed
        }
    };

    assert!(changed(hourly("k", "work")).await);
    let before = updated_at(schedule_row(script_uri, "k").await.expect("a row"));
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;

    assert!(!changed(hourly("k", "work")).await);
    let after = updated_at(schedule_row(script_uri, "k").await.expect("a row"));
    assert_eq!(before, after, "an identical restatement must not write");

    let payload =
        json!({ "key": "k", "handler": "work", "intervalMinutes": 60, "payload": { "a": 1 } });
    assert!(changed(payload).await);
}

/// A due slot becomes exactly one task, as the person, in their lane — even
/// with two workers firing at once.
#[tokio::test(flavor = "multi_thread")]
async fn a_due_schedule_fires_one_task_as_the_person() {
    setup_env().await;
    let user_id = a_user("fires").await;
    let script_uri = "test://schedules/fires";
    repository::upsert_script(script_uri, "function work() {}").expect("script should store");
    granted(&user_id, script_uri).await;

    declare(&user_id, script_uri, hourly("k", "work"))
        .await
        .expect("declared");
    make_due(script_uri, Duration::seconds(1)).await;

    let now = Utc::now();
    let (a, b) = tokio::join!(schedules::fire_due(now), schedules::fire_due(now));
    assert_eq!(a + b, 1, "two workers must fire one slot once");

    let queued = tasks_for(script_uri).await;
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].run_as.as_deref(), Some(user_id.as_str()));
    assert_eq!(queued[0].lane, Some(format!("person:{}", user_id)));

    let row = schedule_row(script_uri, "k").await.expect("row");
    let next: chrono::DateTime<Utc> = row.get("next_run_at");
    assert!(next > Utc::now(), "the next slot is in the future");
}

/// The handler is told which slot of which schedule it is running for.
#[tokio::test(flavor = "multi_thread")]
async fn a_scheduled_run_sees_its_schedule_in_meta() {
    setup_env().await;
    let user_id = a_user("meta").await;
    let script_uri = "test://schedules/meta";
    repository::upsert_script(
        script_uri,
        r#"
        function work(context) {
          const s = context.meta.schedule;
          console.log("slot " + s.key + " " + (typeof s.scheduledFor) + " as " +
            context.request.auth.userId);
        }
        "#,
    )
    .expect("script should store");
    repository::clear_log_messages(script_uri).expect("logs should clear");
    granted(&user_id, script_uri).await;

    declare(&user_id, script_uri, hourly("morning", "work"))
        .await
        .expect("declared");
    make_due(script_uri, Duration::seconds(1)).await;
    schedules::fire_due(Utc::now()).await;
    tasks::run_due_now("test-worker").await;

    let expected = format!("slot morning string as {}", user_id);
    let logs = repository::fetch_log_messages(script_uri);
    assert!(
        logs.iter().any(|entry| entry.message == expected),
        "the handler should see its slot: {:?}",
        logs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
}

/// Work that takes longer than its interval does not pile up behind itself.
#[tokio::test(flavor = "multi_thread")]
async fn a_slot_whose_previous_run_is_unfinished_is_skipped() {
    setup_env().await;
    let user_id = a_user("skip").await;
    let script_uri = "test://schedules/skip";
    repository::upsert_script(script_uri, "function work() {}").expect("script should store");
    granted(&user_id, script_uri).await;

    declare(&user_id, script_uri, hourly("k", "work"))
        .await
        .expect("declared");
    make_due(script_uri, Duration::seconds(1)).await;
    assert_eq!(schedules::fire_due(Utc::now()).await, 1);

    // The first run has not been picked up yet, and the next slot comes due.
    make_due(script_uri, Duration::seconds(1)).await;
    assert_eq!(schedules::fire_due(Utc::now()).await, 0);
    assert_eq!(tasks_for(script_uri).await.len(), 1, "skipped, not stacked");

    let row = schedule_row(script_uri, "k").await.expect("row");
    let next: chrono::DateTime<Utc> = row.get("next_run_at");
    assert!(next > Utc::now(), "a skipped slot still advances");
}

/// After downtime, one run rather than one per missed slot.
#[tokio::test(flavor = "multi_thread")]
async fn after_a_gap_of_several_slots_it_runs_once() {
    setup_env().await;
    let user_id = a_user("gap").await;
    let script_uri = "test://schedules/gap";
    repository::upsert_script(script_uri, "function work() {}").expect("script should store");
    granted(&user_id, script_uri).await;

    declare(&user_id, script_uri, hourly("k", "work"))
        .await
        .expect("declared");
    make_due(script_uri, Duration::hours(5) + Duration::minutes(30)).await;

    assert_eq!(schedules::fire_due(Utc::now()).await, 1);
    assert_eq!(schedules::fire_due(Utc::now()).await, 0);
    assert_eq!(tasks_for(script_uri).await.len(), 1);

    let row = schedule_row(script_uri, "k").await.expect("row");
    let next: chrono::DateTime<Utc> = row.get("next_run_at");
    assert!(next > Utc::now());
    assert!(
        next <= Utc::now() + Duration::minutes(31),
        "still on its grid"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn withdrawing_the_grant_deletes_the_schedules() {
    setup_env().await;
    let user_id = a_user("revoke").await;
    let script_uri = "test://schedules/revoke";
    repository::upsert_script(script_uri, "function work() {}").expect("script should store");
    granted(&user_id, script_uri).await;
    declare(&user_id, script_uri, hourly("k", "work"))
        .await
        .expect("declared");

    delegation::revoke(&user_id, script_uri)
        .await
        .expect("revoked");
    assert_eq!(schedule_count(script_uri).await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn signing_out_everywhere_deletes_every_schedule() {
    setup_env().await;
    let user_id = a_user("revoke-all").await;
    let script_uri = "test://schedules/revoke-all";
    repository::upsert_script(script_uri, "function work() {}").expect("script should store");
    granted(&user_id, script_uri).await;
    declare(&user_id, script_uri, hourly("k", "work"))
        .await
        .expect("declared");

    aiwebengine::security::delete_sessions_for_user(&common::test_pool().await, &user_id)
        .await
        .expect("sign-out should run");
    assert_eq!(schedule_count(script_uri).await, 0);
}

/// Expiry is a deadline passing, not the person saying stop: the schedule
/// pauses, and giving the grant again resumes it.
#[tokio::test(flavor = "multi_thread")]
async fn expiry_pauses_and_granting_again_resumes() {
    setup_env().await;
    let user_id = a_user("expiry").await;
    let script_uri = "test://schedules/expiry";
    repository::upsert_script(script_uri, "function work() {}").expect("script should store");
    granted(&user_id, script_uri).await;
    declare(&user_id, script_uri, hourly("k", "work"))
        .await
        .expect("declared");

    sqlx::query(
        "UPDATE script_delegations SET expires_at = NOW() - INTERVAL '1 hour' WHERE user_id = $1",
    )
    .bind(&user_id)
    .execute(&common::test_pool().await)
    .await
    .expect("the grant should lapse");
    make_due(script_uri, Duration::seconds(1)).await;

    assert_eq!(schedules::fire_due(Utc::now()).await, 0);
    assert!(tasks_for(script_uri).await.is_empty(), "no pointless work");
    let row = schedule_row(script_uri, "k").await.expect("kept");
    assert_eq!(row.get::<String, _>("state"), "paused");
    assert_eq!(
        row.get::<Option<String>, _>("paused_reason").as_deref(),
        Some("expired")
    );

    let resuming = schedules::paused_expired(&user_id, script_uri)
        .await
        .expect("listed");
    assert_eq!(resuming.len(), 1, "the consent page lists what will resume");

    granted(&user_id, script_uri).await;
    let row = schedule_row(script_uri, "k").await.expect("kept");
    assert_eq!(row.get::<String, _>("state"), "active");
}

#[tokio::test(flavor = "multi_thread")]
async fn deleting_the_script_or_the_account_removes_the_rows() {
    setup_env().await;
    let pool = common::test_pool().await;

    let user_id = a_user("delete-script").await;
    let script_uri = "test://schedules/delete-script";
    repository::upsert_script(script_uri, "function work() {}").expect("script should store");
    declare(&user_id, script_uri, hourly("k", "work"))
        .await
        .expect("declared");
    sqlx::query("DELETE FROM scripts WHERE uri = $1")
        .bind(script_uri)
        .execute(&pool)
        .await
        .expect("the script should delete");
    assert_eq!(schedule_count(script_uri).await, 0);

    let user_id = a_user("delete-account").await;
    let script_uri = "test://schedules/delete-account";
    repository::upsert_script(script_uri, "function work() {}").expect("script should store");
    declare(&user_id, script_uri, hourly("k", "work"))
        .await
        .expect("declared");
    sqlx::query("DELETE FROM users WHERE user_id = $1")
        .bind(&user_id)
        .execute(&pool)
        .await
        .expect("the account should delete");
    assert_eq!(schedule_count(script_uri).await, 0);
}

/// A handler that is gone, or a feed that keeps failing, stops costing runs.
#[tokio::test(flavor = "multi_thread")]
async fn three_given_up_runs_pause_it_and_a_success_resets_the_count() {
    setup_env().await;
    let user_id = a_user("failing").await;
    let script_uri = "test://schedules/failing";
    repository::upsert_script(
        script_uri,
        r#"
        function work() {
          if (personalStorage.getItem("ok") !== "yes") { throw new Error("feed is gone"); }
        }
        function fix() { personalStorage.setItem("ok", "yes"); return { status: 200, body: "" }; }
        "#,
    )
    .expect("script should store");
    granted(&user_id, script_uri).await;
    declare(&user_id, script_uri, hourly("k", "work"))
        .await
        .expect("declared");

    async fn one_slot(script_uri: &str) -> tasks::Task {
        make_due(script_uri, Duration::seconds(1)).await;
        assert_eq!(schedules::fire_due(Utc::now()).await, 1);
        let row = schedule_row(script_uri, "k").await.expect("row");
        let task_id: uuid::Uuid = row
            .get::<Option<uuid::Uuid>, _>("last_task_id")
            .expect("fired");
        run_to_the_end(task_id)
            .await
            .expect("a given-up task keeps its row")
    }

    let first = one_slot(script_uri).await;
    assert_eq!(first.state, "failed");
    assert_eq!(first.attempts, 3);
    let row = schedule_row(script_uri, "k").await.expect("row");
    assert_eq!(row.get::<i32, _>("consecutive_failures"), 1);

    // A success in between resets the count.
    request_as(script_uri, &user_id, "fix").await;
    make_due(script_uri, Duration::seconds(1)).await;
    schedules::fire_due(Utc::now()).await;
    let task_id: uuid::Uuid = schedule_row(script_uri, "k")
        .await
        .expect("row")
        .get::<Option<uuid::Uuid>, _>("last_task_id")
        .expect("fired");
    assert!(
        run_to_the_end(task_id).await.is_none(),
        "a success deletes its row"
    );
    let row = schedule_row(script_uri, "k").await.expect("row");
    assert_eq!(row.get::<i32, _>("consecutive_failures"), 0);

    repository::set_user_properties_item(script_uri, &user_id, "ok", "no").expect("broken again");
    for _ in 0..3 {
        one_slot(script_uri).await;
    }
    let row = schedule_row(script_uri, "k").await.expect("row");
    assert_eq!(row.get::<String, _>("state"), "paused");
    assert_eq!(
        row.get::<Option<String>, _>("paused_reason").as_deref(),
        Some("failing")
    );
    make_due(script_uri, Duration::seconds(1)).await;
    assert_eq!(schedules::fire_due(Utc::now()).await, 0);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_limits_refuse_the_eleventh_the_fifty_first_and_every_minute() {
    setup_env().await;
    let user_id = a_user("limits").await;

    let every_minute = declare(
        &user_id,
        "test://schedules/limits-0",
        json!({ "key": "k", "handler": "work", "cron": "* * * * *", "timeZone": "UTC" }),
    )
    .await;
    assert!(matches!(
        every_minute,
        Err(schedules::ScheduleError::TooFrequent(_))
    ));

    for script in 0..5 {
        let script_uri = format!("test://schedules/limits-{}", script);
        repository::upsert_script(&script_uri, "function work() {}").expect("script should store");
        for key in 0..10 {
            declare(&user_id, &script_uri, hourly(&format!("k{}", key), "work"))
                .await
                .expect("within the limits");
        }
        let eleventh = declare(&user_id, &script_uri, hourly("k10", "work")).await;
        assert_eq!(
            eleventh.unwrap_err(),
            schedules::ScheduleError::TooManyForScript(10)
        );
        // Restating one already held is not another.
        declare(&user_id, &script_uri, hourly("k0", "work"))
            .await
            .expect("restating is not adding");
    }

    let script_uri = "test://schedules/limits-5";
    repository::upsert_script(script_uri, "function work() {}").expect("script should store");
    let fifty_first = declare(&user_id, script_uri, hourly("k", "work")).await;
    assert_eq!(
        fifty_first.unwrap_err(),
        schedules::ScheduleError::TooManyForPerson(50)
    );
}

/// The same rule as `enqueue`: no grant, no schedule; and a read-only
/// delegated run has lost the queue, so it has lost schedules too.
#[tokio::test(flavor = "multi_thread")]
async fn schedule_is_refused_without_a_grant_and_from_a_read_only_run() {
    setup_env().await;
    let user_id = a_user("refused").await;
    let script_uri = "test://schedules/refused";
    repository::upsert_script(
        script_uri,
        r#"
        function attempt() {
          try {
            personalTasks.schedule({ key: "k", handler: "work", intervalMinutes: 60 });
            return { status: 200, body: "scheduled" };
          } catch (e) {
            return { status: 200, body: e.name + ": " + e.message };
          }
        }
        function work() {
          try {
            personalTasks.schedule({ key: "k", handler: "work", intervalMinutes: 60 });
            console.log("scheduled from a read-only run");
          } catch (e) {
            console.log("refused: " + e.message);
          }
        }
        "#,
    )
    .expect("script should store");
    repository::clear_log_messages(script_uri).expect("logs should clear");

    let body = request_as(script_uri, &user_id, "attempt").await;
    assert!(
        body.starts_with("SecurityError") && body.contains("has not authorised"),
        "{body}"
    );
    assert_eq!(schedule_count(script_uri).await, 0);

    delegation::grant(
        &user_id,
        script_uri,
        &[Scope::PersonalStorage],
        Duration::days(1),
    )
    .await
    .expect("granted");
    tasks::enqueue(NewTask {
        script_uri: script_uri.to_string(),
        handler_name: "work".to_string(),
        payload: json!({}),
        run_at: None,
        max_attempts: Some(1),
        enqueued_by: Some(user_id.clone()),
        run_as: Some(user_id.clone()),
        lane: None,
    })
    .await
    .expect("accepted");
    tasks::run_due_now("test-worker").await;

    let logs = repository::fetch_log_messages(script_uri);
    assert!(
        logs.iter()
            .any(|e| e.message.starts_with("refused:") && e.message.contains("enqueue_tasks")),
        "{:?}",
        logs.iter().map(|e| &e.message).collect::<Vec<_>>()
    );
    assert_eq!(schedule_count(script_uri).await, 0);
}

/// A webhook names a sender, never an account: an unlinked one schedules
/// nothing, and a linked one schedules for the person who linked it.
#[tokio::test(flavor = "multi_thread")]
async fn schedule_from_resolves_the_sender_and_refuses_an_unlinked_one() {
    setup_env().await;
    let user_id = a_user("from").await;
    let script_uri = "test://schedules/from";
    repository::upsert_script(
        script_uri,
        r#"
        function webhook(context) {
          const identity = context.request.query.identity;
          try {
            const s = personalTasks.scheduleFrom({
              channel: "telegram", identity,
              key: "k", handler: "work", intervalMinutes: 60,
            });
            return { status: 200, body: "scheduled " + s.key };
          } catch (e) {
            return { status: 200, body: e.name + ": " + e.message };
          }
        }
        function work() {}
        "#,
    )
    .expect("script should store");
    granted(&user_id, script_uri).await;
    delegation::bind_channel(&user_id, script_uri, "telegram", "12345")
        .await
        .expect("linked");

    async fn hook(script_uri: &'static str, identity: &str) -> String {
        let mut query = std::collections::HashMap::new();
        query.insert("identity".to_string(), identity.to_string());
        let response = tokio::task::spawn_blocking(move || {
            js_engine::execute_script_for_request_secure(js_engine::RequestExecutionParams {
                script_uri: script_uri.to_string(),
                handler_name: "webhook".to_string(),
                path: "/hook".to_string(),
                method: "POST".to_string(),
                query_params: Some(query),
                url: None,
                form_data: None,
                raw_body: None,
                headers: Default::default(),
                // Nobody signed in, as for every inbound webhook.
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
        String::from_utf8_lossy(&response.body).to_string()
    }

    let stranger = hook(script_uri, "99999").await;
    assert!(
        stranger.starts_with("SecurityError") && stranger.contains("nobody has linked"),
        "{stranger}"
    );
    assert_eq!(schedule_count(script_uri).await, 0);

    assert_eq!(hook(script_uri, "12345").await, "scheduled k");
    let owner: String = sqlx::query_scalar("SELECT user_id FROM schedules WHERE script_uri = $1")
        .bind(script_uri)
        .fetch_one(&common::test_pool().await)
        .await
        .expect("stored");
    assert_eq!(owner, user_id);
}

/// It belongs to the person, not the code version.
#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_set_before_a_redeployment_still_fires_after_it() {
    setup_env().await;
    let user_id = a_user("redeploy").await;
    let script_uri = "test://schedules/redeploy";
    repository::upsert_script(script_uri, "function work() {}").expect("script should store");
    granted(&user_id, script_uri).await;
    declare(&user_id, script_uri, hourly("k", "work"))
        .await
        .expect("declared");

    repository::upsert_script(script_uri, "function work() { console.log('v2'); }")
        .expect("redeployed");
    make_due(script_uri, Duration::seconds(1)).await;
    assert_eq!(schedules::fire_due(Utc::now()).await, 1);
}

/// The account page lists a schedule under its grant, and the person can
/// pause, resume and delete it — their own and nobody else's.
#[tokio::test(flavor = "multi_thread")]
async fn the_account_page_lists_pauses_resumes_and_deletes_a_schedule() {
    let engine = common::AdminServer::start()
        .await
        .expect("server failed to start");
    let http = engine.anonymous().clone();

    let username = format!("sched{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let registered = http
        .post(engine.url("/auth/local/register"))
        .json(&json!({ "username": username, "password": "a-perfectly-fine-password" }))
        .send()
        .await
        .expect("registration should answer");
    assert!(registered.status().is_success());
    let cookie = registered
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::to_string)
        .expect("a session");
    let status: Value = http
        .get(engine.url("/auth/status"))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .expect("status")
        .json()
        .await
        .expect("json");
    let user_id = status["user_id"]
        .as_str()
        .or_else(|| status["userId"].as_str())
        .expect("signed in")
        .to_string();

    let script_uri = "test://schedules/account-page";
    repository::upsert_script(script_uri, "function work() {}").expect("script should store");
    granted(&user_id, script_uri).await;
    let schedule = declare(
        &user_id,
        script_uri,
        json!({
            "key": "morning-digest", "handler": "work",
            "cron": "0 7 * * *", "timeZone": "Europe/Helsinki",
        }),
    )
    .await
    .expect("declared");

    let html = http
        .get(engine.url("/auth/account"))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .expect("the account page should answer")
        .text()
        .await
        .expect("a body");
    assert!(html.contains("morning-digest"), "the schedule is listed");
    assert!(html.contains("daily at 07:00 (Europe/Helsinki)"));

    let marker = r#"name="csrf_token" value=""#;
    let start = html.find(marker).expect("a token") + marker.len();
    let csrf = html[start..][..html[start..].find('"').expect("quoted")].to_string();

    let act = |action: &'static str, id: String| {
        let http = http.clone();
        let cookie = cookie.clone();
        let csrf = csrf.clone();
        let url = engine.url("/auth/delegations/schedule");
        async move {
            http.post(url)
                .header(reqwest::header::COOKIE, &cookie)
                .form(&[
                    ("csrf_token", csrf.as_str()),
                    ("schedule", id.as_str()),
                    ("action", action),
                ])
                .send()
                .await
                .expect("the form should be accepted")
        }
    };
    let id = schedule.schedule_id.to_string();

    assert!(act("pause", id.clone()).await.status().is_redirection());
    let row = schedule_row(script_uri, "morning-digest")
        .await
        .expect("row");
    assert_eq!(row.get::<String, _>("state"), "paused");
    assert_eq!(
        row.get::<Option<String>, _>("paused_reason").as_deref(),
        Some("person")
    );

    // Paused by the person, it does not fire.
    make_due(script_uri, Duration::seconds(1)).await;
    assert_eq!(schedules::fire_due(Utc::now()).await, 0);

    act("resume", id.clone()).await;
    let row = schedule_row(script_uri, "morning-digest")
        .await
        .expect("row");
    assert_eq!(row.get::<String, _>("state"), "active");

    // Somebody else's schedule is not addressable from here.
    let other_user = a_user("other").await;
    let other_script = "test://schedules/account-page-other";
    repository::upsert_script(other_script, "function work() {}").expect("script should store");
    let theirs = declare(&other_user, other_script, hourly("theirs", "work"))
        .await
        .expect("declared");
    act("delete", theirs.schedule_id.to_string()).await;
    assert_eq!(schedule_count(other_script).await, 1);

    act("delete", id).await;
    assert_eq!(schedule_count(script_uri).await, 0);

    engine.shutdown().await;
}
