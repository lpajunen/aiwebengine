//! The shape `console` presents to a script.
//!
//! The host binding takes one string and throws on anything else, so before
//! this prelude existed `console.log(42)` — or an object, or a caught `Error` —
//! raised a `TypeError` where the script had asked for a log line. The browser
//! console is variadic and stringifies whatever it is given; these tests hold
//! the engine's to that, and to the two places it deliberately parts company:
//! `clear()` does nothing, and inspection is capped.
//!
//! The log store is stubbed. `console` resolves `__writeLog` — the Rust half —
//! from the global scope on every call, so replacing it collects the finished
//! lines without a database behind them, and what the repository does with a
//! line is `SCRIPT_LOGS.md`'s subject rather than this file's.

mod common;

use common::{setup_env, test_mutex};

use aiwebengine::repository;
use aiwebengine::script_eval::{EvalReport, EvalRequest, eval_blocking};
use aiwebengine::security::UserContext;
use serde_json::{Value, json};

/// A stand-in `__writeLog` that collects `[level, message]` pairs instead of
/// writing them, so a test can read back exactly what the formatter produced.
const STUB: &str = r#"
    globalThis.__lines = [];
    globalThis.__writeLog = function (message, level) {
      globalThis.__lines.push([level, message]);
      return "Log written successfully";
    };
"#;

async fn eval(uri: &str, source: &str) -> EvalReport {
    repository::upsert_script(uri, "function init() {}").expect("script should be stored");
    let request = EvalRequest {
        script_uri: uri.to_string(),
        source: format!("{}\n{}", STUB, source),
        user_context: UserContext::admin("console-shape".to_string()),
        timeout_ms: Some(10_000),
        rollback: true,
    };
    tokio::task::spawn_blocking(move || eval_blocking(request))
        .await
        .expect("evaluation panicked")
}

/// Runs `body`, then answers with the collected `[level, message]` pairs.
async fn lines(uri: &str, body: &str) -> Vec<(String, String)> {
    let report = eval(uri, &format!("{}\n__lines", body)).await;
    assert!(report.ok, "{:?}", report.outcome.error);
    let value = report.outcome.value.expect("a value");
    value
        .as_array()
        .expect("an array of lines")
        .iter()
        .map(|pair| {
            let pair = pair.as_array().expect("a [level, message] pair");
            (
                pair[0].as_str().unwrap_or_default().to_string(),
                pair[1].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The messages alone, for the cases where the level is not what is under test.
async fn messages(uri: &str, body: &str) -> Vec<String> {
    lines(uri, body)
        .await
        .into_iter()
        .map(|(_, message)| message)
        .collect()
}

/// The regression this prelude exists for. Every one of these raised
/// `TypeError: Error converting from js 'object' into type 'string'` when the
/// host binding was reached directly — including the numbers and booleans.
#[tokio::test(flavor = "multi_thread")]
async fn a_value_that_is_not_a_string_logs_instead_of_throwing() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = messages(
        "test://console-shape/non-strings",
        r#"
        console.log(42);
        console.log(3.5);
        console.log(true);
        console.log(null);
        console.log(undefined);
        console.log([1, 2, 3]);
        console.log({ a: 1, b: "two" });
        "#,
    )
    .await;

    assert_eq!(
        out,
        vec![
            "42",
            "3.5",
            "true",
            "null",
            "undefined",
            "[ 1, 2, 3 ]",
            r#"{ a: 1, b: "two" }"#,
        ]
    );
}

/// `catch (e) { console.error(e) }` is the line this most affects: it used to
/// throw a second error from inside the handler for the first.
#[tokio::test(flavor = "multi_thread")]
async fn an_error_logs_with_its_name_message_and_stack() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = lines(
        "test://console-shape/error",
        r#"
        try {
          throw new TypeError("boom");
        } catch (e) {
          console.error("failed:", e);
        }
        "#,
    )
    .await;

    assert_eq!(out.len(), 1);
    let (level, message) = &out[0];
    assert_eq!(level, "ERROR");
    assert!(
        message.starts_with("failed: TypeError: boom"),
        "expected the name and message to lead, got {:?}",
        message
    );
    assert!(
        message.contains('\n'),
        "expected a stack under the message, got {:?}",
        message
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn arguments_are_joined_with_spaces() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = messages(
        "test://console-shape/variadic",
        r#"console.log("user:", { id: 7 }, "active:", false);"#,
    )
    .await;

    assert_eq!(out, vec!["user: { id: 7 } active: false"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn format_specifiers_are_substituted() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = messages(
        "test://console-shape/specifiers",
        r#"
        console.log("%s took %dms", "query", 12.7);
        console.log("%i of %f", "42abc", "3.5rem");
        console.log("%o", { a: [1] });
        console.log("%j", { a: 1 });
        console.log("100%% done");
        console.log("%c styled", "color: red");
        console.log("%s and", "one", "two");
        console.log("%s %s", "only-one");
        "#,
    )
    .await;

    assert_eq!(
        out,
        vec![
            "query took 12ms",
            "42 of 3.5",
            "{ a: [ 1 ] }",
            r#"{"a":1}"#,
            // A lone `%%` with no further arguments is not a format string at
            // all, so it stays exactly as written.
            "100%% done",
            " styled",
            "one and two",
            // Nothing left to consume: the specifier survives literally.
            "only-one %s",
        ]
    );
}

/// A naive `JSON.stringify` fallback would throw here, which would have traded
/// one `TypeError` for another on exactly the objects people log most.
#[tokio::test(flavor = "multi_thread")]
async fn a_cycle_is_marked_rather_than_thrown() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = messages(
        "test://console-shape/cycle",
        r#"
        const node = { name: "root" };
        node.self = node;
        console.log(node);

        const shared = { id: 1 };
        console.log({ left: shared, right: shared });
        "#,
    )
    .await;

    assert_eq!(out[0], r#"{ name: "root", self: [Circular] }"#);
    // Two references to one object are not a cycle, and must not be reported
    // as one just because the first was seen already.
    assert_eq!(out[1], "{ left: { id: 1 }, right: { id: 1 } }");
}

#[tokio::test(flavor = "multi_thread")]
async fn inspection_stops_at_the_depth_cap() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = messages(
        "test://console-shape/depth",
        r#"console.log({ a: { b: { c: { d: { e: 1 } } } } });"#,
    )
    .await;

    assert_eq!(out, vec!["{ a: { b: { c: { d: [Object] } } } }"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_long_line_is_truncated_rather_than_written_whole() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = messages(
        "test://console-shape/cap",
        r#"console.log("x".repeat(20000));"#,
    )
    .await;

    assert!(
        out[0].len() < 20000,
        "expected the line to be capped, got {} characters",
        out[0].len()
    );
    assert!(
        out[0].ends_with("more characters"),
        "expected a truncation marker, got the tail {:?}",
        &out[0][out[0].len().saturating_sub(40)..]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn each_method_writes_at_its_own_level() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = lines(
        "test://console-shape/levels",
        r#"
        console.log("a");
        console.info("b");
        console.warn("c");
        console.error("d");
        console.debug("e");
        "#,
    )
    .await;

    let levels: Vec<&str> = out.iter().map(|(level, _)| level.as_str()).collect();
    assert_eq!(levels, vec!["LOG", "INFO", "WARN", "ERROR", "DEBUG"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_group_indents_what_it_contains() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = messages(
        "test://console-shape/group",
        r#"
        console.log("before");
        console.group("import");
        console.log("42 rows");
        console.group();
        console.log("nested");
        console.groupEnd();
        console.groupEnd();
        console.log("after");
        "#,
    )
    .await;

    assert_eq!(
        out,
        vec!["before", "import", "  42 rows", "    nested", "after",]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn counters_and_timers_report_and_reset() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = lines(
        "test://console-shape/counters",
        r#"
        console.count();
        console.count();
        console.count("hits");
        console.countReset();
        console.count();
        console.countReset("never-counted");

        console.time("work");
        console.timeEnd("work");
        console.timeEnd("work");
        "#,
    )
    .await;

    let messages: Vec<&str> = out.iter().map(|(_, m)| m.as_str()).collect();
    assert_eq!(messages[0], "default: 1");
    assert_eq!(messages[1], "default: 2");
    assert_eq!(messages[2], "hits: 1");
    assert_eq!(messages[3], "default: 1");
    assert_eq!(
        out[4],
        (
            "WARN".into(),
            "Count for 'never-counted' does not exist".into()
        )
    );

    assert!(
        messages[5].starts_with("work: ") && messages[5].ends_with("ms"),
        "expected an elapsed time, got {:?}",
        messages[5]
    );
    // The timer is gone once reported, so asking again is the warning case.
    assert_eq!(
        out[6],
        ("WARN".into(), "Timer 'work' does not exist".into())
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn assert_writes_only_when_the_condition_fails() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = lines(
        "test://console-shape/assert",
        r#"
        console.assert(true, "never written");
        console.assert(false, "expected rows for", "users");
        console.assert(false);
        "#,
    )
    .await;

    assert_eq!(
        out,
        vec![
            (
                "ERROR".to_string(),
                "Assertion failed: expected rows for users".to_string()
            ),
            ("ERROR".to_string(), "Assertion failed".to_string()),
        ]
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn table_renders_rows_and_columns() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = messages(
        "test://console-shape/table",
        r#"console.table([{ id: 1, name: "ada" }, { id: 2, name: "linus" }], ["id"]);"#,
    )
    .await;

    let table = &out[0];
    assert!(table.contains("(index)"), "{}", table);
    assert!(table.contains("id"), "{}", table);
    // The column was filtered out, so it must not appear.
    assert!(!table.contains("name"), "{}", table);
    assert!(!table.contains("ada"), "{}", table);
    assert_eq!(table.lines().count(), 6, "{}", table);
}

/// Pruning stored log lines is engine administration. `clear` exists so the
/// call is not a `ReferenceError`, and does nothing so it cannot be mistaken
/// for one that works.
#[tokio::test(flavor = "multi_thread")]
async fn clear_does_nothing_and_leaves_the_log_alone() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = messages(
        "test://console-shape/clear",
        r#"
        console.log("kept");
        console.clear();
        "#,
    )
    .await;

    assert_eq!(out, vec!["kept"]);
}

/// The browser's console answers `undefined`. The host call does return a
/// status string, but surfacing it would give `console.log` a return value no
/// browser gives it — and the type declaration has always said `void`.
#[tokio::test(flavor = "multi_thread")]
async fn every_method_answers_undefined() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let report = eval(
        "test://console-shape/returns",
        r#"
        [
          console.log("a"),
          console.warn("b"),
          console.group("c"),
          console.groupEnd(),
          console.count(),
          console.clear(),
        ].every(function (value) { return value === undefined; })
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    assert_eq!(report.outcome.value, Some(json!(true)));
}

/// Logging is one of the few things a script does inside a `catch`, so the
/// formatter must not itself become a source of throws.
#[tokio::test(flavor = "multi_thread")]
async fn a_throwing_getter_does_not_take_the_line_with_it() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = messages(
        "test://console-shape/getter",
        r#"
        const hostile = { ok: 1 };
        Object.defineProperty(hostile, "bad", {
          enumerable: true,
          get: function () { throw new Error("nope"); },
        });
        console.log(hostile);
        "#,
    )
    .await;

    assert_eq!(out, vec!["{ ok: 1, bad: [Getter threw] }"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn maps_sets_and_dates_render_as_themselves() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = messages(
        "test://console-shape/collections",
        r#"
        console.log(new Map([["a", 1]]));
        console.log(new Set([1, 2]));
        console.log(new Date(0));
        console.log(/ab+c/gi);
        console.log(function named() {});
        console.log(Symbol("s"));
        "#,
    )
    .await;

    assert_eq!(out[0], r#"Map(1) { "a" => 1 }"#);
    assert_eq!(out[1], "Set(2) { 1, 2 }");
    assert_eq!(out[2], "1970-01-01T00:00:00.000Z");
    assert_eq!(out[3], "/ab+c/gi");
    assert_eq!(out[4], "[Function: named]");
    assert_eq!(out[5], "Symbol(s)");
}

/// A string logged on its own is written bare; nested inside a structure it is
/// quoted, so the boundaries of a value with spaces in it stay visible.
#[tokio::test(flavor = "multi_thread")]
async fn strings_are_bare_at_the_top_level_and_quoted_inside() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = messages(
        "test://console-shape/strings",
        r#"
        console.log("plain text");
        console.log(["plain text"]);
        "#,
    )
    .await;

    assert_eq!(out[0], "plain text");
    assert_eq!(out[1], r#"[ "plain text" ]"#);
}

/// Nothing above changes the levels the repository already understands, so a
/// line written through the real host call still lands where it did.
#[tokio::test(flavor = "multi_thread")]
async fn the_real_host_call_still_accepts_a_formatted_line() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // No stub: this one goes all the way through to the repository.
    repository::upsert_script("test://console-shape/real", "function init() {}")
        .expect("script should be stored");
    let request = EvalRequest {
        script_uri: "test://console-shape/real".to_string(),
        source: r#"console.log("row", { id: 1 }); "written""#.to_string(),
        user_context: UserContext::admin("console-shape".to_string()),
        timeout_ms: Some(10_000),
        rollback: true,
    };
    let report = tokio::task::spawn_blocking(move || eval_blocking(request))
        .await
        .expect("evaluation panicked");

    assert!(report.ok, "{:?}", report.outcome.error);
    assert_eq!(report.outcome.value, Some(Value::String("written".into())));
}
