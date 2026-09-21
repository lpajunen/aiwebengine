//! A column added to a table a script has already queried.
//!
//! `ensureTable` lets a script grow a column at any moment, which is the whole
//! point of it — a deployment that adds a field should not have to migrate
//! anything by hand. But `SELECT *` and `RETURNING *` name every column, so
//! adding one changes the *result type* of those statements, and Postgres
//! refuses to run a cached plan whose result type has changed:
//!
//!     cached plan must not change result type
//!
//! sqlx prepares and caches a statement per connection by default, so the
//! statements a script ran before the change sit in the pool waiting to fail.
//! Which connection a call lands on decides whether it does, so the failure is
//! intermittent, survives for as long as the poisoned connections do, and
//! arrives as a query error — which most callers read as "no rows".
//!
//! That last step is what makes it worth a test rather than a shrug. A script
//! asking "is this run still going" gets "no such row" and concludes it is
//! not.

mod common;

use aiwebengine::db_schema_utils::ColumnType;
use aiwebengine::repository::{self, QueryOptions};
use common::{setup_env, test_mutex};
use std::collections::HashMap;

fn rows_for(uri: &str, table: &str) -> Result<Vec<serde_json::Value>, String> {
    let mut filters = HashMap::new();
    filters.insert("label".to_string(), serde_json::json!("one"));
    repository::query_table(uri, table, Some(&filters), &QueryOptions::default())
        .map_err(|e| e.to_string())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_query_survives_a_column_appearing_under_it() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "https://test.local/column-added.ts";
    repository::upsert_script(uri, "function init() {}").expect("script stored");
    let _ = repository::drop_script_table(uri, "probe");
    repository::create_script_table(uri, "probe").expect("table created");
    repository::add_column_to_script_table(uri, "probe", "label", ColumnType::Text, true, None)
        .expect("label column");

    let mut row = HashMap::new();
    row.insert("label".to_string(), serde_json::json!("one"));
    repository::insert_row(uri, "probe", &row).expect("row inserted");

    // Prepare and cache the statement on as many pooled connections as the
    // test can reach, which is what a script does by simply running.
    for _ in 0..20 {
        rows_for(uri, "probe").expect("the query works before the column is added");
    }

    // The change a script makes when it grows a field.
    repository::add_column_to_script_table(uri, "probe", "extra", ColumnType::Bigint, true, None)
        .expect("extra column");

    // Every one of these has to answer. A failure here is not a wrong answer,
    // it is an error the caller will read as an empty table.
    let mut failures = Vec::new();
    for attempt in 0..40 {
        match rows_for(uri, "probe") {
            Ok(rows) => assert_eq!(rows.len(), 1, "attempt {attempt} lost the row"),
            Err(e) => failures.push(format!("attempt {attempt}: {e}")),
        }
    }

    let _ = repository::drop_script_table(uri, "probe");
    assert!(
        failures.is_empty(),
        "{} of 40 queries failed after a column was added:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The same failure on the write paths, which is how it was first noticed:
/// a turn counter that silently stopped counting.
///
/// The statement has to be *the same statement* on both sides of the change.
/// sqlx caches by SQL text and these builders name only the columns they were
/// handed, so writing a different set of fields afterwards produces different
/// text, prepares it fresh, and hides the bug. A counter updating the same
/// column every turn is the shape that meets it.
#[tokio::test(flavor = "multi_thread")]
async fn a_repeated_write_survives_a_column_appearing_under_it() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "https://test.local/column-added-writes.ts";
    repository::upsert_script(uri, "function init() {}").expect("script stored");
    let _ = repository::drop_script_table(uri, "probe");
    repository::create_script_table(uri, "probe").expect("table created");
    repository::add_column_to_script_table(uri, "probe", "label", ColumnType::Text, true, None)
        .expect("label column");

    let mut seed = HashMap::new();
    seed.insert("label".to_string(), serde_json::json!("start"));
    let inserted = repository::insert_row(uri, "probe", &seed).expect("seed inserted");
    let id = inserted["id"].as_i64().expect("an id") as i32;

    // Warm the two statements, naming exactly the columns they will name again.
    for n in 0..20 {
        let mut row = HashMap::new();
        row.insert("label".to_string(), serde_json::json!(format!("warm-{n}")));
        repository::insert_row(uri, "probe", &row).expect("warm insert");
        repository::update_row(uri, "probe", id, &row).expect("warm update");
    }

    repository::add_column_to_script_table(uri, "probe", "extra", ColumnType::Bigint, true, None)
        .expect("extra column");

    let mut failures = Vec::new();
    for n in 0..40 {
        let mut row = HashMap::new();
        row.insert("label".to_string(), serde_json::json!(format!("after-{n}")));
        if let Err(e) = repository::insert_row(uri, "probe", &row) {
            failures.push(format!("insert {n}: {e}"));
        }
        if let Err(e) = repository::update_row(uri, "probe", id, &row) {
            failures.push(format!("update {n}: {e}"));
        }
    }

    let _ = repository::drop_script_table(uri, "probe");
    assert!(
        failures.is_empty(),
        "{} writes failed after a column was added:\n{}",
        failures.len(),
        failures
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
