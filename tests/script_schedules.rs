//! A script's own schedules: declared in `init()` with `scriptTasks.schedule`,
//! fired into the task queue in script context, and replaced by the next
//! `init()` — without touching the schedules people declared on the script.

mod common;

use aiwebengine::delegation::{self, Scope};
use aiwebengine::schedules::{self, ScheduleSpec};
use aiwebengine::{repository, script_init::ScriptInitializer};
use chrono::{DateTime, Duration, Utc};
use common::setup_env;
use serde_json::json;
use sqlx::Row;

async fn init(script_uri: &str) {
    ScriptInitializer::with_configured_timeout()
        .initialize_script(script_uri, false)
        .await
        .expect("init should report a result");
}

/// `(key, user_id, next_run_at)` for every schedule on the script.
async fn rows(script_uri: &str) -> Vec<(String, Option<String>, DateTime<Utc>)> {
    sqlx::query(
        "SELECT schedule_key, user_id, next_run_at FROM schedules \
         WHERE script_uri = $1 ORDER BY schedule_key",
    )
    .bind(script_uri)
    .fetch_all(&common::test_pool().await)
    .await
    .expect("schedules should read")
    .iter()
    .map(|row| {
        (
            row.get("schedule_key"),
            row.get("user_id"),
            row.get("next_run_at"),
        )
    })
    .collect()
}

async fn make_due(script_uri: &str) {
    sqlx::query("UPDATE schedules SET next_run_at = $2 WHERE script_uri = $1")
        .bind(script_uri)
        .bind(Utc::now() - Duration::seconds(1))
        .execute(&common::test_pool().await)
        .await
        .expect("made due");
}

fn source(declarations: &str) -> String {
    format!(
        "function tick(context) {{}}\nfunction init() {{\n{}\n}}\n",
        declarations
    )
}

const NIGHTLY: &str = r#"scriptTasks.schedule({ key: "nightly", handler: "tick", cron: "0 3 * * *", timeZone: "Europe/Helsinki" });"#;
const HOURLY: &str =
    r#"scriptTasks.schedule({ key: "hourly", handler: "tick", intervalMinutes: 60 });"#;

/// A due script schedule becomes a task in script context: no person, one
/// attempt, and the schedule it came from on the row.
#[tokio::test(flavor = "multi_thread")]
async fn a_script_schedule_fires_a_script_context_task() {
    setup_env().await;
    let script_uri = "test://script-schedules/fires";
    repository::upsert_script(script_uri, &source(HOURLY)).expect("script should store");
    init(script_uri).await;
    assert_eq!(rows(script_uri).await.len(), 1);

    make_due(script_uri).await;
    schedules::fire_due(Utc::now()).await;

    let fired = sqlx::query(
        "SELECT run_as, max_attempts, schedule_id FROM script_tasks WHERE script_uri = $1",
    )
    .bind(script_uri)
    .fetch_all(&common::test_pool().await)
    .await
    .expect("tasks should read");
    assert_eq!(fired.len(), 1, "one slot, one task");
    assert_eq!(fired[0].get::<Option<String>, _>("run_as"), None);
    assert_eq!(fired[0].get::<i32, _>("max_attempts"), 1);
    assert!(
        fired[0]
            .get::<Option<uuid::Uuid>, _>("schedule_id")
            .is_some()
    );
}

/// The next `init()` keeps what it declares again — at the same slot — and
/// removes what it no longer declares. A person's schedule on the same script
/// is not the code's to remove.
#[tokio::test(flavor = "multi_thread")]
async fn init_replaces_the_scripts_schedules_and_leaves_personal_ones() {
    setup_env().await;
    let script_uri = "test://script-schedules/redeploy";
    repository::upsert_script(script_uri, &source(&format!("{}\n{}", NIGHTLY, HOURLY)))
        .expect("script should store");
    init(script_uri).await;

    let user_id = aiwebengine::user_repository::upsert_internal_user(
        Some("script schedule test".to_string()),
        "test".to_string(),
        format!("redeploy-{}", uuid::Uuid::new_v4()),
        "localhost".to_string(),
    )
    .await
    .expect("the account should be created");
    delegation::grant(&user_id, script_uri, &Scope::all(), Duration::days(30))
        .await
        .expect("granted");
    let limits = schedules::limits();
    let spec = ScheduleSpec::from_json(
        &json!({ "key": "hourly", "handler": "tick", "intervalMinutes": 60 }),
        limits.min_interval_ms,
    )
    .expect("a valid spec");
    let pool = common::test_pool().await;
    let mut conn = pool.acquire().await.expect("a connection");
    schedules::declare_on(&mut conn, &user_id, script_uri, &spec, None, &limits)
        .await
        .expect("the person's schedule should store");

    let before = rows(script_uri).await;
    assert_eq!(before.len(), 3, "two of the script's, one of the person's");
    let nightly_before = before
        .iter()
        .find(|(key, user, _)| key == "nightly" && user.is_none())
        .map(|(_, _, at)| *at)
        .expect("nightly declared");

    // The next version declares only the nightly one.
    repository::upsert_script(script_uri, &source(NIGHTLY)).expect("v2 should store");
    init(script_uri).await;

    let after = rows(script_uri).await;
    let script_keys: Vec<&str> = after
        .iter()
        .filter(|(_, user, _)| user.is_none())
        .map(|(key, _, _)| key.as_str())
        .collect();
    assert_eq!(script_keys, vec!["nightly"]);
    assert_eq!(
        after
            .iter()
            .find(|(key, user, _)| key == "nightly" && user.is_none())
            .map(|(_, _, at)| *at),
        Some(nightly_before),
        "declared again with the same cadence, it keeps its slot"
    );
    assert!(
        after
            .iter()
            .any(|(key, user, _)| key == "hourly" && user.as_deref() == Some(user_id.as_str())),
        "the person's schedule under the same key is not the code's to remove"
    );
}

/// Outside `init()` a declaration is refused as a value and stores nothing;
/// a check of the script reports it without storing it either.
#[tokio::test(flavor = "multi_thread")]
async fn only_init_declares_and_a_check_stores_nothing() {
    setup_env().await;
    let script_uri = "test://script-schedules/phase";
    let declare_late = format!(
        "function declareLate(context) {{ const r = {} if (r.ok !== false) throw new Error('declared'); }}",
        HOURLY
    );
    repository::upsert_script(script_uri, &format!("{}\n{}", source(""), declare_late))
        .expect("script should store");

    // A task handler runs outside the registration phase, as a request does.
    let invocation = aiwebengine::tasks::TaskInvocation {
        task_id: uuid::Uuid::new_v4(),
        invocation_id: "phase-test".to_string(),
        script_uri: script_uri.to_string(),
        handler_name: "declareLate".to_string(),
        payload: json!({}),
        attempts: 0,
        max_attempts: 1,
        run_as: None,
        claim_epoch: 0,
        schedule: None,
    };
    tokio::task::spawn_blocking(move || {
        aiwebengine::js_engine::execute_task_handler(&invocation, None, None)
    })
    .await
    .expect("no panic")
    .expect("a refusal is a value, not a thrown error");
    assert!(rows(script_uri).await.is_empty());

    let report = aiwebengine::script_check::check_blocking(
        aiwebengine::script_check::CheckRequest {
            script_uri: script_uri.to_string(),
            content: None,
            rollback: true,
            timeout_ms: None,
            view: Default::default(),
        },
        10_000,
    );
    assert!(report.ok, "{:?}", report.diagnostics);
    assert!(
        rows(script_uri).await.is_empty(),
        "a check declares nothing"
    );
}

/// A script's schedule whose runs keep failing is counted, never paused:
/// nobody but its code could resume it.
#[tokio::test(flavor = "multi_thread")]
async fn a_failing_script_schedule_is_not_paused() {
    setup_env().await;
    let script_uri = "test://script-schedules/failing";
    repository::upsert_script(script_uri, &source(HOURLY)).expect("script should store");
    init(script_uri).await;

    let pool = common::test_pool().await;
    let schedule_id: uuid::Uuid =
        sqlx::query_scalar("SELECT schedule_id FROM schedules WHERE script_uri = $1")
            .bind(script_uri)
            .fetch_one(&pool)
            .await
            .expect("declared");
    let mut conn = pool.acquire().await.expect("a connection");
    for _ in 0..schedules::FAILURES_BEFORE_PAUSE + 1 {
        schedules::record_outcome_on(&mut conn, schedule_id, false)
            .await
            .expect("recorded");
    }

    let (state, failures): (String, i32) =
        sqlx::query_as("SELECT state, consecutive_failures FROM schedules WHERE schedule_id = $1")
            .bind(schedule_id)
            .fetch_one(&pool)
            .await
            .expect("still there");
    assert_eq!(state, "active");
    assert_eq!(failures, schedules::FAILURES_BEFORE_PAUSE + 1);
}
