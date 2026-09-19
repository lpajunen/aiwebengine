//! What one script may spend, when that differs from what the engine allows.
//!
//! The point of these is the two ends: that an override actually bounds a real
//! execution, and that a script with none behaves exactly as it did before any
//! of this existed.

mod common;

use aiwebengine::script_limits::{self, Overrides};
use aiwebengine::{js_engine, repository};
use common::setup_env;

/// The property that makes the feature safe to ship: nothing changes for a
/// script nobody has overridden.
#[tokio::test(flavor = "multi_thread")]
async fn a_script_with_no_override_runs_on_the_engines_limits() {
    setup_env().await;

    let script_uri = "test://limits/untouched";
    let engine = js_engine::current_execution_limits();
    let applied = script_limits::for_script(script_uri);

    assert_eq!(applied.timeout_ms, engine.timeout_ms);
    assert_eq!(applied.max_memory_mb, engine.max_memory_mb);
    assert_eq!(applied.stack_size_bytes, engine.stack_size_bytes);
}

/// Lowering is the direction that gets used: containing a script that has
/// started holding execution slots, without restarting the engine or touching
/// anybody else.
#[tokio::test(flavor = "multi_thread")]
async fn a_lowered_timeout_actually_stops_a_slow_script() {
    setup_env().await;

    let script_uri = "test://limits/contained";
    repository::upsert_script(
        script_uri,
        r#"
        function spin() {
          const until = Date.now() + 60000;
          while (Date.now() < until) { /* deliberately runaway */ }
          return { status: 200, body: "finished" };
        }
        "#,
    )
    .expect("script should store");

    script_limits::set(
        script_uri,
        Overrides {
            timeout_ms: Some(300),
            ..Default::default()
        },
        Some("a test containing a runaway handler"),
        None,
    )
    .await
    .expect("the override should store");

    let started = std::time::Instant::now();
    let result = tokio::task::spawn_blocking(move || {
        js_engine::execute_script_for_request(
            script_uri,
            "spin",
            "/spin",
            "GET",
            Default::default(),
            Default::default(),
            None,
        )
    })
    .await
    .expect("no panic");

    let elapsed = started.elapsed();
    script_limits::clear(script_uri).await.expect("cleanup");

    assert!(
        result.is_err(),
        "a handler past its budget should be stopped, not allowed to finish"
    );
    assert!(
        elapsed < std::time::Duration::from_secs(10),
        "it should have been stopped at its own 300ms budget rather than the engine's, \
         but took {:?}",
        elapsed
    );
}

/// The override has to survive the round trip through the database and end up
/// in the cache the execution path reads.
#[tokio::test(flavor = "multi_thread")]
async fn setting_an_override_takes_effect_without_a_restart() {
    setup_env().await;

    let script_uri = "test://limits/live";
    let engine = js_engine::current_execution_limits();

    script_limits::set(
        script_uri,
        Overrides {
            timeout_ms: Some(engine.timeout_ms + 5_000),
            ..Default::default()
        },
        None,
        None,
    )
    .await
    .expect("stored");

    assert_eq!(
        script_limits::for_script(script_uri).timeout_ms,
        engine.timeout_ms + 5_000,
        "the execution path should already see it"
    );

    script_limits::clear(script_uri).await.expect("cleared");

    assert_eq!(
        script_limits::for_script(script_uri).timeout_ms,
        engine.timeout_ms,
        "clearing should put it straight back on the engine's own limits"
    );
}

/// An override is per script, which is the entire point: one script's ceiling
/// must not become everybody's.
#[tokio::test(flavor = "multi_thread")]
async fn an_override_reaches_only_the_script_it_names() {
    setup_env().await;

    let overridden = "test://limits/one";
    let neighbour = "test://limits/another";
    let engine = js_engine::current_execution_limits();

    script_limits::set(
        overridden,
        Overrides {
            timeout_ms: Some(1_234),
            ..Default::default()
        },
        None,
        None,
    )
    .await
    .expect("stored");

    assert_eq!(script_limits::for_script(overridden).timeout_ms, 1_234);
    assert_eq!(
        script_limits::for_script(neighbour).timeout_ms,
        engine.timeout_ms,
        "one script's budget must not become the engine's policy"
    );

    script_limits::clear(overridden).await.expect("cleared");
}

/// A job is not answering a request, so the two budgets stay distinct even
/// when both are overridden on the same script.
#[tokio::test(flavor = "multi_thread")]
async fn a_request_budget_and_a_job_budget_stay_separate() {
    setup_env().await;

    let script_uri = "test://limits/both";
    script_limits::set(
        script_uri,
        Overrides {
            timeout_ms: Some(2_000),
            job_timeout_ms: Some(90_000),
            ..Default::default()
        },
        None,
        None,
    )
    .await
    .expect("stored");

    assert_eq!(script_limits::for_script(script_uri).timeout_ms, 2_000);
    assert_eq!(
        script_limits::for_script_job(script_uri).timeout_ms,
        90_000,
        "background work should take the job budget, not the request one"
    );
    assert_eq!(script_limits::job_timeout_ms_for(script_uri), 90_000);

    script_limits::clear(script_uri).await.expect("cleared");
}

/// Setting a value the engine would not survive is clamped rather than taken
/// as written: the row takes effect without a restart, so a mistyped one would
/// hold a slot until somebody noticed.
#[tokio::test(flavor = "multi_thread")]
async fn a_stored_override_is_clamped_into_range() {
    setup_env().await;

    let script_uri = "test://limits/clamped";
    let stored = script_limits::set(
        script_uri,
        Overrides {
            timeout_ms: Some(u64::MAX),
            job_timeout_ms: Some(0),
            max_memory_bytes: Some(u64::MAX),
        },
        None,
        None,
    )
    .await
    .expect("stored");

    assert_eq!(
        stored.overrides.timeout_ms,
        Some(script_limits::MAX_TIMEOUT_MS)
    );
    assert_eq!(
        stored.overrides.job_timeout_ms,
        Some(script_limits::MIN_TIMEOUT_MS)
    );
    assert_eq!(
        stored.overrides.max_memory_bytes,
        Some(script_limits::MAX_MEMORY_BYTES)
    );

    script_limits::clear(script_uri).await.expect("cleared");
}

/// Setting replaces rather than merges, so the row always says the whole of
/// what is in force.
#[tokio::test(flavor = "multi_thread")]
async fn setting_again_replaces_rather_than_merging() {
    setup_env().await;

    let script_uri = "test://limits/replaced";
    script_limits::set(
        script_uri,
        Overrides {
            timeout_ms: Some(5_000),
            job_timeout_ms: Some(50_000),
            ..Default::default()
        },
        None,
        None,
    )
    .await
    .expect("stored");

    let second = script_limits::set(
        script_uri,
        Overrides {
            timeout_ms: Some(6_000),
            ..Default::default()
        },
        None,
        None,
    )
    .await
    .expect("stored again");

    assert_eq!(second.overrides.timeout_ms, Some(6_000));
    assert_eq!(
        second.overrides.job_timeout_ms, None,
        "a field left out should follow the engine, not keep an earlier override"
    );

    script_limits::clear(script_uri).await.expect("cleared");
}

/// The gate that matters. A script's limits are a claim on execution slots,
/// threads and memory shared with every other script, so ownership is the
/// wrong test: an owner who could raise their own ceiling would be back to one
/// script setting the policy for all of them, which is what this exists to
/// stop.
#[tokio::test(flavor = "multi_thread")]
async fn only_an_administrator_may_change_what_a_script_may_spend() {
    use aiwebengine::engine_api::execute_native_mcp_tool;
    use aiwebengine::security::UserContext;
    use serde_json::json;

    setup_env().await;

    let script_uri = "test://limits/authorization";
    repository::upsert_script(script_uri, "function handler() {}").expect("script should store");

    let args = json!({ "script": script_uri, "timeoutMs": 600000 });

    // Everyone below an administrator, including the tier that authors
    // solutions and the one that owns this very script.
    for refused in [
        UserContext::anonymous(),
        UserContext::authenticated("someone".to_string()),
        UserContext::editor("an-author".to_string()),
    ] {
        let answer = tokio::task::spawn_blocking({
            let args = args.clone();
            move || execute_native_mcp_tool("set_script_limits", &args, &refused)
        })
        .await
        .expect("no panic")
        .expect("the tool exists");

        assert!(
            answer.get("error").is_some(),
            "a non-administrator must not set a script's limits, got: {}",
            answer
        );
    }

    // And nothing was stored by any of those attempts.
    assert!(
        script_limits::get(script_uri)
            .await
            .expect("lookup")
            .is_none(),
        "a refused attempt must not leave an override behind"
    );

    // An administrator is unaffected.
    let admin = UserContext::admin("root".to_string());
    let answer = tokio::task::spawn_blocking({
        let args = args.clone();
        move || execute_native_mcp_tool("set_script_limits", &args, &admin)
    })
    .await
    .expect("no panic")
    .expect("the tool exists");

    assert!(
        answer.get("error").is_none(),
        "an administrator should be able to set limits, got: {}",
        answer
    );

    script_limits::clear(script_uri).await.expect("cleanup");
}

/// Reading is gated the same way. What one script is allowed to spend tells a
/// caller about the engine's capacity and about other tenants' arrangements.
#[tokio::test(flavor = "multi_thread")]
async fn only_an_administrator_may_read_what_scripts_may_spend() {
    use aiwebengine::engine_api::execute_native_mcp_tool;
    use aiwebengine::security::UserContext;
    use serde_json::json;

    setup_env().await;

    let refused = UserContext::editor("an-author".to_string());
    let answer = tokio::task::spawn_blocking(move || {
        execute_native_mcp_tool("get_script_limits", &json!({}), &refused)
    })
    .await
    .expect("no panic")
    .expect("the tool exists");

    assert!(
        answer.get("error").is_some(),
        "listing every script's limits must take an administrator, got: {}",
        answer
    );
}
