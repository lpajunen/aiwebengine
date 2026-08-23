//! The shape `sharedStorage` and `personalStorage` present to a script.
//!
//! Both are the WHATWG `Storage` interface now, so what a browser teaches about
//! `localStorage` holds here: a missing key is `null`, keys and values are
//! coerced with `String()`, `length`/`key(i)`/named access work, and a write
//! that cannot be done throws rather than answering with prose about it.
//!
//! The write path is what these tests are really for. `setItem` used to return
//! `"Error: …"` while the type declaration said `void`, so a quota overflow —
//! or writing to personal storage with nobody logged in — was invisible to a
//! script that believed its own types.

mod common;

use aiwebengine::repository;
use aiwebengine::script_eval::{EvalReport, EvalRequest, eval_blocking};
use aiwebengine::security::UserContext;
use serde_json::json;
use std::sync::OnceLock;
use tokio::sync::{Mutex, OnceCell};

static INIT: OnceCell<()> = OnceCell::const_new();
static TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

fn test_mutex() -> &'static Mutex<()> {
    TEST_MUTEX.get_or_init(|| Mutex::new(()))
}

async fn setup_env() {
    INIT.get_or_init(|| async {
        let config = aiwebengine::config::AppConfig::test_config_postgres(0);
        if let Ok(db) = aiwebengine::database::Database::new(&config.repository).await {
            let db_arc = std::sync::Arc::new(db);
            aiwebengine::database::initialize_global_database(db_arc.clone());
            repository::initialize_repository(repository::PostgresRepository::new(
                db_arc.pool().clone(),
                "test".to_string(),
            ));
        }
    })
    .await;
}

/// Evaluates `source` against a deployed script that does nothing itself.
///
/// `rollback: false`, because these tests are about what the store holds after
/// a write, and a rolled-back transaction would take every write with it.
async fn eval(uri: &str, source: &str) -> EvalReport {
    repository::upsert_script(uri, "function init() {}").expect("script should be stored");
    let request = EvalRequest {
        script_uri: uri.to_string(),
        source: source.to_string(),
        user_context: UserContext::admin("storage-shape".to_string()),
        timeout_ms: Some(10_000),
        rollback: false,
    };
    tokio::task::spawn_blocking(move || eval_blocking(request))
        .await
        .expect("evaluation panicked")
}

/// Runs `source` and answers with the value it produced.
async fn value(uri: &str, source: &str) -> serde_json::Value {
    let report = eval(uri, source).await;
    assert!(report.ok, "{:?}", report.outcome.error);
    report.outcome.value.expect("a value")
}

/// Shared storage belongs to the script, so an evaluation reaches it whether or
/// not anyone is logged in. Every test below starts from a known-empty store.
const RESET: &str = "sharedStorage.clear();";

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_key_reads_as_null() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://storage-shape/missing",
        &format!(
            "{}\n{}",
            RESET, r#"[sharedStorage.getItem("nope"), sharedStorage.length]"#
        ),
    )
    .await;

    assert_eq!(out, json!([serde_json::Value::Null, 0]));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_value_survives_a_round_trip() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://storage-shape/round-trip",
        &format!(
            "{}\n{}",
            RESET,
            r#"
            sharedStorage.setItem("theme", "dark");
            sharedStorage.getItem("theme")
            "#
        ),
    )
    .await;

    assert_eq!(out, json!("dark"));
}

/// The spec coerces both, so `setItem("count", 1)` stores the string `"1"`.
/// Reaching the host binding with a number used to be a TypeError.
#[tokio::test(flavor = "multi_thread")]
async fn keys_and_values_are_coerced_to_strings() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://storage-shape/coercion",
        &format!(
            "{}\n{}",
            RESET,
            r#"
            sharedStorage.setItem("count", 1);
            sharedStorage.setItem(2, true);
            sharedStorage.setItem("nothing", null);
            [
              sharedStorage.getItem("count"),
              sharedStorage.getItem("2"),
              sharedStorage.getItem(2),
              sharedStorage.getItem("nothing"),
            ]
            "#
        ),
    )
    .await;

    assert_eq!(out, json!(["1", "true", "true", "null"]));
}

#[tokio::test(flavor = "multi_thread")]
async fn setting_a_value_answers_undefined_rather_than_a_status_string() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://storage-shape/returns",
        &format!(
            "{}\n{}",
            RESET,
            r#"
            [
              sharedStorage.setItem("a", "1"),
              sharedStorage.removeItem("a"),
              sharedStorage.clear(),
            ].every(function (v) { return v === undefined; })
            "#
        ),
    )
    .await;

    assert_eq!(out, json!(true));
}

/// The failure this whole change exists for: a write too large to store used to
/// answer `"Error: Value too large (>1MB)"` from a method declared `void`.
#[tokio::test(flavor = "multi_thread")]
async fn an_oversized_value_throws_quota_exceeded() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://storage-shape/quota",
        &format!(
            "{}\n{}",
            RESET,
            r#"
            try {
              sharedStorage.setItem("big", "x".repeat(1000001));
              "no throw";
            } catch (e) {
              [e.name, e instanceof DOMException, e instanceof Error];
            }
            "#
        ),
    )
    .await;

    assert_eq!(out, json!(["QuotaExceededError", true, true]));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_empty_key_throws_rather_than_writing() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://storage-shape/empty-key",
        &format!(
            "{}\n{}",
            RESET,
            r#"
            try {
              sharedStorage.setItem("   ", "value");
              "no throw";
            } catch (e) {
              e.name;
            }
            "#
        ),
    )
    .await;

    assert_eq!(out, json!("SyntaxError"));
}

/// Personal storage is keyed by an authenticated user. An evaluation has no
/// request behind it, so there is nobody to store for — which is a different
/// answer from "the key is not there", and now says so.
#[tokio::test(flavor = "multi_thread")]
async fn personal_storage_throws_security_error_without_a_user() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://storage-shape/unauthenticated",
        r#"
        function nameOfThrow(fn) {
          try {
            fn();
            return "no throw";
          } catch (e) {
            return e.name;
          }
        }

        [
          nameOfThrow(function () { return personalStorage.getItem("theme"); }),
          nameOfThrow(function () { personalStorage.setItem("theme", "dark"); }),
          nameOfThrow(function () { personalStorage.removeItem("theme"); }),
          nameOfThrow(function () { personalStorage.clear(); }),
          nameOfThrow(function () { return personalStorage.length; }),
          nameOfThrow(function () { return personalStorage.key(0); }),
          nameOfThrow(function () { personalStorage.theme = "dark"; }),
          nameOfThrow(function () { delete personalStorage.theme; }),
        ]
        "#,
    )
    .await;

    // Every one of these says why rather than answering `null`, `0` or
    // `undefined` — which would report an empty store when the truth is that
    // there is no store to read.
    assert_eq!(out, json!(vec!["SecurityError"; 8]));
}

/// The other half of that rule. Reading a *property* is how the language pokes
/// at any object — `JSON.stringify` looks for `toJSON`, `await` looks for
/// `then` — so a property read that throws would make the store unusable by
/// anything generic. Those answer as if the store were empty.
#[tokio::test(flavor = "multi_thread")]
async fn reading_a_property_of_an_unavailable_store_stays_quiet() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://storage-shape/quiet-reads",
        r#"
        (async function () {
          return [
            personalStorage.theme === undefined,
            "theme" in personalStorage,
            JSON.stringify(personalStorage),
            Object.keys(personalStorage),
            // `await` reaches for `then`; a throwing read would surface here.
            (await personalStorage) !== null,
          ];
        })()
        "#,
    )
    .await;

    assert_eq!(out, json!([true, false, "{}", [], true]));
}

/// Inspecting a store must never throw, whoever is asking. `console.log` and
/// `JSON.stringify` reach for the reflection traps, and a log line that raises
/// is the failure mode this whole direction exists to remove.
#[tokio::test(flavor = "multi_thread")]
async fn an_unavailable_store_inspects_as_empty_rather_than_throwing() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://storage-shape/inspect-unavailable",
        r#"
        [
          Object.keys(personalStorage),
          JSON.stringify(personalStorage),
          JSON.stringify(Object.assign({}, personalStorage)),
        ]
        "#,
    )
    .await;

    assert_eq!(out, json!([[], "{}", "{}"]));
}

/// A store converts to a string the way any object does. Without the interface
/// check covering inherited properties, the proxy answered `undefined` for both
/// `toString` and `valueOf`, and a template literal could not convert the store
/// at all — it threw `TypeError: toPrimitive`.
#[tokio::test(flavor = "multi_thread")]
async fn a_store_converts_to_a_string() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://storage-shape/to-primitive",
        &format!(
            "{}\n{}",
            RESET,
            r#"
            sharedStorage.setItem("toString", "stored");
            [
              String(sharedStorage),
              `${sharedStorage}`,
              typeof sharedStorage.toString,
              typeof sharedStorage.valueOf,
              typeof sharedStorage.hasOwnProperty,
              // The stored key is still reachable the explicit way.
              sharedStorage.getItem("toString"),
            ]
            "#
        ),
    )
    .await;

    assert_eq!(
        out,
        json!([
            "[object Object]",
            "[object Object]",
            "function",
            "function",
            "function",
            "stored"
        ])
    );
}

/// `length` and `key(i)` are what the interface indexes its keys with, and the
/// order is ascending so an index means the same thing twice running.
#[tokio::test(flavor = "multi_thread")]
async fn length_and_key_enumerate_in_a_stable_order() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://storage-shape/enumerate",
        &format!(
            "{}\n{}",
            RESET,
            r#"
            sharedStorage.setItem("gamma", "3");
            sharedStorage.setItem("alpha", "1");
            sharedStorage.setItem("beta", "2");

            const collected = [];
            for (let i = 0; i < sharedStorage.length; i++) {
              collected.push(sharedStorage.key(i));
            }
            [
              sharedStorage.length,
              collected,
              sharedStorage.key(99),
              sharedStorage.key(-1),
            ]
            "#
        ),
    )
    .await;

    assert_eq!(
        out,
        json!([
            3,
            ["alpha", "beta", "gamma"],
            serde_json::Value::Null,
            serde_json::Value::Null
        ])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn named_access_reads_writes_and_deletes() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://storage-shape/named",
        &format!(
            "{}\n{}",
            RESET,
            r#"
            sharedStorage.theme = "dark";
            const read = sharedStorage.theme;
            const present = "theme" in sharedStorage;
            const absent = "nothing" in sharedStorage;
            const missing = sharedStorage.nothing;
            delete sharedStorage.theme;
            [read, present, absent, missing === undefined, sharedStorage.getItem("theme")]
            "#
        ),
    )
    .await;

    assert_eq!(
        out,
        json!(["dark", true, false, true, serde_json::Value::Null])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_store_enumerates_as_an_object() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://storage-shape/object-keys",
        &format!(
            "{}\n{}",
            RESET,
            r#"
            sharedStorage.setItem("b", "2");
            sharedStorage.setItem("a", "1");
            [Object.keys(sharedStorage), JSON.stringify(Object.assign({}, sharedStorage))]
            "#
        ),
    )
    .await;

    assert_eq!(out, json!([["a", "b"], r#"{"a":"1","b":"2"}"#]));
}

/// A stored key that collides with a member of the interface is reachable
/// through `getItem`, and the member still wins as a property — which is what a
/// browser does, where the prototype shadows the named properties.
#[tokio::test(flavor = "multi_thread")]
async fn an_interface_member_is_not_shadowed_by_a_stored_key() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://storage-shape/shadowing",
        &format!(
            "{}\n{}",
            RESET,
            r#"
            sharedStorage.setItem("getItem", "stored");
            sharedStorage.setItem("length", "stored");
            [
              typeof sharedStorage.getItem,
              typeof sharedStorage.length,
              sharedStorage.getItem("getItem"),
              sharedStorage.getItem("length"),
            ]
            "#
        ),
    )
    .await;

    assert_eq!(out, json!(["function", "number", "stored", "stored"]));
}

#[tokio::test(flavor = "multi_thread")]
async fn clear_empties_the_store() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://storage-shape/clear",
        &format!(
            "{}\n{}",
            RESET,
            r#"
            sharedStorage.setItem("a", "1");
            sharedStorage.setItem("b", "2");
            const before = sharedStorage.length;
            sharedStorage.clear();
            [before, sharedStorage.length, sharedStorage.getItem("a")]
            "#
        ),
    )
    .await;

    assert_eq!(out, json!([2, 0, serde_json::Value::Null]));
}

/// Removing a key that was never there is not an error, in a browser or here.
#[tokio::test(flavor = "multi_thread")]
async fn removing_an_absent_key_is_not_an_error() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://storage-shape/remove-absent",
        &format!(
            "{}\n{}",
            RESET,
            r#"
            sharedStorage.removeItem("never-stored");
            "survived"
            "#
        ),
    )
    .await;

    assert_eq!(out, json!("survived"));
}

/// The interface's arity checks, which a browser raises as `TypeError` before
/// it looks at the store at all.
#[tokio::test(flavor = "multi_thread")]
async fn calling_without_the_required_arguments_throws_a_type_error() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://storage-shape/arity",
        &format!(
            "{}\n{}",
            RESET,
            r#"
            function nameOfThrow(fn) {
              try {
                fn();
                return "no throw";
              } catch (e) {
                return e.name;
              }
            }
            [
              nameOfThrow(function () { return sharedStorage.getItem(); }),
              nameOfThrow(function () { sharedStorage.setItem("only-key"); }),
              nameOfThrow(function () { sharedStorage.removeItem(); }),
            ]
            "#
        ),
    )
    .await;

    assert_eq!(out, json!(["TypeError", "TypeError", "TypeError"]));
}

/// Two stores, two namespaces: writing to one must not be visible in the other.
#[tokio::test(flavor = "multi_thread")]
async fn the_two_stores_do_not_share_keys() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://storage-shape/separate",
        &format!(
            "{}\n{}",
            RESET,
            r#"
            sharedStorage.setItem("only-shared", "yes");
            try {
              personalStorage.getItem("only-shared");
              "read without a user";
            } catch (e) {
              [e.name, sharedStorage.getItem("only-shared")];
            }
            "#
        ),
    )
    .await;

    assert_eq!(out, json!(["SecurityError", "yes"]));
}
