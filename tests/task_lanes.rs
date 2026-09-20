//! What must not run beside what.
//!
//! Claiming is `FOR UPDATE SKIP LOCKED` across whatever is due, and the
//! worker spawns each claimed run rather than awaiting it — so two tasks
//! enqueued a moment apart run at the same time. For most work that is the
//! point. For work belonging to one person it is a correctness bug: two
//! prompts to an agent become two runs interleaving turn for turn over the
//! same storage.
//!
//! A lane says those two must not run together. The invariant is "at most
//! one running per `(script, lane)`", and it has to hold against three
//! different ways two tasks in one lane can be claimed at once — a lane
//! already busy, two due in one worker's batch, and two workers racing at
//! the batch boundary. Each needs its own piece of the claim statement, so
//! each gets its own test here: a lane key that occasionally lets two run is
//! worse than none, because scripts would go on writing the workaround it
//! exists to remove.

mod common;

use aiwebengine::tasks::{self, NewTask, TaskKind};
use chrono::Utc;
use common::setup_env;
use serde_json::json;

/// One worker's claim, as a count. `claim_due` is the statement under test;
/// what it hands back is exactly what it let through.
async fn claim(worker_id: &str) -> usize {
    tasks::claim_due(worker_id, Utc::now()).await.len()
}

/// A worker that died: nothing renews its lease, so it lapses. Written
/// directly rather than by waiting out the real TTL, which is measured in
/// minutes.
async fn expire_leases(script_uri: &str) {
    sqlx::query(
        "UPDATE script_tasks SET lock_expires_at = NOW() - INTERVAL '1 minute' \
         WHERE script_uri = $1 AND state = 'running'",
    )
    .bind(script_uri)
    .execute(&common::test_pool().await)
    .await
    .expect("the lease should expire");
}

fn task(script_uri: &str, handler: &str, lane: Option<&str>) -> NewTask {
    NewTask {
        script_uri: script_uri.to_string(),
        handler_name: handler.to_string(),
        payload: json!({}),
        run_at: None,
        max_attempts: Some(3),
        enqueued_by: None,
        kind: TaskKind::Task,
        run_as: None,
        lane: lane.map(str::to_string),
    }
}

/// How many of a script's tasks a worker took. Claiming marks them
/// `running`, so this is what the claim let through.
async fn claimed(script_uri: &str) -> usize {
    tasks::list(script_uri, 100)
        .await
        .expect("listing")
        .iter()
        .filter(|task| task.state == "running")
        .count()
}

/// The invariant, in the case the whole feature is named for: two tasks due
/// at once in one lane, one worker, one claim. One runs; the other waits.
#[tokio::test(flavor = "multi_thread")]
async fn two_tasks_in_one_lane_are_not_claimed_together() {
    setup_env().await;
    let script_uri = "test://lanes/one-batch";

    for _ in 0..2 {
        tasks::enqueue(task(script_uri, "work", Some("person:a")))
            .await
            .expect("accepted");
    }

    assert_eq!(
        claim("worker-one").await,
        1,
        "a second task in the same lane must wait for the first"
    );
    assert_eq!(claimed(script_uri).await, 1);
}

/// And tasks in different lanes are not held up by each other. A lane that
/// serialised more than it was asked to would be worse than none.
#[tokio::test(flavor = "multi_thread")]
async fn tasks_in_different_lanes_run_together() {
    setup_env().await;
    let script_uri = "test://lanes/separate";

    for lane in ["person:a", "person:b", "person:c"] {
        tasks::enqueue(task(script_uri, "work", Some(lane)))
            .await
            .expect("accepted");
    }

    assert_eq!(
        claim("worker-one").await,
        3,
        "three lanes should claim three tasks"
    );
}

/// No lane is no constraint, which is every task that existed before lanes
/// did. The default must not have changed under anybody.
#[tokio::test(flavor = "multi_thread")]
async fn tasks_without_a_lane_are_unconstrained() {
    setup_env().await;
    let script_uri = "test://lanes/none";

    for _ in 0..3 {
        tasks::enqueue(task(script_uri, "work", None))
            .await
            .expect("accepted");
    }

    assert_eq!(
        claim("worker-one").await,
        3,
        "unlaned tasks are claimed alongside each other, as they always were"
    );
}

/// The second way two can be claimed at once: a lane already holds a live
/// run, and a second worker comes along afterwards. The `NOT EXISTS` is what
/// answers this one.
#[tokio::test(flavor = "multi_thread")]
async fn a_busy_lane_is_not_claimed_from_again() {
    setup_env().await;
    let script_uri = "test://lanes/busy";

    tasks::enqueue(task(script_uri, "work", Some("person:a")))
        .await
        .expect("accepted");
    assert_eq!(claim("worker-one").await, 1);

    // Queued after the first was already claimed and running.
    tasks::enqueue(task(script_uri, "work", Some("person:a")))
        .await
        .expect("accepted");

    assert_eq!(
        claim("worker-two").await,
        0,
        "a lane with a live run must hand out nothing"
    );
    assert_eq!(claimed(script_uri).await, 1);
}

/// Two workers claiming at the same moment. `FOR UPDATE` locks every
/// candidate a statement selected, so most of this is already impossible.
///
/// This asserts the invariant rather than the mechanism, and it is worth
/// being plain about what it does not reach: the window it is aiming at is
/// sub-millisecond, and two spawned claims against a local database do not
/// reliably land inside it. What it does catch is a claim that stopped
/// holding lanes at all. The narrow window has its own test below, which
/// forces the overlap instead of hoping for it.
#[tokio::test(flavor = "multi_thread")]
async fn two_workers_claiming_at_once_still_hold_the_lane() {
    setup_env().await;
    let script_uri = "test://lanes/racing";

    // More than one claim batch, deliberately. With fewer, `FOR UPDATE`
    // locks the whole candidate set and the second worker's `SKIP LOCKED`
    // passes over all of it — the race cannot happen and the test would
    // prove nothing. The gap this is aiming at is the batch boundary: a
    // lane's later tasks fall outside the first worker's selection, so they
    // are not locked, and the second worker evaluates the lane against a
    // snapshot in which the first claim has not committed.
    for _ in 0..40 {
        tasks::enqueue(task(script_uri, "work", Some("person:a")))
            .await
            .expect("accepted");
    }

    // Spawned rather than joined: `join!` polls two futures from one task, so
    // they take turns at each await and the statements would not overlap.
    // These two land on the runtime's threads and race for real.
    let one = tokio::spawn(claim("worker-one"));
    let two = tokio::spawn(claim("worker-two"));
    let (one, two) = (one.await.expect("no panic"), two.await.expect("no panic"));

    assert_eq!(
        one + two,
        1,
        "however two workers race, one lane hands out one task"
    );
    assert_eq!(claimed(script_uri).await, 1);
}

/// The batch boundary, forced rather than raced for.
///
/// A lane's later tasks fall outside the first worker's `LIMIT`, so they are
/// not row-locked, and the second worker evaluates the lane against a
/// snapshot in which the first claim has not committed. `FOR UPDATE` cannot
/// help there and the `NOT EXISTS` sees nothing, so the advisory lock is the
/// only thing holding the invariant — and a guard nothing exercises is a
/// guard nobody will notice breaking.
///
/// So this holds the lane's lock open from outside and asks the real claim
/// what it does. That makes it white-box about how the key is derived, which
/// is the point as much as a cost: the two halves of `hashtext(script),
/// hashtext(lane)` have to agree, and a change to one of them would
/// otherwise turn the guard off in silence.
#[tokio::test(flavor = "multi_thread")]
async fn a_lane_another_worker_is_claiming_from_is_left_alone() {
    setup_env().await;
    let script_uri = "test://lanes/boundary";

    tasks::enqueue(task(script_uri, "work", Some("person:a")))
        .await
        .expect("accepted");
    tasks::enqueue(task(script_uri, "work", Some("person:b")))
        .await
        .expect("accepted");

    let pool = common::test_pool().await;
    let mut holding = pool.begin().await.expect("a transaction");

    // Exactly what a worker mid-claim holds for the lane it is claiming
    // from. Nothing is running yet — that is the whole difficulty — so the
    // lock is the only signal there is.
    let taken: bool =
        sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtext($1), hashtext($2))")
            .bind(script_uri)
            .bind("person:a")
            .fetch_one(&mut *holding)
            .await
            .expect("the lock should be available");
    assert!(taken, "nothing else should be holding this lane");

    // The other lane is untouched by it, so this is one task and not none.
    assert_eq!(
        claim("worker-two").await,
        1,
        "a lane being claimed from elsewhere is skipped; its neighbour is not"
    );

    holding.rollback().await.expect("release");

    assert_eq!(
        claim("worker-three").await,
        1,
        "and it is claimable again once the other worker is done"
    );
}

/// A lane is per script, like a job key. Two solutions both using the lane
/// "inbox" are not talking about the same thing, and serialising them
/// against each other would make lane names a global namespace nobody
/// agreed to share.
#[tokio::test(flavor = "multi_thread")]
async fn a_lane_does_not_reach_across_scripts() {
    setup_env().await;
    let first = "test://lanes/script-one";
    let second = "test://lanes/script-two";

    tasks::enqueue(task(first, "work", Some("inbox")))
        .await
        .expect("accepted");
    tasks::enqueue(task(second, "work", Some("inbox")))
        .await
        .expect("accepted");

    assert_eq!(claim("worker-one").await, 2);
    assert_eq!(claimed(first).await, 1);
    assert_eq!(claimed(second).await, 1);
}

/// A lane held by a worker that died unblocks when its lease lapses. Without
/// that, one crash would shut a person's agent for good — so "busy" is a
/// *live* run rather than the `running` state alone.
#[tokio::test(flavor = "multi_thread")]
async fn a_lane_held_by_a_dead_worker_comes_back() {
    setup_env().await;
    let script_uri = "test://lanes/lapsed";

    tasks::enqueue(task(script_uri, "work", Some("person:a")))
        .await
        .expect("accepted");
    tasks::enqueue(task(script_uri, "work", Some("person:a")))
        .await
        .expect("accepted");

    assert_eq!(claim("worker-one").await, 1);
    assert_eq!(
        claim("worker-two").await,
        0,
        "the lane is busy while the lease is live"
    );

    // The worker dies: nothing renews the lease, and it lapses.
    expire_leases(script_uri).await;

    assert_eq!(
        claim("worker-two").await,
        1,
        "a lapsed lease must not hold its lane shut"
    );
}

/// The lane is on the row, so a person looking at the queue can see why
/// something is waiting rather than guessing.
#[tokio::test(flavor = "multi_thread")]
async fn a_lane_is_visible_on_the_task() {
    setup_env().await;
    let script_uri = "test://lanes/visible";

    let stored = tasks::enqueue(task(script_uri, "work", Some("  person:a  ")))
        .await
        .expect("accepted");

    assert_eq!(
        stored.lane.as_deref(),
        Some("person:a"),
        "a lane is trimmed on the way in"
    );
    assert_eq!(
        tasks::to_json(&stored)["lane"],
        json!("person:a"),
        "and reported to whoever is reading the queue"
    );
}
