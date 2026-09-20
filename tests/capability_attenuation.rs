//! A script running part of its own work with fewer capabilities than it holds.
//!
//! The unit under test is the boundary rather than the plumbing: what these
//! assert is that a narrowed sub-execution *cannot* do the thing it was
//! narrowed out of, at the gate underneath the JavaScript rather than in
//! anything the running code could talk its way past. `src/sandbox.rs` covers
//! the narrowing arithmetic; these run real code in a real sub-execution and
//! check that the refusal arrives.
//!
//! The shape they describe is the one item 1 of `TODO-agent.md` asked for:
//!
//!   - model-authored code evaluated holding a chosen subset, and
//!   - a plan mode that is enforced rather than offered — a turn that may read
//!     everything and write nothing, with no arrangement of the transcript
//!     reaching past it.

mod common;

use common::{setup_env, test_mutex};

use aiwebengine::repository;
use aiwebengine::script_eval::{EvalReport, EvalRequest, eval_blocking};
use aiwebengine::security::UserContext;
use serde_json::{Value, json};

/// The outer turn: an administrator running against a script that does
/// nothing itself. Everything interesting happens in the sub-execution the
/// source starts, so the outer context is deliberately the widest one there
/// is — a narrowing that only worked from an already-narrow caller would
/// prove nothing.
async fn outer(uri: &str, source: &str) -> EvalReport {
    repository::upsert_script(uri, "function init() {}").expect("script should be stored");
    let request = EvalRequest {
        timeout_ms: Some(15_000),
        rollback: false,
        // A person, because half of what a narrowed turn has to carry
        // through is who it is acting as, and an evaluation with no auth
        // context could not tell a carried identity from an absent one.
        auth_context: Some(aiwebengine::auth::JsAuthContext::authenticated(
            "attenuation".to_string(),
            Some("someone@example.test".to_string()),
            Some("Someone".to_string()),
            "internal".to_string(),
            true,
            true,
        )),
        ..EvalRequest::new(
            uri.to_string(),
            source.to_string(),
            UserContext::admin("attenuation".to_string()),
        )
    };
    tokio::task::spawn_blocking(move || eval_blocking(request))
        .await
        .expect("evaluation panicked")
}

/// The value the outer turn produced.
async fn value(uri: &str, source: &str) -> Value {
    let report = outer(uri, source).await;
    assert!(report.ok, "outer turn failed: {:?}", report.outcome.error);
    report
        .outcome
        .value
        .expect("the outer turn produced a value")
}

/// A sub-execution that was given nothing still computes, because computing
/// is not a capability. This is the base case the rest subtract from — and
/// the reason the primitive is worth having at all: model-authored code can
/// be run with no authority whatsoever and still answer.
#[tokio::test(flavor = "multi_thread")]
async fn code_runs_with_no_capabilities_at_all() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://attenuation/compute",
        r#"
        const run = sandbox.run("context.args.a + context.args.b", {
            capabilities: [],
            input: { a: 2, b: 40 },
        });
        ({ value: run.value, error: run.error })
        "#,
    )
    .await;

    assert_eq!(out["value"], json!(42));
    assert_eq!(out["error"], Value::Null);
}

/// The plan-mode case, and the one that was inexpressible before
/// `UseScriptDatabase` was split: a turn that reads a table and cannot write
/// it. Both halves are asserted in one sub-execution, because "the read
/// worked" is what makes "the write was refused" mean something other than
/// "nothing worked".
#[tokio::test(flavor = "multi_thread")]
async fn a_planning_turn_reads_and_cannot_write() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://attenuation/plan",
        r#"
        database.ensureTable("notes", JSON.stringify({
            columns: [{ name: "body", type: "text" }],
        }));
        database.insert("notes", JSON.stringify({ body: "before" }));

        const run = sandbox.run(`
            const rows = database.query("notes").json();
            // The database answers with an envelope rather than throwing, so
            // a refusal is read rather than caught.
            const write = database.insert("notes", JSON.stringify({ body: "during" })).json();
            ({ read: rows.length, refused: write.error || null });
        `, { capabilities: ["read_script_data"] });

        ({
            value: run.value,
            error: run.error,
            after: database.query("notes").json().length,
        })
        "#,
    )
    .await;

    assert_eq!(out["error"], Value::Null, "the planning turn should run");
    assert_eq!(out["value"]["read"], json!(1), "the read must work");
    assert!(
        out["value"]["refused"]
            .as_str()
            .unwrap_or_default()
            .contains("write_script_data"),
        "the write should name the capability it wanted: {}",
        out["value"]["refused"]
    );
    assert_eq!(
        out["after"],
        json!(1),
        "the refused write must not have landed"
    );
}

/// `fetch` was gated by nothing at all before this, which made every other
/// restriction decorative: code that could call out could spend tokens, post
/// anywhere, and carry whatever it had been shown with it.
#[tokio::test(flavor = "multi_thread")]
async fn a_narrowed_turn_cannot_reach_the_network() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://attenuation/network",
        r#"
        const run = sandbox.run(`
            try {
                fetch("https://example.com");
                "not refused";
            } catch (e) {
                String(e.message || e);
            }
        `, { capabilities: ["read_script_data"] });
        run.value
        "#,
    )
    .await;

    assert!(
        out.as_str().unwrap_or_default().contains("use_network"),
        "fetch should be refused by name: {}",
        out
    );
}

/// Queueing is how an execution outlives itself. A turn that may not write
/// now must not be able to arrange a write for later, since the task runs in
/// script context holding what the script holds rather than what this turn
/// was narrowed to.
#[tokio::test(flavor = "multi_thread")]
async fn a_narrowed_turn_cannot_queue_work_for_later() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://attenuation/queue",
        r#"
        const run = sandbox.run(`
            try {
                scriptTasks.enqueue({ handler: "later", payload: {} });
                "not refused";
            } catch (e) {
                String(e.message || e);
            }
        `, { capabilities: ["read_script_data"] });
        run.value
        "#,
    )
    .await;

    assert!(
        out.as_str().unwrap_or_default().contains("enqueue_tasks"),
        "enqueueing should be refused by name: {}",
        out
    );
}

/// Asking for more than the caller holds is refused where it was asked,
/// rather than becoming a puzzling refusal from inside the sub-execution.
/// A request-level mistake throws; only what the *code* did comes back as a
/// value.
#[tokio::test(flavor = "multi_thread")]
async fn asking_for_more_than_the_caller_holds_throws_at_the_call() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://attenuation/refused",
        r#"
        const answers = [];
        // Narrow once, then try to climb back out from inside.
        const run = sandbox.run(`
            try {
                sandbox.run("1", { capabilities: ["write_script_data"] });
                "not refused";
            } catch (e) {
                String(e.message || e);
            }
        `, { capabilities: ["read_script_data"] });
        answers.push(run.value);

        // And a name the engine does not have is a mistake, not a narrowing.
        try {
            sandbox.run("1", { capabilities: ["read_only"] });
            answers.push("not refused");
        } catch (e) {
            answers.push(String(e.message || e));
        }
        answers
        "#,
    )
    .await;

    let inner = out[0].as_str().unwrap_or_default();
    assert!(
        inner.contains("write_script_data") && inner.contains("does not hold"),
        "a narrowed turn must not be able to widen itself: {}",
        inner
    );
    assert!(
        out[1]
            .as_str()
            .unwrap_or_default()
            .contains("not a capability"),
        "an unknown name should be refused as one: {}",
        out[1]
    );
}

/// The sub-execution's own output comes back whether it succeeded or not.
/// For an agent this is usually the most useful part of a failed turn: the
/// model's code printed its reasoning and then threw, and both halves are
/// what goes back into the next prompt.
#[tokio::test(flavor = "multi_thread")]
async fn console_and_failures_both_come_back_as_values() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://attenuation/output",
        r#"
        const run = sandbox.run(`
            console.log("thinking out loud");
            throw new Error("did not work");
        `, { capabilities: [] });
        ({
            error: run.error,
            printed: run.console.map(line => line.message),
        })
        "#,
    )
    .await;

    assert!(
        out["error"]
            .as_str()
            .unwrap_or_default()
            .contains("did not work"),
        "a throw inside should be reported rather than thrown out: {}",
        out["error"]
    );
    assert!(
        out["printed"]
            .as_array()
            .map(|lines| lines.iter().any(|line| line
                .as_str()
                .unwrap_or_default()
                .contains("thinking out loud")))
            .unwrap_or(false),
        "console output should survive the failure: {}",
        out["printed"]
    );
}

/// Nesting is bounded. Each level is a live runtime and a stack frame, and
/// the shared budget bounds how *long* a chain runs without bounding how
/// deep it goes.
#[tokio::test(flavor = "multi_thread")]
async fn nesting_is_bounded() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://attenuation/depth",
        r#"
        // Each level runs the same source one level down, so the chain ends
        // at the depth limit rather than at anything the script counted.
        const recurse = `
            try {
                const inner = sandbox.run(context.args.source, {
                    capabilities: [],
                    input: { source: context.args.source },
                });
                inner.value === undefined ? "ran" : inner.value;
            } catch (e) {
                String(e.message || e);
            }
        `;
        const run = sandbox.run(recurse, {
            capabilities: [],
            input: { source: recurse },
        });
        run.value
        "#,
    )
    .await;

    assert!(
        out.as_str().unwrap_or_default().contains("limit"),
        "a chain of sub-executions should bottom out: {}",
        out
    );
}

/// The identity carries through. Attenuation says what may be done, not who
/// is doing it — a narrowed turn that changed identity would silently move a
/// script's reads and writes into a different person's rows.
#[tokio::test(flavor = "multi_thread")]
async fn the_person_carries_into_the_sub_execution() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://attenuation/identity",
        r#"
        const run = sandbox.run(
            "context.request.auth.userId",
            { capabilities: [] },
        );
        ({ inner: run.value, outer: context.request.auth.userId })
        "#,
    )
    .await;

    assert_eq!(
        out["inner"], out["outer"],
        "the sub-execution runs as the same person: {}",
        out
    );
}
