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

        const failed = database.addUniqueIndex("dupes", JSON.stringify(["label"])).json();
        const rows = database.query("dupes").json();
        database.insert("dupes", JSON.stringify({ label: "third" }));

        ({
          refused: typeof failed.error === "string",
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

#[tokio::test(flavor = "multi_thread")]
async fn racing_handlers_creating_one_table_get_one_winner_and_a_clear_answer() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    const URI: &str = "test://schema-locks/race";
    const RACERS: usize = 6;

    repository::upsert_script(URI, "function init() {}").expect("script should be stored");
    let _ = tokio::task::spawn_blocking(|| repository::drop_script_table(URI, "contested")).await;

    // The shape of a cold start: every instance's first write calls
    // `ensureSchema()` at once, and each one's existence check runs before any
    // of them has created anything.
    let mut racers = Vec::new();
    for _ in 0..RACERS {
        racers.push(tokio::task::spawn_blocking(|| {
            repository::create_script_table(URI, "contested")
        }));
    }

    let mut created = 0;
    let mut refused = Vec::new();
    for racer in racers {
        match racer.await.expect("racer panicked") {
            Ok(_) => created += 1,
            Err(e) => refused.push(e.to_string()),
        }
    }

    assert_eq!(created, 1, "exactly one racer should create the table");

    // The point of serialising them. A loser that ran its existence check
    // before the winner committed used to reach `CREATE TABLE` anyway and get
    // Postgres's own complaint, which names a physical table the script has
    // never heard of.
    for message in &refused {
        assert!(
            message.contains("already exists for this script"),
            "a loser should get the engine's answer, not the database's: {}",
            message
        );
    }
    assert_eq!(refused.len(), RACERS - 1);

    let _ = tokio::task::spawn_blocking(|| repository::drop_script_table(URI, "contested")).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn ensuring_a_table_converges_it_and_then_does_nothing() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    const SCHEMA: &str = r#"JSON.stringify({
        columns: [
          { name: "item_id", type: "text" },
          { name: "owner", type: "text" },
          { name: "updated_at", type: "bigint" },
        ],
        uniqueIndexes: [["item_id"]],
    })"#;

    drop_table("test://schema-locks/ensure", "world_items").await;

    // First run builds it. Note this is inside a transaction, which is where a
    // solution's ensureSchema() actually runs and where it used to wedge.
    let first = eval(
        "test://schema-locks/ensure",
        false,
        &format!(
            r#"
            const answer = database.ensureTable("world_items", {SCHEMA}).json();
            ({{ created: answer.created, added: answer.columnsAdded, error: answer.error || null }})
            "#
        ),
    )
    .await;

    assert!(first.ok, "{:?}", first.outcome.error);
    let value = first.outcome.value.expect("a value");
    assert_eq!(value["error"], json!(null));
    assert_eq!(value["created"], json!(true));
    assert_eq!(
        value["added"],
        json!(["item_id", "owner", "updated_at"]),
        "the first run should add every column asked for"
    );

    // Second run is the one that matters: converging an already-correct table
    // has to be a no-op that reports itself, not an exception to swallow.
    let second = eval(
        "test://schema-locks/ensure",
        false,
        &format!(
            r#"
            const answer = database.ensureTable("world_items", {SCHEMA}).json();
            database.insert("world_items", JSON.stringify({{ item_id: "sword", owner: "me" }}));
            ({{
              created: answer.created,
              added: answer.columnsAdded,
              error: answer.error || null,
              rows: database.query("world_items").json().length,
            }})
            "#
        ),
    )
    .await;

    assert!(second.ok, "{:?}", second.outcome.error);
    let value = second.outcome.value.expect("a value");
    assert_eq!(value["error"], json!(null));
    assert_eq!(value["created"], json!(false));
    assert_eq!(value["added"], json!([]), "nothing was left to add");
    assert_eq!(value["rows"], json!(1));

    // A column added later converges onto the table already holding rows.
    let grown = eval(
        "test://schema-locks/ensure",
        false,
        r#"
        const answer = database.ensureTable("world_items", JSON.stringify({
            columns: [
              { name: "item_id", type: "text" },
              { name: "durability", type: "integer" },
            ],
        })).json();
        ({ added: answer.columnsAdded, error: answer.error || null,
           rowsKept: database.query("world_items").json().length })
        "#,
    )
    .await;

    assert!(grown.ok, "{:?}", grown.outcome.error);
    let value = grown.outcome.value.expect("a value");
    assert_eq!(value["error"], json!(null));
    assert_eq!(value["added"], json!(["durability"]));
    assert_eq!(value["rowsKept"], json!(1), "existing rows must survive");

    drop_table("test://schema-locks/ensure", "world_items").await;
}
