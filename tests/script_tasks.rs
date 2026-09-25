//! The durable queue: a script enqueues work, a worker runs it afterwards.
//!
//! What these cover is the behaviour that distinguishes a queue from the
//! scheduler beside it — enqueueing outside `init()`, surviving a
//! re-initialisation, carrying a payload, and retrying then giving up.

mod common;

use aiwebengine::tasks::{self, NewTask};
use aiwebengine::{js_engine, repository};
use common::setup_env;
use serde_json::json;

fn new_task(script_uri: &str, handler: &str) -> NewTask {
    NewTask {
        script_uri: script_uri.to_string(),
        handler_name: handler.to_string(),
        payload: json!({ "value": 41 }),
        run_at: None,
        max_attempts: None,
        enqueued_by: None,
        run_as: None,
        lane: None,
    }
}

/// The whole point of the payload: the handler is told what the work is about,
/// and writes something a later assertion can see.
#[tokio::test(flavor = "multi_thread")]
async fn a_task_runs_its_handler_with_the_payload_it_carried() {
    setup_env().await;

    let script_uri = "test://tasks/runs";
    repository::upsert_script(
        script_uri,
        r#"
        function handleWork(context) {
          console.log("handled value " + (context.meta.task.payload.value + 1));
        }
        "#,
    )
    .expect("script should store");
    repository::clear_log_messages(script_uri).expect("logs should clear");

    let task = tasks::enqueue(new_task(script_uri, "handleWork"))
        .await
        .expect("the task should be accepted");

    let invocation = tasks::TaskInvocation {
        task_id: task.task_id,
        invocation_id: "test-invocation".to_string(),
        script_uri: script_uri.to_string(),
        handler_name: "handleWork".to_string(),
        payload: task.payload.clone(),
        attempts: 0,
        max_attempts: task.max_attempts,
        run_as: None,
    };

    tokio::task::spawn_blocking(move || js_engine::execute_task_handler(&invocation, None))
        .await
        .expect("no panic")
        .expect("the handler should run");

    let logs = repository::fetch_log_messages(script_uri);
    assert!(
        logs.iter()
            .any(|entry| entry.message.contains("handled value 42")),
        "the handler should have seen its payload: {:?}",
        logs
    );
}

/// The distinction from `scheduler_jobs`, which `script_init` wipes before
/// every re-initialisation because a schedule belongs to the version of the
/// code that declared it. Work already accepted does not.
#[tokio::test(flavor = "multi_thread")]
async fn a_task_survives_the_script_being_deployed_again() {
    setup_env().await;

    let script_uri = "test://tasks/survives-init";
    repository::upsert_script(script_uri, "function handleWork() {}").expect("script should store");

    let task = tasks::enqueue(new_task(script_uri, "handleWork"))
        .await
        .expect("the task should be accepted");

    // A second write is a re-initialisation: it clears the script's scheduled
    // jobs and its listeners.
    repository::upsert_script(script_uri, "function handleWork() { /* v2 */ }")
        .expect("the second version should store");
    aiwebengine::script_init::ScriptInitializer::with_configured_timeout()
        .initialize_script(script_uri, false)
        .await
        .ok();

    let found = tasks::get(task.task_id)
        .await
        .expect("the lookup should succeed");
    assert!(
        found.is_some(),
        "a queued task should outlive a new version of the script that queued it"
    );
}

/// Deleting the script is the case where queued work *should* go: there is no
/// version left to run it, and every attempt would fail before giving up.
#[tokio::test(flavor = "multi_thread")]
async fn deleting_a_script_takes_its_queued_work_with_it() {
    setup_env().await;

    let script_uri = "test://tasks/deleted";
    repository::upsert_script(script_uri, "function handleWork() {}").expect("script should store");

    let task = tasks::enqueue(new_task(script_uri, "handleWork"))
        .await
        .expect("the task should be accepted");

    tokio::task::spawn_blocking(move || repository::delete_script(script_uri))
        .await
        .expect("no panic");

    let found = tasks::get(task.task_id)
        .await
        .expect("the lookup should succeed");
    assert!(
        found.is_none(),
        "a deleted script should not leave work nothing can run"
    );
}

/// A pending task can be stopped; the answer distinguishes "cancelled" from
/// "there was nothing to cancel".
#[tokio::test(flavor = "multi_thread")]
async fn cancelling_stops_a_pending_task_once() {
    setup_env().await;

    let script_uri = "test://tasks/cancelled";
    repository::upsert_script(script_uri, "function handleWork() {}").expect("script should store");

    let task = tasks::enqueue(new_task(script_uri, "handleWork"))
        .await
        .expect("the task should be accepted");

    assert!(
        tasks::cancel(task.task_id)
            .await
            .expect("cancel should run"),
        "a pending task should cancel"
    );
    assert!(
        !tasks::cancel(task.task_id)
            .await
            .expect("cancel should run"),
        "cancelling twice should report that there was nothing to cancel"
    );

    let found = tasks::get(task.task_id)
        .await
        .expect("the lookup should succeed")
        .expect("a cancelled task keeps its row");
    assert_eq!(found.state, "cancelled");
}

/// A failed task is the one somebody has to read, so it keeps its row and the
/// error that ended it. Driven through the worker's own claim and finalize
/// rather than around them, since the `locked_by` guard is the part worth
/// exercising.
#[tokio::test(flavor = "multi_thread")]
async fn a_task_whose_handler_throws_is_retried_and_then_given_up_on() {
    setup_env().await;

    let script_uri = "test://tasks/always-fails";
    repository::upsert_script(
        script_uri,
        r#"
        function handleWork() {
          throw new Error("nope");
        }
        "#,
    )
    .expect("script should store");

    let mut task = new_task(script_uri, "handleWork");
    task.max_attempts = Some(2);
    let task = tasks::enqueue(task)
        .await
        .expect("the task should be accepted");

    // Two attempts, each claimed the way the worker claims: the second is only
    // reachable because the first requeued the row. The backoff is real, so the
    // retry is pulled forward rather than waited out — that it was scheduled
    // into the future at all is asserted below.
    for attempt in 1..=2 {
        if attempt > 1 {
            sqlx::query("UPDATE script_tasks SET run_at = NOW() WHERE task_id = $1")
                .bind(task.task_id)
                .execute(&common::test_pool().await)
                .await
                .expect("the retry should be brought forward");
        }

        tasks::run_due_now("test-worker").await;

        let found = tasks::get(task.task_id)
            .await
            .expect("the lookup should succeed")
            .expect("a failing task keeps its row");

        assert_eq!(found.attempts, attempt, "the attempt should be counted");
        assert!(
            found.last_error.is_some_and(|e| e.contains("nope")),
            "the failure should say what went wrong"
        );

        if attempt < 2 {
            assert_eq!(found.state, "pending", "there is an attempt left");
            // Against `updated_at` rather than the wall clock. Both are
            // written by the one statement that requeues the row, from the
            // database's clock, so this compares the backoff with the moment
            // it was applied and says exactly what it means.
            //
            // `Utc::now()` here was a race: the read that follows the write
            // goes through the connection pool, and under a loaded suite
            // acquiring a connection can take longer than the five-second
            // backoff — at which point a correctly scheduled retry reads as a
            // spin. Widening the margin would only have made the race rarer.
            assert!(
                found.run_at > found.updated_at,
                "a retry should wait before running again, not spin: \
                 run_at {} is not after updated_at {}",
                found.run_at,
                found.updated_at
            );
        } else {
            assert_eq!(found.state, "failed", "the attempts are spent");
        }
    }
}

/// A task that succeeds keeps no row, so the table does not grow for the one
/// outcome nobody looks up. What it did is in the script's log.
#[tokio::test(flavor = "multi_thread")]
async fn a_task_that_succeeds_leaves_no_row_behind() {
    setup_env().await;

    let script_uri = "test://tasks/succeeds";
    repository::upsert_script(script_uri, "function handleWork() { }")
        .expect("script should store");

    let task = tasks::enqueue(new_task(script_uri, "handleWork"))
        .await
        .expect("the task should be accepted");

    tasks::run_due_now("test-worker").await;

    assert!(
        tasks::get(task.task_id)
            .await
            .expect("the lookup should succeed")
            .is_none(),
        "a completed task should not keep a row"
    );
}

/// Discarding is for the rows that are finished; it must not take work that is
/// still waiting to run.
#[tokio::test(flavor = "multi_thread")]
async fn discarding_finished_tasks_leaves_the_pending_ones() {
    setup_env().await;

    let script_uri = "test://tasks/discard";
    repository::upsert_script(script_uri, "function handleWork() {}").expect("script should store");

    let cancelled = tasks::enqueue(new_task(script_uri, "handleWork"))
        .await
        .expect("accepted");
    tasks::cancel(cancelled.task_id).await.expect("cancel runs");

    let pending = tasks::enqueue(new_task(script_uri, "handleWork"))
        .await
        .expect("accepted");

    let discarded = tasks::clear_finished(script_uri)
        .await
        .expect("discard should run");
    assert!(discarded >= 1, "the cancelled task should have gone");

    assert!(
        tasks::get(cancelled.task_id)
            .await
            .expect("lookup")
            .is_none(),
        "a cancelled task should be discarded"
    );
    assert!(
        tasks::get(pending.task_id).await.expect("lookup").is_some(),
        "work still waiting should survive a discard"
    );
}

/// The shape the queue exists for, end to end: a request handler enqueues,
/// answers, and the work runs afterwards.
///
/// Enqueueing from a handler is the part `schedulerService` cannot do —
/// `registerOnce` outside `init()` returns a string saying nothing was
/// registered — so this is the test that says the gap is closed.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_handler_can_enqueue_work_that_runs_after_it_answers() {
    setup_env().await;

    let script_uri = "test://tasks/from-a-handler";
    repository::upsert_script(
        script_uri,
        r#"
        function startWork(context) {
          const task = scriptTasks.enqueue({
            handler: "finishWork",
            payload: { note: "from the handler" },
          });
          return { status: 202, body: task.taskId, contentType: "text/plain" };
        }

        function finishWork(context) {
          console.log("finished: " + context.meta.task.payload.note);
        }
        "#,
    )
    .expect("script should store");
    repository::clear_log_messages(script_uri).expect("logs should clear");

    let (status, body, _) = tokio::task::spawn_blocking(move || {
        js_engine::execute_script_for_request(
            script_uri,
            "startWork",
            "/start",
            "GET",
            Default::default(),
            Default::default(),
            None,
        )
    })
    .await
    .expect("no panic")
    .expect("the handler should answer");

    assert_eq!(status, 202, "the handler answers before the work is done");
    let task_id: uuid::Uuid = body.trim().parse().expect("the handler returns a task id");

    // Nothing has run it yet: the request is over and the row is waiting.
    let queued = tasks::get(task_id)
        .await
        .expect("lookup")
        .expect("the task should be queued");
    assert_eq!(queued.state, "pending");
    assert_eq!(queued.script_uri, script_uri);

    tasks::run_due_now("test-worker").await;

    let logs = repository::fetch_log_messages(script_uri);
    assert!(
        logs.iter()
            .any(|entry| entry.message.contains("finished: from the handler")),
        "the work should have run after the request: {:?}",
        logs
    );
    assert!(
        tasks::get(task_id).await.expect("lookup").is_none(),
        "a completed task keeps no row"
    );
}
