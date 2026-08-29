//! Who holds a lease, and when it lapses.
//!
//! `acquireLease` is one statement that has to decide four things at once: an
//! empty slot is taken, an expired one is taken, the holder extends its own,
//! and everyone else is turned away. Until now none of that was tested, and the
//! expiry was computed by the database — `NOW() + interval`, which only
//! Postgres spells that way. The clock is the engine's now, which is what these
//! tests pin: not the SQL, but what a script sees when it asks for a lease.

mod common;

use aiwebengine::repository;
use common::{setup_env, should_skip_integration_tests, test_mutex};
use serde_json::Value;

/// A lease table belonging to `script_uri`, empty whatever an earlier run left.
async fn fresh_lease_table(script_uri: &str) {
    repository::upsert_script(script_uri, "function init() {}").expect("script should be stored");

    let uri = script_uri.to_string();
    tokio::task::spawn_blocking(move || {
        // Ignored: there is nothing to drop the first time this runs.
        let _ = repository::drop_script_table(&uri, "leases");
        repository::create_lease_table(&uri, "leases").expect("lease table should be created");
    })
    .await
    .expect("lease table setup panicked");
}

/// Ask for the lease, off the async runtime — the repository call blocks.
async fn acquire(script_uri: &str, owner: &str, ttl_ms: i64) -> Value {
    let (uri, owner) = (script_uri.to_string(), owner.to_string());
    tokio::task::spawn_blocking(move || {
        repository::acquire_lease(&uri, "leases", "the_slot", &owner, ttl_ms)
    })
    .await
    .expect("acquireLease panicked")
    .expect("acquireLease should have answered")
}

fn acquired(answer: &Value) -> bool {
    answer
        .get("acquired")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| panic!("an answer should say whether it acquired: {}", answer))
}

fn expires_at(answer: &Value) -> chrono::DateTime<chrono::Utc> {
    let raw = answer
        .get("expires_at")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("a held lease should report when it expires: {}", answer));
    chrono::DateTime::parse_from_rfc3339(raw)
        .unwrap_or_else(|_| panic!("expires_at should be ISO 8601, got {}", raw))
        .with_timezone(&chrono::Utc)
}

#[tokio::test(flavor = "multi_thread")]
async fn an_empty_slot_is_taken_by_whoever_asks_first() {
    if should_skip_integration_tests() {
        return;
    }
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script = "test://leases/empty-slot";
    fresh_lease_table(script).await;

    let before = chrono::Utc::now();
    let answer = acquire(script, "instance-a", 5_000).await;

    assert!(acquired(&answer), "an empty slot should be taken: {answer}");
    assert_eq!(answer.get("owner"), Some(&Value::from("instance-a")));

    // The expiry is the moment of the call plus the TTL, not the moment the
    // database happened to run the statement.
    let expiry = expires_at(&answer);
    assert!(
        expiry >= before + chrono::TimeDelta::milliseconds(5_000)
            && expiry <= chrono::Utc::now() + chrono::TimeDelta::milliseconds(5_000),
        "a 5s lease should expire about 5s out, got {expiry} for a call at {before}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_live_lease_turns_away_everyone_but_its_holder() {
    if should_skip_integration_tests() {
        return;
    }
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script = "test://leases/contended";
    fresh_lease_table(script).await;

    let held = acquire(script, "instance-a", 30_000).await;
    assert!(acquired(&held));

    let refused = acquire(script, "instance-b", 30_000).await;
    assert!(
        !acquired(&refused),
        "a live lease held by someone else must not be taken: {refused}"
    );

    // Being turned away has to say who holds it and until when, or a caller
    // has no way to know how long to back off for.
    assert_eq!(
        refused.get("owner"),
        Some(&Value::from("instance-a")),
        "a refusal should name the current holder: {refused}"
    );
    assert_eq!(
        expires_at(&refused),
        expires_at(&held),
        "a refusal should report the holder's expiry, unchanged"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_holder_extends_its_own_lease_rather_than_being_refused() {
    if should_skip_integration_tests() {
        return;
    }
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script = "test://leases/extend";
    fresh_lease_table(script).await;

    let first = acquire(script, "instance-a", 2_000).await;
    assert!(acquired(&first));

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let second = acquire(script, "instance-a", 2_000).await;
    assert!(
        acquired(&second),
        "the holder renewing before expiry should keep the lease: {second}"
    );
    assert!(
        expires_at(&second) > expires_at(&first),
        "renewing should push the expiry out, not leave it where it was"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lapsed_lease_is_taken_by_the_next_caller() {
    if should_skip_integration_tests() {
        return;
    }
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script = "test://leases/lapsed";
    fresh_lease_table(script).await;

    // The whole point of a TTL: an instance that stops renewing — because it
    // died, not because it released anything — must not hold the slot forever.
    let held = acquire(script, "instance-a", 100).await;
    assert!(acquired(&held));

    let too_soon = acquire(script, "instance-b", 5_000).await;
    assert!(
        !acquired(&too_soon),
        "the lease has not lapsed yet: {too_soon}"
    );

    tokio::time::sleep(std::time::Duration::from_millis(250)).await;

    let taken = acquire(script, "instance-b", 5_000).await;
    assert!(
        acquired(&taken),
        "a lapsed lease should be taken by the next caller: {taken}"
    );
    assert_eq!(taken.get("owner"), Some(&Value::from("instance-b")));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_ttl_that_names_no_future_is_refused() {
    if should_skip_integration_tests() {
        return;
    }
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script = "test://leases/bad-ttl";
    fresh_lease_table(script).await;

    for ttl in [0i64, -1] {
        let uri = script.to_string();
        let result = tokio::task::spawn_blocking(move || {
            repository::acquire_lease(&uri, "leases", "the_slot", "instance-a", ttl)
        })
        .await
        .expect("acquireLease panicked");

        assert!(
            result.is_err(),
            "a TTL of {ttl} grants a lease that has already expired, so it is refused"
        );
    }

    // Far enough out to overflow the instant it would expire at. Previously
    // this reached the database as interval arithmetic and failed there.
    let uri = script.to_string();
    let result = tokio::task::spawn_blocking(move || {
        repository::acquire_lease(&uri, "leases", "the_slot", "instance-a", i64::MAX)
    })
    .await
    .expect("acquireLease panicked");

    assert!(
        result.is_err(),
        "a TTL that cannot be added to the current instant is refused, not wrapped"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn two_slots_in_one_table_are_held_independently() {
    if should_skip_integration_tests() {
        return;
    }
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let script = "test://leases/two-slots";
    fresh_lease_table(script).await;

    // `lease_id` is the slot. One table serves as many leases as a script has
    // things to serialise, so holding one must say nothing about the others.
    let uri = script.to_string();
    let answers = tokio::task::spawn_blocking(move || {
        let first = repository::acquire_lease(&uri, "leases", "world_1", "instance-a", 30_000)
            .expect("first slot should answer");
        let second = repository::acquire_lease(&uri, "leases", "world_2", "instance-b", 30_000)
            .expect("second slot should answer");
        let contended = repository::acquire_lease(&uri, "leases", "world_1", "instance-b", 30_000)
            .expect("contended slot should answer");
        (first, second, contended)
    })
    .await
    .expect("acquireLease panicked");

    assert!(acquired(&answers.0), "world_1 should be taken by a");
    assert!(acquired(&answers.1), "world_2 should be taken by b");
    assert!(
        !acquired(&answers.2),
        "b holding world_2 does not entitle it to world_1: {}",
        answers.2
    );
}
