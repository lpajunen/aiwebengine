//! Which connection a schema change runs on.
//!
//! Schema work used to take its own pooled connection while the caller's
//! transaction stayed open on another. `CREATE INDEX` takes SHARE and `ALTER
//! TABLE` takes ACCESS EXCLUSIVE, so the moment the caller had written to the
//! same table its own ROW EXCLUSIVE blocked the schema change — and Postgres
//! could not see that as a deadlock, because the holder was waiting on the
//! engine rather than on the database. The statement blocked until the
//! connection died, with every later writer queued behind the pending strong
//! lock: reads on the table stayed fast while every write in the cluster
//! stopped.
//!
//! Schema changes now join the caller's transaction, where a connection cannot
//! block on locks it already holds.

mod common;

use aiwebengine::repository;
use aiwebengine::script_eval::{EvalReport, EvalRequest, eval_blocking};
use aiwebengine::security::UserContext;
use serde_json::json;
use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::{Mutex, OnceCell};

static INIT: OnceCell<()> = OnceCell::const_new();
static TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();

/// How long a schema change gets before the test calls it wedged.
///
/// Every operation here is milliseconds of work against a table holding a
/// handful of rows. The margin is for a loaded CI machine, not for the lock.
const WEDGE_TIMEOUT: Duration = Duration::from_secs(20);

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

/// Evaluates `source` with `rollback` deciding whether its writes survive.
///
/// A rolled-back evaluation is the engine's own `beginTransaction`, so the
/// snippet runs with a transaction already open — the state a script is in
/// inside `transaction()`, and the one that used to wedge.
///
/// The timeout is what makes a regression visible. The blocking thread cannot
/// be cancelled once it is parked inside a lock wait — that is the whole
/// problem — so the test gives up on it rather than hanging until the run is
/// killed with no verdict.
async fn eval(uri: &str, rollback: bool, source: &str) -> EvalReport {
    repository::upsert_script(uri, "function init() {}").expect("script should be stored");

    let request = EvalRequest {
        script_uri: uri.to_string(),
        source: source.to_string(),
        user_context: UserContext::admin("schema-locks".to_string()),
        timeout_ms: Some(10_000),
        rollback,
    };

    tokio::time::timeout(
        WEDGE_TIMEOUT,
        tokio::task::spawn_blocking(move || eval_blocking(request)),
    )
    .await
    .expect("a schema change blocked on a lock the caller itself holds")
    .expect("evaluation panicked")
}

/// Drops `table` for real, so a run starts from nothing.
///
/// The rolled-back evaluations below cannot clean up after themselves — that is
/// the property two of them are checking — so the cleanup has to commit.
async fn drop_table(uri: &str, table: &str) {
    let report = eval(uri, false, &format!(r#"database.dropTable("{}")"#, table)).await;
    assert!(report.ok, "{:?}", report.outcome.error);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_schema_change_completes_while_the_caller_holds_the_table() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // The shape that wedged: write a row, then index the column just written.
    // A solution's `ensureSchema()` called from inside `transaction()` on the
    // first write of the process does exactly this.
    drop_table("test://schema-locks/index", "items").await;

    let report = eval(
        "test://schema-locks/index",
        true,
        r#"
        database.createTable("items");
        database.addTextColumn("items", "item_id", true);
        database.insert("items", JSON.stringify({ item_id: "sword" }));
        const indexed = database.addUniqueIndex("items", JSON.stringify(["item_id"])).json();
        ({ indexed: indexed.success === true, error: indexed.error || null })
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let value = report.outcome.value.expect("a value");
    assert_eq!(value["error"], json!(null));
    assert_eq!(value["indexed"], json!(true));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_schema_change_leaves_the_transaction_usable() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // Joining the transaction is what makes this a question at all: a statement
    // that errors aborts the transaction it ran in, and every call after it
    // would fail with "current transaction is aborted". The savepoint around
    // each schema change is what keeps the caller's transaction alive — and
    // these calls answer with `{ error }` rather than throwing, so scripts
    // routinely carry on after one.
    drop_table("test://schema-locks/recover", "dupes").await;

    let report = eval(
        "test://schema-locks/recover",
        true,
        r#"
        database.createTable("dupes");
        database.addTextColumn("dupes", "label", true);
        database.insert("dupes", JSON.stringify({ label: "same" }));
        database.insert("dupes", JSON.stringify({ label: "same" }));

        // Read as a string rather than with `.json()`: the engine interpolates
        // the driver's message into `{"error": "..."}` without escaping it, and
        // a duplicate-key message carries quotes of its own.
        const failed = String(database.addUniqueIndex("dupes", JSON.stringify(["label"])));
        const rows = database.query("dupes").json();
        database.insert("dupes", JSON.stringify({ label: "third" }));

        ({
          refused: failed.indexOf("error") >= 0,
          readable: rows.length,
          writable: database.query("dupes").json().length,
        })
        "#,
    )
    .await;

    assert!(report.ok, "{:?}", report.outcome.error);
    let value = report.outcome.value.expect("a value");
    assert_eq!(
        value["refused"],
        json!(true),
        "a unique index over duplicate values must still be refused"
    );
    assert_eq!(value["readable"], json!(2));
    assert_eq!(value["writable"], json!(3));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rolled_back_transaction_takes_its_schema_with_it() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    // The other half of joining the transaction, and the reason `--rollback`
    // means what it says: a table created inside one used to be committed by a
    // connection the rollback never reached, so an evaluation that promised to
    // leave nothing behind left a table behind every time.
    drop_table("test://schema-locks/rollback", "ephemeral").await;

    let created = eval(
        "test://schema-locks/rollback",
        true,
        r#"
        database.createTable("ephemeral");
        database.addTextColumn("ephemeral", "label", true);
        ({ created: true })
        "#,
    )
    .await;
    assert!(created.ok, "{:?}", created.outcome.error);

    let after = eval(
        "test://schema-locks/rollback",
        true,
        r#"
        const answer = database.query("ephemeral").json();
        ({ gone: typeof answer.error === "string" })
        "#,
    )
    .await;

    assert!(after.ok, "{:?}", after.outcome.error);
    let value = after.outcome.value.expect("a value");
    assert_eq!(
        value["gone"],
        json!(true),
        "the rolled-back table must not have survived"
    );
}
