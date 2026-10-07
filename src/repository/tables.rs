//! A script's own database tables: schema changes, queries and row writes.

use super::*;
use crate::db_schema_utils::{
    BindType, ColumnType, MAX_COLUMNS_PER_TABLE, MAX_TABLES_PER_SCRIPT,
    generate_physical_table_name, quote_identifier, validate_identifier,
};
use crate::error::{AppError, AppResult};
use crate::sql_dialect::dialect;
use chrono::{DateTime, Utc};
use sqlx::{PgConnection, PgPool, Row};
use std::collections::HashMap;
use tracing::{debug, error};

// ============================================================================
// Script Database Schema Introspection Types
// ============================================================================

/// Metadata about a script-owned table
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TableInfo {
    pub logical_name: String,
    pub physical_name: String,
    pub created_at: DateTime<Utc>,
}

/// Schema information for a table
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TableSchema {
    pub table_name: String,
    pub columns: Vec<ColumnInfo>,
}

/// One column a script wants a table to have.
#[derive(Debug, Clone)]
pub struct EnsuredColumn {
    pub name: String,
    pub column_type: crate::db_schema_utils::ColumnType,
    pub nullable: bool,
    pub default_value: Option<String>,
    /// For a reference column: the logical table it points at. The column is
    /// an integer holding that table's `id`, with a foreign key behind it.
    pub references: Option<String>,
}

/// Which way a query orders its rows.
///
/// An enum rather than a string because there are two answers and the caller
/// has to pick one of them. The string form used to fall through to ascending
/// for anything it did not recognise, so a misspelled `"descending"` sorted
/// the wrong way without saying so.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum OrderDirection {
    #[default]
    Ascending,
    Descending,
}

impl OrderDirection {
    /// Read a direction as a script would write it, or `None` if it is neither.
    pub fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_lowercase().as_str() {
            "asc" => Some(OrderDirection::Ascending),
            "desc" => Some(OrderDirection::Descending),
            _ => None,
        }
    }

    /// `ASC` and `DESC` are spelled the same by every backend, so this needs
    /// no dialect.
    pub(super) fn sql(self) -> &'static str {
        match self {
            OrderDirection::Ascending => "ASC",
            OrderDirection::Descending => "DESC",
        }
    }
}

/// How a query runs, beyond which rows it matches.
///
/// One value rather than four trailing parameters. A call site passing
/// `(None, None, None, true)` says nothing about what any of them are, and the
/// list is the part of a query most likely to grow.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QueryOptions {
    /// Most rows to return. `None` takes the default of 100; anything above
    /// the cap of 1000 is clamped to it.
    pub limit: Option<i64>,
    /// Column to sort by. `None` leaves the order to the database.
    pub order_by: Option<String>,
    /// Which way to sort, when `order_by` names a column.
    pub order_dir: OrderDirection,
    /// Hold the rows this query returns until the transaction ends, so a
    /// read-modify-write can rely on what it read.
    ///
    /// Only meaningful inside a transaction: a lock taken outside one is
    /// released the moment the statement finishes, which looks like a guard
    /// and is not one. Asking for it outside a transaction is refused rather
    /// than quietly ignored.
    pub for_update: bool,
}

/// The shape a script wants a table to be in, whatever shape it is in now.
#[derive(Debug, Clone, Default)]
pub struct TableSpec {
    pub columns: Vec<EnsuredColumn>,
    /// Column groups that must each carry a unique index — what `upsert` needs
    /// before it can use them as a conflict target.
    pub unique_indexes: Vec<Vec<String>>,
}

/// What converging a table to a [`TableSpec`] actually changed.
#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnsuredTable {
    /// Whether this call is the one that created the table.
    pub created: bool,
    /// Columns this call added. A column already present is not listed.
    pub columns_added: Vec<String>,
    /// Index column groups this call ensured. Postgres does not report whether
    /// `CREATE UNIQUE INDEX IF NOT EXISTS` created anything, so these are
    /// "present now", not "added now".
    pub unique_indexes_ensured: Vec<Vec<String>>,
}

/// Information about a table column
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String, // "INTEGER", "TEXT", "BOOLEAN", "TIMESTAMPTZ"
    pub nullable: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_value: Option<String>,
    #[serde(default)]
    pub is_primary_key: bool,
}

/// Foreign key relationship information
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ForeignKeyInfo {
    pub column_name: String,
    pub referenced_table_logical: String,
    pub referenced_table_physical: String,
    pub referenced_column: String,
}

/// Names each savepoint a [`ScopedConn`] brackets an operation with.
pub(super) static SCHEMA_SAVEPOINT_COUNTER: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// The connection a script's database operation runs on, and the scope that
/// undoes it if it fails.
///
/// Whatever opened it, the rule is the same: an operation that fails inside
/// the caller's transaction must not take the transaction with it. Postgres
/// aborts a transaction on any error, so without a savepoint one bad statement
/// discards every write made before it — a scheduled tick's unrelated work
/// included — and the script's `try`/`catch` catches an error it can no longer
/// do anything about.
///
/// Schema work must never run on a second connection while the caller holds a
/// transaction. `CREATE INDEX` takes SHARE and `ALTER TABLE` takes ACCESS
/// EXCLUSIVE, and both conflict with the ROW EXCLUSIVE the caller's own writes
/// already hold on the table. Postgres cannot see that as a deadlock — the
/// holder is not waiting on the database, it is waiting on the engine to finish
/// the request — so its detector never fires, the statement blocks until the
/// connection dies, and every later writer queues behind the pending strong
/// lock. A script calling `ensureSchema()` inside `transaction()` was enough to
/// wedge a table for every instance in the cluster.
///
/// Joining the caller's transaction makes that conflict impossible: a
/// connection never blocks on locks it already holds.
///
/// Inside a transaction the operation is bracketed by a savepoint, so a failure
/// — `table already exists`, an invalid column type — leaves the caller's
/// transaction usable instead of aborting it. That is what a script wrapping an
/// ensure-schema step in `try`/`catch` expects, and what running on a separate
/// connection used to give it for free.
///
/// Outside one, the operation gets a transaction of its own. These are
/// multi-statement units — `CREATE TABLE` plus the `script_tables` row that
/// records it — and running them in autocommit leaves a physical table with no
/// metadata behind whenever the second statement fails.
///
/// Every path must end at [`ScopedConn::finish`], which releases the savepoint
/// or commits, and undoes either one if the operation failed.
pub(super) enum ScopedConn<'a> {
    /// Bracketing the caller's transaction, which this must leave open.
    Savepoint {
        tx: &'a mut sqlx::Transaction<'static, sqlx::Postgres>,
        savepoint: String,
    },
    /// A transaction of this operation's own, to be committed or rolled back.
    Owned(sqlx::Transaction<'a, sqlx::Postgres>),
    /// A pooled connection in autocommit, for an operation that is one
    /// statement and has no caller's transaction to protect.
    Pooled(sqlx::pool::PoolConnection<sqlx::Postgres>),
}

impl<'a> ScopedConn<'a> {
    /// Opens the savepoint every scope shares when a transaction is active.
    pub(super) async fn savepoint_in(
        tx: &'a mut sqlx::Transaction<'static, sqlx::Postgres>,
    ) -> AppResult<Self> {
        let savepoint = format!(
            "aiwe_scope_{}",
            SCHEMA_SAVEPOINT_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        sqlx::query(sqlx::AssertSqlSafe(format!("SAVEPOINT {}", savepoint)))
            .execute(&mut **tx)
            .await
            .map_err(|e| schema_transaction_error("opening a savepoint", e))?;
        Ok(ScopedConn::Savepoint { tx, savepoint })
    }

    /// A connection for a schema operation.
    ///
    /// Outside a transaction the operation gets one of its own, because these
    /// are multi-statement units — `CREATE TABLE` plus the `script_tables` row
    /// that records it — and running them in autocommit leaves a physical
    /// table with no metadata behind whenever the second statement fails.
    pub(super) async fn for_schema(pool: &'a PgPool) -> AppResult<Self> {
        match crate::database::get_current_executor(pool) {
            crate::database::TransactionExecutor::Transaction(tx) => Self::savepoint_in(tx).await,
            crate::database::TransactionExecutor::Pool(pool) => {
                let tx = pool
                    .begin()
                    .await
                    .map_err(|e| schema_transaction_error("opening a schema transaction", e))?;
                Ok(ScopedConn::Owned(tx))
            }
        }
    }

    /// A connection for a schema operation on one named table, serialised
    /// against every other engine instance doing the same.
    ///
    /// The existence checks these operations start with are worthless
    /// concurrently: two handlers calling `ensureSchema()` on a cold cache both
    /// read "no such table" and both go on to create it. The loser gets
    /// Postgres's own `relation already exists` rather than the engine's
    /// answer, and between the two statements each of these operations makes
    /// there is room for worse — a physical table with no `script_tables` row
    /// to find it by.
    ///
    /// The advisory lock is keyed on the script and table, so two scripts, or
    /// one script's two tables, never wait on each other. It is held for the
    /// transaction rather than the savepoint, which means until the caller's
    /// transaction ends — the same scope the schema change itself commits in,
    /// and the only scope at which "did this table exist" stays true.
    pub(super) async fn for_schema_of(
        pool: &'a PgPool,
        script_uri: &str,
        logical_table_name: &str,
    ) -> AppResult<Self> {
        let mut scope = Self::for_schema(pool).await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1), hashtext($2))")
            .bind(script_uri)
            .bind(logical_table_name)
            .execute(scope.conn())
            .await
            .map_err(|e| schema_transaction_error("taking the schema lock", e))?;
        Ok(scope)
    }

    /// A connection for a single data statement — a row read or write.
    ///
    /// Inside a transaction it is bracketed like any other operation, which is
    /// what makes a failed write survivable: a duplicate key, a value the
    /// column will not take, a timestamp Postgres cannot parse. The script
    /// catches the error and the transaction it is standing in remains usable.
    /// The bracket costs two round trips, which is the price of not losing the
    /// rest of a transaction to one failed statement.
    ///
    /// Outside a transaction there is nothing to protect: a lone statement in
    /// autocommit is already its own unit, and wrapping it would only add
    /// round trips.
    pub(super) async fn for_statement(pool: &'a PgPool) -> AppResult<Self> {
        match crate::database::get_current_executor(pool) {
            crate::database::TransactionExecutor::Transaction(tx) => Self::savepoint_in(tx).await,
            crate::database::TransactionExecutor::Pool(pool) => {
                let conn = pool.acquire().await.map_err(|e| AppError::Database {
                    message: format!("Failed to acquire connection: {}", e),
                    source: None,
                })?;
                Ok(ScopedConn::Pooled(conn))
            }
        }
    }

    pub(super) fn conn(&mut self) -> &mut PgConnection {
        match self {
            ScopedConn::Savepoint { tx, .. } => tx,
            ScopedConn::Owned(tx) => tx,
            ScopedConn::Pooled(conn) => conn,
        }
    }

    /// Closes the bracket around `outcome` and returns it unchanged.
    ///
    /// The operation's own error is the one worth reporting, so undoing a
    /// failed operation is best effort — but recovering the caller's
    /// transaction is not optional, which is why the rollback runs before the
    /// error is handed back.
    pub(super) async fn finish<T>(self, outcome: AppResult<T>) -> AppResult<T> {
        match self {
            ScopedConn::Savepoint { tx, savepoint } => {
                let verb = if outcome.is_ok() {
                    "RELEASE SAVEPOINT"
                } else {
                    "ROLLBACK TO SAVEPOINT"
                };
                let closed = sqlx::query(sqlx::AssertSqlSafe(format!("{} {}", verb, savepoint)))
                    .execute(&mut **tx)
                    .await
                    .map_err(|e| schema_transaction_error("closing a savepoint", e));
                match outcome {
                    Ok(value) => closed.map(|_| value),
                    Err(operation_error) => Err(operation_error),
                }
            }
            ScopedConn::Pooled(_) => outcome,
            ScopedConn::Owned(tx) => match outcome {
                Ok(value) => {
                    tx.commit().await.map_err(|e| {
                        schema_transaction_error("committing a schema transaction", e)
                    })?;
                    Ok(value)
                }
                Err(operation_error) => {
                    if let Err(e) = tx.rollback().await {
                        error!("Database error rolling back a schema transaction: {}", e);
                    }
                    Err(operation_error)
                }
            },
        }
    }
}

pub(super) fn schema_transaction_error(what: &str, e: sqlx::Error) -> AppError {
    error!("Database error {}: {}", what, e);
    AppError::Database {
        message: format!("Database error: {}", e),
        source: None,
    }
}

/// Database-backed create script-owned table
pub(super) async fn db_create_script_table(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
) -> AppResult<String> {
    // Validate the logical table name
    validate_identifier(logical_table_name).map_err(|e| AppError::Validation {
        field: "table_name".to_string(),
        reason: e.to_string(),
    })?;

    // Check table limit for this script
    let table_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM script_tables WHERE script_uri = $1")
            .bind(script_uri)
            .fetch_one(&mut *conn)
            .await
            .map_err(|e| {
                error!("Database error counting tables for script: {}", e);
                AppError::Database {
                    message: format!("Database error: {}", e),
                    source: None,
                }
            })?;

    if table_count >= MAX_TABLES_PER_SCRIPT as i64 {
        return Err(AppError::Validation {
            field: "table_name".to_string(),
            reason: format!(
                "Script has reached maximum table limit of {}",
                MAX_TABLES_PER_SCRIPT
            ),
        });
    }

    // Generate physical table name, from the script's id: the URI is a name
    // and can change, the id cannot.
    let script_id: Option<String> =
        sqlx::query_scalar("SELECT id::text FROM scripts WHERE uri = $1")
            .bind(script_uri)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| {
                error!("Database error reading script id: {}", e);
                AppError::Database {
                    message: format!("Database error: {}", e),
                    source: None,
                }
            })?;
    let script_id = script_id.ok_or_else(|| AppError::Validation {
        field: "script".to_string(),
        reason: format!("Script not found: {}", script_uri),
    })?;
    let physical_table_name = generate_physical_table_name(&script_id, logical_table_name);

    // Check if table already exists for this script
    let exists: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM script_tables WHERE script_uri = $1 AND logical_table_name = $2)",
    )
    .bind(script_uri)
    .bind(logical_table_name)
    .fetch_one(&mut *conn)
    .await
    .map_err(|e| {
        error!("Database error checking table existence: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    if exists {
        return Err(AppError::Validation {
            field: "table_name".to_string(),
            reason: format!(
                "Table '{}' already exists for this script",
                logical_table_name
            ),
        });
    }

    // Create the physical table with id column
    let create_table_sql = format!(
        "CREATE TABLE {} ({})",
        quote_identifier(&physical_table_name),
        dialect().identity_primary_key()
    );

    sqlx::query(sqlx::AssertSqlSafe(create_table_sql.as_str()))
        .execute(&mut *conn)
        .await
        .map_err(|e| {
            error!("Database error creating table: {}", e);
            AppError::Database {
                message: format!("Failed to create table: {}", e),
                source: None,
            }
        })?;

    // Record the table in script_tables metadata
    let schema_json = serde_json::json!({
        "columns": [
            {
                "name": "id",
                "type": ColumnType::Integer.canonical(),
                "nullable": false,
                "primary_key": true
            }
        ]
    });

    sqlx::query(
        r#"
        INSERT INTO script_tables (script_uri, logical_table_name, physical_table_name, schema_json)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(script_uri)
    .bind(logical_table_name)
    .bind(&physical_table_name)
    .bind(schema_json)
    .execute(&mut *conn)
    .await
    .map_err(|e| {
        error!("Database error recording table metadata: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    debug!(
        "Created script table: {} -> {}",
        logical_table_name, physical_table_name
    );

    Ok(physical_table_name)
}

/// Brings a script-owned table to the shape `spec` describes.
///
/// Every solution ends up writing this by hand — create the table, add the
/// columns, add the indexes, catch and ignore the "already exists" from each —
/// and every hand-written version has the same two faults. It runs on the
/// request path, where its `ALTER TABLE`s meet whatever else is holding the
/// table; and it treats an error as success because the common error means
/// "already done", which hides the ones that mean something else.
///
/// Here it is one call: one advisory lock, one transaction, and a check before
/// each step instead of an exception after it. Doing nothing is the normal
/// outcome and costs one query.
pub(super) async fn db_ensure_script_table(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
    spec: &TableSpec,
) -> AppResult<EnsuredTable> {
    validate_identifier(logical_table_name).map_err(|e| AppError::Validation {
        field: "table_name".to_string(),
        reason: e.to_string(),
    })?;

    let mut outcome = EnsuredTable::default();

    let existing: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT schema_json FROM script_tables WHERE script_uri = $1 AND logical_table_name = $2",
    )
    .bind(script_uri)
    .bind(logical_table_name)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| {
        error!("Database error reading table metadata: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let mut present: Vec<String> = match &existing {
        Some(schema_json) => schema_json
            .get("columns")
            .and_then(|columns| columns.as_array())
            .map(|columns| {
                columns
                    .iter()
                    .filter_map(|column| column.get("name")?.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default(),
        None => {
            db_create_script_table(&mut *conn, script_uri, logical_table_name).await?;
            outcome.created = true;
            vec!["id".to_string()]
        }
    };

    for column in &spec.columns {
        if present.iter().any(|name| name == &column.name) {
            continue;
        }
        match column.references.as_deref() {
            Some(referenced) => {
                db_add_reference_column(
                    &mut *conn,
                    script_uri,
                    logical_table_name,
                    &column.name,
                    referenced,
                    column.nullable,
                )
                .await?
            }
            None => {
                db_add_column_to_script_table(
                    &mut *conn,
                    script_uri,
                    logical_table_name,
                    &column.name,
                    column.column_type,
                    column.nullable,
                    column.default_value.as_deref(),
                )
                .await?
            }
        }
        present.push(column.name.clone());
        outcome.columns_added.push(column.name.clone());
    }

    for columns in &spec.unique_indexes {
        db_add_unique_index(&mut *conn, script_uri, logical_table_name, columns).await?;
        outcome.unique_indexes_ensured.push(columns.clone());
    }

    Ok(outcome)
}

/// Database-backed add column to script-owned table
pub(super) async fn db_add_column_to_script_table(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
    column_name: &str,
    column_type: ColumnType,
    nullable: bool,
    default_value: Option<&str>,
) -> AppResult<()> {
    // Validate identifiers
    validate_identifier(logical_table_name).map_err(|e| AppError::Validation {
        field: "table_name".to_string(),
        reason: e.to_string(),
    })?;
    validate_identifier(column_name).map_err(|e| AppError::Validation {
        field: "column_name".to_string(),
        reason: e.to_string(),
    })?;

    // Get the physical table name
    let row: Option<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT physical_table_name, schema_json FROM script_tables WHERE script_uri = $1 AND logical_table_name = $2",
    )
    .bind(script_uri)
    .bind(logical_table_name)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| {
        error!("Database error fetching table metadata: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let (physical_table_name, mut schema_json) = row.ok_or_else(|| AppError::Validation {
        field: "table_name".to_string(),
        reason: format!("Table '{}' not found for this script", logical_table_name),
    })?;

    // Check column limit
    let column_count = schema_json
        .get("columns")
        .and_then(|c| c.as_array())
        .map(|c| c.len())
        .unwrap_or(0);

    if column_count >= MAX_COLUMNS_PER_TABLE {
        return Err(AppError::Validation {
            field: "column_name".to_string(),
            reason: format!(
                "Table has reached maximum column limit of {}",
                MAX_COLUMNS_PER_TABLE
            ),
        });
    }

    // Check if column already exists
    if let Some(columns) = schema_json.get("columns").and_then(|c| c.as_array())
        && columns
            .iter()
            .any(|col| col.get("name").and_then(|n| n.as_str()) == Some(column_name))
    {
        return Err(AppError::Validation {
            field: "column_name".to_string(),
            reason: format!(
                "Column '{}' already exists in table '{}'",
                column_name, logical_table_name
            ),
        });
    }

    // Build ALTER TABLE statement
    let mut alter_sql = format!(
        "ALTER TABLE {} ADD COLUMN {} {}",
        quote_identifier(&physical_table_name),
        quote_identifier(column_name),
        dialect().column_type(column_type)
    );

    if !nullable {
        alter_sql.push_str(" NOT NULL");
    }

    if let Some(default) = default_value {
        // Read into a value first, so what reaches the statement is this
        // backend's spelling of that value rather than the script's text.
        let parsed = column_type
            .parse_default(default)
            .map_err(|e| AppError::Validation {
                field: "default_value".to_string(),
                reason: e.to_string(),
            })?;
        alter_sql.push_str(&format!(" DEFAULT {}", dialect().render_default(&parsed)));
    }

    // Execute the ALTER TABLE
    sqlx::query(sqlx::AssertSqlSafe(alter_sql.as_str()))
        .execute(&mut *conn)
        .await
        .map_err(|e| {
            error!("Database error adding column: {}", e);
            AppError::Database {
                message: format!("Failed to add column: {}", e),
                source: None,
            }
        })?;

    // Update schema_json metadata
    if let Some(columns) = schema_json
        .get_mut("columns")
        .and_then(|c| c.as_array_mut())
    {
        columns.push(serde_json::json!({
            "name": column_name,
            "type": column_type.canonical(),
            "nullable": nullable,
            "default": default_value,
        }));
    }

    sqlx::query("UPDATE script_tables SET schema_json = $1, updated_at = NOW() WHERE script_uri = $2 AND logical_table_name = $3")
        .bind(schema_json)
        .bind(script_uri)
        .bind(logical_table_name)
        .execute(&mut *conn)
        .await
        .map_err(|e| {
            error!("Database error updating schema metadata: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;

    debug!(
        "Added column {} to table {}: {} {}",
        column_name,
        logical_table_name,
        column_type.canonical(),
        if nullable { "NULL" } else { "NOT NULL" }
    );

    Ok(())
}

/// Database-backed add reference column (creates INTEGER column with FK constraint)
pub(super) async fn db_add_reference_column(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
    column_name: &str,
    referenced_logical_table_name: &str,
    nullable: bool,
) -> AppResult<()> {
    // Validate identifiers
    validate_identifier(logical_table_name).map_err(|e| AppError::Validation {
        field: "table_name".to_string(),
        reason: e.to_string(),
    })?;
    validate_identifier(column_name).map_err(|e| AppError::Validation {
        field: "column_name".to_string(),
        reason: e.to_string(),
    })?;
    validate_identifier(referenced_logical_table_name).map_err(|e| AppError::Validation {
        field: "referenced_table_name".to_string(),
        reason: e.to_string(),
    })?;

    // First, add the integer column
    db_add_column_to_script_table(
        &mut *conn,
        script_uri,
        logical_table_name,
        column_name,
        ColumnType::Integer,
        nullable,
        None,
    )
    .await?;

    // Get physical table names for FK constraint
    let source_table: String = sqlx::query_scalar(
        "SELECT physical_table_name FROM script_tables WHERE script_uri = $1 AND logical_table_name = $2",
    )
    .bind(script_uri)
    .bind(logical_table_name)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| {
        error!("Database error fetching source table: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?
    .ok_or_else(|| AppError::Validation {
        field: "table_name".to_string(),
        reason: format!("Table '{}' not found for this script", logical_table_name),
    })?;

    let referenced_table: String = sqlx::query_scalar(
        "SELECT physical_table_name FROM script_tables WHERE script_uri = $1 AND logical_table_name = $2",
    )
    .bind(script_uri)
    .bind(referenced_logical_table_name)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| {
        error!("Database error fetching referenced table: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?
    .ok_or_else(|| AppError::Validation {
        field: "referenced_table_name".to_string(),
        reason: format!(
            "Referenced table '{}' not found for this script",
            referenced_logical_table_name
        ),
    })?;

    // Create the foreign key constraint
    let constraint_name = format!("fk_{}_{}", logical_table_name.replace("_", ""), column_name);
    let alter_sql = format!(
        "ALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} (id)",
        quote_identifier(&source_table),
        quote_identifier(&constraint_name),
        quote_identifier(column_name),
        quote_identifier(&referenced_table)
    );

    sqlx::query(sqlx::AssertSqlSafe(alter_sql.as_str()))
        .execute(&mut *conn)
        .await
        .map_err(|e| {
            error!("Database error creating foreign key: {}", e);
            AppError::Database {
                message: format!("Failed to create foreign key: {}", e),
                source: None,
            }
        })?;

    debug!(
        "Created reference column: {}.{} -> {}.id (nullable: {})",
        logical_table_name, column_name, referenced_logical_table_name, nullable
    );

    Ok(())
}

/// Database-backed drop column from script-owned table
pub(super) async fn db_drop_column(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
    column_name: &str,
) -> AppResult<bool> {
    // Validate identifiers
    validate_identifier(logical_table_name).map_err(|e| AppError::Validation {
        field: "table_name".to_string(),
        reason: e.to_string(),
    })?;
    validate_identifier(column_name).map_err(|e| AppError::Validation {
        field: "column_name".to_string(),
        reason: e.to_string(),
    })?;

    // Get the physical table name and schema
    let row: Option<(String, serde_json::Value)> = sqlx::query_as(
        "SELECT physical_table_name, schema_json FROM script_tables WHERE script_uri = $1 AND logical_table_name = $2",
    )
    .bind(script_uri)
    .bind(logical_table_name)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| {
        error!("Database error fetching table metadata: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let (physical_table_name, mut schema_json) = row.ok_or_else(|| AppError::Validation {
        field: "table_name".to_string(),
        reason: format!("Table '{}' not found for this script", logical_table_name),
    })?;

    // Check if column exists in schema
    let column_exists = schema_json
        .get("columns")
        .and_then(|c| c.as_array())
        .map(|columns| {
            columns
                .iter()
                .any(|col| col.get("name").and_then(|n| n.as_str()) == Some(column_name))
        })
        .unwrap_or(false);

    if !column_exists {
        return Ok(false);
    }

    // Don't allow dropping the id column
    if column_name == "id" {
        return Err(AppError::Validation {
            field: "column_name".to_string(),
            reason: "Cannot drop the 'id' column".to_string(),
        });
    }

    // Drop the column
    let drop_sql = format!(
        "ALTER TABLE {} DROP COLUMN {}",
        quote_identifier(&physical_table_name),
        quote_identifier(column_name)
    );

    sqlx::query(sqlx::AssertSqlSafe(drop_sql.as_str()))
        .execute(&mut *conn)
        .await
        .map_err(|e| {
            error!("Database error dropping column: {}", e);
            AppError::Database {
                message: format!("Failed to drop column: {}", e),
                source: None,
            }
        })?;

    // Update schema_json metadata
    if let Some(columns) = schema_json
        .get_mut("columns")
        .and_then(|c| c.as_array_mut())
    {
        columns.retain(|col| col.get("name").and_then(|n| n.as_str()) != Some(column_name));
    }

    sqlx::query(
        "UPDATE script_tables SET schema_json = $1, updated_at = NOW() WHERE script_uri = $2 AND logical_table_name = $3",
    )
    .bind(schema_json)
    .bind(script_uri)
    .bind(logical_table_name)
    .execute(&mut *conn)
    .await
    .map_err(|e| {
        error!("Database error updating schema metadata: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    debug!(
        "Dropped column {} from table {}",
        column_name, logical_table_name
    );

    Ok(true)
}

/// Database-backed drop script-owned table
pub(super) async fn db_drop_script_table(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
) -> AppResult<bool> {
    // Validate identifier
    validate_identifier(logical_table_name).map_err(|e| AppError::Validation {
        field: "table_name".to_string(),
        reason: e.to_string(),
    })?;

    // Get the physical table name
    let physical_table_name: Option<String> = sqlx::query_scalar(
        "SELECT physical_table_name FROM script_tables WHERE script_uri = $1 AND logical_table_name = $2",
    )
    .bind(script_uri)
    .bind(logical_table_name)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| {
        error!("Database error fetching table metadata: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    if let Some(physical_name) = physical_table_name {
        // Drop the physical table
        let drop_sql = format!(
            "DROP TABLE IF EXISTS {} CASCADE",
            quote_identifier(&physical_name)
        );

        sqlx::query(sqlx::AssertSqlSafe(drop_sql.as_str()))
            .execute(&mut *conn)
            .await
            .map_err(|e| {
                error!("Database error dropping table: {}", e);
                AppError::Database {
                    message: format!("Failed to drop table: {}", e),
                    source: None,
                }
            })?;

        // Remove from script_tables metadata (will cascade due to FK)
        sqlx::query("DELETE FROM script_tables WHERE script_uri = $1 AND logical_table_name = $2")
            .bind(script_uri)
            .bind(logical_table_name)
            .execute(&mut *conn)
            .await
            .map_err(|e| {
                error!("Database error removing table metadata: {}", e);
                AppError::Database {
                    message: format!("Database error: {}", e),
                    source: None,
                }
            })?;

        debug!(
            "Dropped script table: {} ({})",
            logical_table_name, physical_name
        );
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Database-backed drop all tables for a script
pub(super) async fn db_drop_all_script_tables(
    conn: &mut PgConnection,
    script_uri: &str,
) -> AppResult<usize> {
    // Get all tables for this script
    let tables: Vec<String> =
        sqlx::query_scalar("SELECT physical_table_name FROM script_tables WHERE script_uri = $1")
            .bind(script_uri)
            .fetch_all(&mut *conn)
            .await
            .map_err(|e| {
                error!("Database error fetching script tables: {}", e);
                AppError::Database {
                    message: format!("Database error: {}", e),
                    source: None,
                }
            })?;

    let count = tables.len();

    // Drop each table
    for physical_name in tables {
        let drop_sql = format!(
            "DROP TABLE IF EXISTS {} CASCADE",
            quote_identifier(&physical_name)
        );

        sqlx::query(sqlx::AssertSqlSafe(drop_sql.as_str()))
            .execute(&mut *conn)
            .await
            .map_err(|e| {
                error!("Database error dropping table {}: {}", physical_name, e);
                AppError::Database {
                    message: format!("Failed to drop table: {}", e),
                    source: None,
                }
            })?;
    }

    // Delete metadata entries (script_uri FK will auto-delete on script deletion)
    sqlx::query("DELETE FROM script_tables WHERE script_uri = $1")
        .bind(script_uri)
        .execute(&mut *conn)
        .await
        .map_err(|e| {
            error!("Database error removing table metadata: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;

    if count > 0 {
        debug!("Dropped {} tables for script {}", count, script_uri);
    }

    Ok(count)
}

// ============================================================================
// Script Database Schema Introspection Functions
// ============================================================================

/// List all tables owned by a script
pub(super) async fn db_list_script_tables(
    conn: &mut PgConnection,
    script_uri: &str,
) -> AppResult<Vec<TableInfo>> {
    let rows = sqlx::query!(
        r#"
        SELECT logical_table_name, physical_table_name, created_at
        FROM script_tables
        WHERE script_uri = $1
        ORDER BY logical_table_name
        "#,
        script_uri
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| {
        error!("Database error listing script tables: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    Ok(rows
        .into_iter()
        .map(|row| TableInfo {
            logical_name: row.logical_table_name,
            physical_name: row.physical_table_name,
            created_at: row.created_at,
        })
        .collect())
}

/// Get detailed schema information for a specific table
pub(super) async fn db_get_table_schema(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
) -> AppResult<TableSchema> {
    // Fetch schema_json from script_tables
    let schema_json: Option<serde_json::Value> = sqlx::query_scalar(
        "SELECT schema_json FROM script_tables WHERE script_uri = $1 AND logical_table_name = $2",
    )
    .bind(script_uri)
    .bind(logical_table_name)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| {
        error!("Database error fetching table schema: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let schema_json = schema_json.ok_or_else(|| AppError::Validation {
        field: "table_name".to_string(),
        reason: format!("Table '{}' not found for this script", logical_table_name),
    })?;

    // Parse columns from schema_json
    let columns_array = schema_json
        .get("columns")
        .and_then(|c| c.as_array())
        .ok_or_else(|| AppError::Validation {
            field: "schema_json".to_string(),
            reason: "Invalid schema format: missing columns array".to_string(),
        })?;

    let mut columns = Vec::new();
    for col in columns_array {
        let name = col
            .get("name")
            .and_then(|n| n.as_str())
            .ok_or_else(|| AppError::Validation {
                field: "column".to_string(),
                reason: "Column missing name".to_string(),
            })?
            .to_string();

        let data_type = col
            .get("type")
            .and_then(|t| t.as_str())
            .ok_or_else(|| AppError::Validation {
                field: "column".to_string(),
                reason: format!("Column '{}' missing type", name),
            })?
            .to_string();

        let nullable = col
            .get("nullable")
            .and_then(|n| n.as_bool())
            .unwrap_or(true);

        let default_value = col.get("default").map(|d| {
            if let Some(s) = d.as_str() {
                s.to_string()
            } else {
                d.to_string()
            }
        });

        let is_primary_key = col
            .get("primary_key")
            .and_then(|p| p.as_bool())
            .unwrap_or(false);

        columns.push(ColumnInfo {
            name,
            data_type,
            nullable,
            default_value,
            is_primary_key,
        });
    }

    Ok(TableSchema {
        table_name: logical_table_name.to_string(),
        columns,
    })
}

/// Get foreign key relationships for a table
pub(super) async fn db_get_foreign_keys(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
) -> AppResult<Vec<ForeignKeyInfo>> {
    // Get the physical table name first
    let physical_table_name: Option<String> = sqlx::query_scalar(
        "SELECT physical_table_name FROM script_tables WHERE script_uri = $1 AND logical_table_name = $2",
    )
    .bind(script_uri)
    .bind(logical_table_name)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| {
        error!("Database error fetching table metadata: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let physical_table_name = physical_table_name.ok_or_else(|| AppError::Validation {
        field: "table_name".to_string(),
        reason: format!("Table '{}' not found for this script", logical_table_name),
    })?;

    // Query PostgreSQL information_schema for foreign keys
    let rows = sqlx::query!(
        r#"
        SELECT
            kcu.column_name,
            ccu.table_name AS referenced_table_physical,
            ccu.column_name AS referenced_column
        FROM information_schema.table_constraints AS tc
        JOIN information_schema.key_column_usage AS kcu
            ON tc.constraint_name = kcu.constraint_name
            AND tc.table_schema = kcu.table_schema
        JOIN information_schema.constraint_column_usage AS ccu
            ON ccu.constraint_name = tc.constraint_name
            AND ccu.table_schema = tc.table_schema
        WHERE tc.constraint_type = 'FOREIGN KEY'
            AND tc.table_name = $1
        "#,
        &physical_table_name
    )
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| {
        error!("Database error fetching foreign keys: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let mut foreign_keys = Vec::new();

    for row in rows {
        // Look up the logical table name for the referenced table
        let referenced_table_logical: Option<String> = sqlx::query_scalar(
            "SELECT logical_table_name FROM script_tables WHERE script_uri = $1 AND physical_table_name = $2",
        )
        .bind(script_uri)
        .bind(&row.referenced_table_physical)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| {
            error!("Database error fetching referenced table name: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;

        // Only include FK if it references another table owned by the same script
        if let Some(referenced_logical) = referenced_table_logical
            && let Some(column_name) = row.column_name
        {
            foreign_keys.push(ForeignKeyInfo {
                column_name,
                referenced_table_logical: referenced_logical,
                referenced_table_physical: row.referenced_table_physical.unwrap_or_default(),
                referenced_column: row.referenced_column.unwrap_or_else(|| "id".to_string()),
            });
        }
    }

    Ok(foreign_keys)
}

// ============================================================================
// Script Database Data Access Functions
// ============================================================================

/// Supported comparison operators for query filters.
pub(super) enum FilterOp {
    Eq,
    Gt,
    Gte,
    Lt,
    Lte,
    Ne,
}

/// A single resolved filter condition (column, operator, value).
pub(super) struct FilterCondition {
    pub(super) column: String,
    pub(super) op: FilterOp,
    pub(super) value: serde_json::Value,
}

/// Parse a filter map into a flat list of `FilterCondition`s.
///
/// Supports two forms:
/// - Equality:  `{ "col": value }`
/// - Operators: `{ "col": { "$gt": v, "$lt": w, ... } }`
///
/// Allowed operators: `$gt`, `$gte`, `$lt`, `$lte`, `$ne`.
pub(super) fn parse_filter_conditions(
    filters: &HashMap<String, serde_json::Value>,
) -> AppResult<Vec<FilterCondition>> {
    let mut conditions = Vec::new();
    for (column, value) in filters {
        validate_identifier(column).map_err(|e| AppError::Validation {
            field: "filter_column".to_string(),
            reason: e.to_string(),
        })?;
        match value {
            serde_json::Value::Object(ops) => {
                for (op_key, op_val) in ops {
                    let op = match op_key.as_str() {
                        "$gt" => FilterOp::Gt,
                        "$gte" => FilterOp::Gte,
                        "$lt" => FilterOp::Lt,
                        "$lte" => FilterOp::Lte,
                        "$ne" => FilterOp::Ne,
                        other => {
                            return Err(AppError::Validation {
                                field: "filter_operator".to_string(),
                                reason: format!("Unknown filter operator: {}", other),
                            });
                        }
                    };
                    conditions.push(FilterCondition {
                        column: column.clone(),
                        op,
                        value: op_val.clone(),
                    });
                }
            }
            _ => {
                conditions.push(FilterCondition {
                    column: column.clone(),
                    op: FilterOp::Eq,
                    value: value.clone(),
                });
            }
        }
    }
    Ok(conditions)
}

/// The columns of a script-owned table, with the physical name to address it by.
///
/// Loaded in the one query that used to fetch the physical name alone, so
/// typing a statement's parameters costs no extra round trip.
pub(super) struct TableColumns {
    pub(super) physical_name: String,
    /// Column name to declared type. Empty for a table that records no schema.
    pub(super) declared: HashMap<String, BindType>,
}

impl TableColumns {
    pub(super) async fn load(
        conn: &mut PgConnection,
        script_uri: &str,
        logical_table_name: &str,
    ) -> AppResult<Self> {
        let row: Option<(String, Option<serde_json::Value>)> = sqlx::query_as(
            "SELECT physical_table_name, schema_json FROM script_tables WHERE script_uri = $1 AND logical_table_name = $2",
        )
        .bind(script_uri)
        .bind(logical_table_name)
        .fetch_optional(&mut *conn)
        .await
        .map_err(|e| {
            error!("Database error fetching table metadata: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;

        let (physical_name, schema_json) = row.ok_or_else(|| AppError::Validation {
            field: "table_name".to_string(),
            reason: format!("Table '{}' not found for this script", logical_table_name),
        })?;

        let mut declared = HashMap::new();
        if let Some(columns) = schema_json
            .as_ref()
            .and_then(|s| s.get("columns"))
            .and_then(|c| c.as_array())
        {
            for column in columns {
                if let Some(name) = column.get("name").and_then(|n| n.as_str())
                    && let Some(bind_type) = column
                        .get("type")
                        .and_then(|t| t.as_str())
                        .and_then(BindType::from_declared)
                {
                    declared.insert(name.to_string(), bind_type);
                }
            }
        }

        Ok(Self {
            physical_name,
            declared,
        })
    }

    /// The type `column` was declared as, if the table records one for it.
    pub(super) fn declared_type(&self, column: &str) -> Option<BindType> {
        self.declared.get(column).copied()
    }

    /// The type `column` must be bound as, falling back to the value's shape.
    pub(super) fn bind_type(&self, column: &str, value: &serde_json::Value) -> BindType {
        self.declared
            .get(column)
            .copied()
            .unwrap_or_else(|| BindType::infer(value))
    }
}

/// A value to bind, with the type resolved from the column it is going into.
pub(super) struct BoundValue<'a> {
    pub(super) column: &'a str,
    pub(super) value: &'a serde_json::Value,
    pub(super) bind_type: BindType,
}

impl<'a> BoundValue<'a> {
    pub(super) fn new(
        columns: &TableColumns,
        column: &'a str,
        value: &'a serde_json::Value,
    ) -> Self {
        Self {
            column,
            value,
            bind_type: columns.bind_type(column, value),
        }
    }

    /// The placeholder for this value, spelled by the backend in use.
    pub(super) fn placeholder(&self, position: usize) -> String {
        dialect().placeholder(position, self.bind_type)
    }
}

/// Resolve a script's `{column: value}` map into a deterministic binding order.
///
/// Sorted rather than left in hash order: the column list is part of the SQL
/// text, and the statement cache is keyed on that text, so an unstable order
/// would scatter one logical statement across as many cached statements as the
/// map has orderings.
pub(super) fn ordered_bindings<'a>(
    columns: &TableColumns,
    data: &'a HashMap<String, serde_json::Value>,
) -> AppResult<Vec<BoundValue<'a>>> {
    let mut names: Vec<&'a String> = data.keys().collect();
    names.sort();

    let mut bound = Vec::with_capacity(names.len());
    for name in names {
        validate_identifier(name).map_err(|e| AppError::Validation {
            field: "column_name".to_string(),
            reason: e.to_string(),
        })?;
        bound.push(BoundValue::new(columns, name, &data[name]));
    }
    Ok(bound)
}

/// Reject a value the column's declared type cannot hold.
pub(super) fn value_rejected(column: &str, bind_type: BindType, got: &str) -> AppError {
    AppError::Validation {
        field: column.to_string(),
        reason: format!(
            "Column '{}' is {}; got {}",
            column,
            bind_type.describe(),
            got
        ),
    }
}

/// A JSON number as a whole number, or an error naming what was wrong with it.
///
/// A script computing `1.57` for an integer column has a bug, and rounding it
/// to `2` on the script's behalf hides that bug behind a value it never asked
/// to store. `2.0` is a different matter: JavaScript has one numeric type, so
/// a whole number arrives as a float whenever it has been through arithmetic,
/// and refusing it would refuse ordinary integer work.
pub(super) fn as_whole_number(
    column: &str,
    bind_type: BindType,
    n: &serde_json::Number,
) -> AppResult<i64> {
    if let Some(i) = n.as_i64() {
        return Ok(i);
    }
    match n.as_f64() {
        Some(f) if f.fract() == 0.0 && f >= i64::MIN as f64 && f <= i64::MAX as f64 => Ok(f as i64),
        Some(f) if f.fract() != 0.0 => Err(value_rejected(
            column,
            bind_type,
            &format!(
                "{} — a whole number is required, or a FLOAT column to keep the fraction",
                f
            ),
        )),
        _ => Err(value_rejected(
            column,
            bind_type,
            &format!("{} — out of range", n),
        )),
    }
}

/// Bind one resolved value to a sqlx query as the type its column declared.
///
/// Every mismatch is reported here, as a validation error naming the column,
/// rather than being sent to Postgres to fail against. That matters inside a
/// transaction: a statement that never runs cannot abort one.
pub(super) fn bind_value<'q>(
    query: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    bound: &BoundValue<'q>,
) -> AppResult<sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>> {
    use serde_json::Value;

    let column = bound.column;
    let bind_type = bound.bind_type;

    match (bind_type, bound.value) {
        (BindType::Int4, Value::Null) => Ok(query.bind(Option::<i32>::None)),
        (BindType::Int4, Value::Number(n)) => {
            let whole = as_whole_number(column, bind_type, n)?;
            let narrowed = i32::try_from(whole).map_err(|_| {
                value_rejected(column, bind_type, &format!("{} — out of range", whole))
            })?;
            Ok(query.bind(narrowed))
        }

        (BindType::Int8, Value::Null) => Ok(query.bind(Option::<i64>::None)),
        (BindType::Int8, Value::Number(n)) => {
            Ok(query.bind(as_whole_number(column, bind_type, n)?))
        }

        (BindType::Float8, Value::Null) => Ok(query.bind(Option::<f64>::None)),
        (BindType::Float8, Value::Number(n)) => {
            // A `serde_json` number is finite by construction, so there is no
            // NaN or infinity to screen out here.
            let f = n.as_f64().ok_or_else(|| {
                value_rejected(column, bind_type, &format!("{} — out of range", n))
            })?;
            Ok(query.bind(f))
        }

        (BindType::Text, Value::Null) => Ok(query.bind(Option::<String>::None)),
        (BindType::Text, Value::String(s)) => Ok(query.bind(s.as_str())),

        (BindType::Bool, Value::Null) => Ok(query.bind(Option::<bool>::None)),
        (BindType::Bool, Value::Bool(b)) => Ok(query.bind(*b)),

        // Postgres parses the timestamp out of the cast placeholder, which
        // accepts the ISO 8601 strings scripts get from `toISOString()`.
        (BindType::Timestamptz, Value::Null) => Ok(query.bind(Option::<String>::None)),
        (BindType::Timestamptz, Value::String(s)) => Ok(query.bind(s.as_str())),

        (_, Value::Array(_)) | (_, Value::Object(_)) => Err(value_rejected(
            column,
            bind_type,
            "an array or object — only scalar values can be stored",
        )),
        (_, Value::Number(n)) => Err(value_rejected(column, bind_type, &format!("{}", n))),
        (_, Value::String(_)) => Err(value_rejected(column, bind_type, "a string")),
        (_, Value::Bool(b)) => Err(value_rejected(column, bind_type, &format!("{}", b))),
    }
}

/// Convert a sqlx `PgRow` to a `serde_json::Value::Object`, decoding each
/// column as the type the script declared it.
///
/// Reading a value by trying each Rust type in turn until one decodes is only
/// safe where the wire format carries the column's own type. Postgres does, so
/// a `bool` column fails the integer attempt and falls through to the right
/// one. A backend that stores a boolean as 0 or 1 does not: the integer
/// attempt succeeds first, and `true` comes back to the script as `1`. Nothing
/// errors, nothing is logged, and the script sees a different value than it
/// wrote.
///
/// Consulting the declared type makes the read path the mirror of the write
/// path, which already binds by declared type. Sniffing survives only as the
/// fallback for columns nothing declared — lease tables, which record no
/// schema, and tables predating the metadata.
pub(super) fn row_to_json(
    row: &sqlx::postgres::PgRow,
    columns: &TableColumns,
) -> serde_json::Value {
    use sqlx::Column;

    let mut obj = serde_json::Map::new();
    for (idx, column) in row.columns().iter().enumerate() {
        let name = column.name();

        let value = match columns.declared_type(name) {
            // A declared type that will not decode means the recorded schema
            // and the physical column have drifted apart. Falling back leaves
            // the script with the value rather than a null, which is the same
            // thing it got before any of this was declared.
            Some(bind_type) => {
                decode_as(row, idx, bind_type).unwrap_or_else(|| decode_by_shape(row, idx))
            }
            None => decode_by_shape(row, idx),
        };

        obj.insert(name.to_string(), value);
    }
    serde_json::Value::Object(obj)
}

/// Decode one column as `bind_type`, or `None` if it does not hold that type.
///
/// A SQL `NULL` decodes as `Some(Value::Null)` — it is a value of the column's
/// type, not a failure to decode one.
pub(super) fn decode_as(
    row: &sqlx::postgres::PgRow,
    idx: usize,
    bind_type: BindType,
) -> Option<serde_json::Value> {
    fn number(value: impl Into<serde_json::Number>) -> serde_json::Value {
        serde_json::Value::Number(value.into())
    }

    let value = match bind_type {
        BindType::Int4 => row
            .try_get::<Option<i32>, _>(idx)
            .ok()?
            .map_or_else(|| serde_json::Value::Null, number),
        BindType::Int8 => row
            .try_get::<Option<i64>, _>(idx)
            .ok()?
            .map_or_else(|| serde_json::Value::Null, number),
        BindType::Float8 => row.try_get::<Option<f64>, _>(idx).ok()?.map_or_else(
            || serde_json::Value::Null,
            // JSON has no NaN or infinity. A column holding one has no
            // representation here, and null is the only honest answer.
            |v| {
                serde_json::Number::from_f64(v)
                    .map_or(serde_json::Value::Null, serde_json::Value::Number)
            },
        ),
        BindType::Text => row
            .try_get::<Option<String>, _>(idx)
            .ok()?
            .map_or(serde_json::Value::Null, serde_json::Value::String),
        BindType::Bool => row
            .try_get::<Option<bool>, _>(idx)
            .ok()?
            .map_or(serde_json::Value::Null, serde_json::Value::Bool),
        BindType::Timestamptz => row
            .try_get::<Option<DateTime<Utc>>, _>(idx)
            .ok()?
            .map_or(serde_json::Value::Null, |v| {
                serde_json::Value::String(v.to_rfc3339())
            }),
    };

    Some(value)
}

/// Decode one column by trying each type in turn.
///
/// Only for columns the recorded schema does not describe. Correct on Postgres,
/// where the column's type decides which attempt succeeds; see [`row_to_json`]
/// for why it cannot be the general case.
pub(super) fn decode_by_shape(row: &sqlx::postgres::PgRow, idx: usize) -> serde_json::Value {
    if let Ok(v) = row.try_get::<i64, _>(idx) {
        serde_json::Value::Number(v.into())
    } else if let Ok(v) = row.try_get::<i32, _>(idx) {
        serde_json::Value::Number(v.into())
    } else if let Ok(v) = row.try_get::<String, _>(idx) {
        serde_json::Value::String(v)
    } else if let Ok(v) = row.try_get::<f64, _>(idx) {
        // Ahead of the null fallback rather than after it: a `float8`
        // column matches none of the arms above, so without this one every
        // float a script stored would read back as null.
        serde_json::Number::from_f64(v).map_or(serde_json::Value::Null, serde_json::Value::Number)
    } else if let Ok(v) = row.try_get::<bool, _>(idx) {
        serde_json::Value::Bool(v)
    } else if let Ok(v) = row.try_get::<DateTime<Utc>, _>(idx) {
        serde_json::Value::String(v.to_rfc3339())
    } else {
        serde_json::Value::Null
    }
}

/// Look up the physical table name for a script-owned logical table.
pub(super) async fn get_physical_table_name(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
) -> AppResult<String> {
    let physical_table_name: Option<String> = sqlx::query_scalar(
        "SELECT physical_table_name FROM script_tables WHERE script_uri = $1 AND logical_table_name = $2",
    )
    .bind(script_uri)
    .bind(logical_table_name)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| {
        error!("Database error fetching table metadata: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    physical_table_name.ok_or_else(|| AppError::Validation {
        field: "table_name".to_string(),
        reason: format!("Table '{}' not found for this script", logical_table_name),
    })
}

/// Query rows from a script-owned table.
///
/// `filters` supports equality (`{"col": value}`) and comparison operators
/// (`{"col": {"$gt": v}}`). Supported operators: `$gt`, `$gte`, `$lt`,
/// `$lte`, `$ne`.
///
/// `order_by` must be a valid column identifier; `order_dir` is `"asc"` or
/// `"desc"` (defaults to `"asc"`).
pub(super) async fn db_query_table(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
    filters: Option<&HashMap<String, serde_json::Value>>,
    options: &QueryOptions,
) -> AppResult<Vec<serde_json::Value>> {
    if options.for_update && !crate::database::get_current_transaction_active() {
        return Err(AppError::Validation {
            field: "forUpdate".to_string(),
            reason: "forUpdate needs an open transaction to hold the rows it locks — make \
                     the read and the write inside database.transaction(fn). Outside a \
                     transaction the lock is released as soon as the query returns, which \
                     would read like a guard without being one."
                .to_string(),
        });
    }

    let columns = TableColumns::load(conn, script_uri, logical_table_name).await?;

    // Parse filter conditions (supports equality and range operators)
    let conditions = if let Some(f) = filters {
        parse_filter_conditions(f)?
    } else {
        Vec::new()
    };

    let bound: Vec<BoundValue<'_>> = conditions
        .iter()
        .map(|c| BoundValue::new(&columns, &c.column, &c.value))
        .collect();

    // Build WHERE clause
    let mut sql = format!("SELECT * FROM {}", quote_identifier(&columns.physical_name));
    let mut param_count = 0usize;

    if !conditions.is_empty() {
        let clauses: Vec<String> = conditions
            .iter()
            .zip(&bound)
            .map(|(c, b)| {
                param_count += 1;
                let op_str = match c.op {
                    FilterOp::Eq => "=",
                    FilterOp::Gt => ">",
                    FilterOp::Gte => ">=",
                    FilterOp::Lt => "<",
                    FilterOp::Lte => "<=",
                    FilterOp::Ne => "!=",
                };
                format!(
                    "{} {} {}",
                    quote_identifier(&c.column),
                    op_str,
                    b.placeholder(param_count)
                )
            })
            .collect();
        sql.push_str(&format!(" WHERE {}", clauses.join(" AND ")));
    }

    // ORDER BY
    if let Some(order_col) = options.order_by.as_deref() {
        validate_identifier(order_col).map_err(|e| AppError::Validation {
            field: "order_by".to_string(),
            reason: e.to_string(),
        })?;
        sql.push_str(&format!(
            " ORDER BY {} {}",
            quote_identifier(order_col),
            options.order_dir.sql()
        ));
    }

    // LIMIT (default 100, max 1000)
    let limit_val = options
        .limit
        .unwrap_or(DEFAULT_QUERY_LIMIT)
        .min(MAX_QUERY_LIMIT);
    param_count += 1;
    sql.push_str(&format!(" LIMIT ${}::int8", param_count));

    // Last, after LIMIT: the clause applies to the rows the query settles on.
    if options.for_update
        && let Some(clause) = dialect().row_lock_clause()
    {
        sql.push(' ');
        sql.push_str(clause);
    }

    // Bind parameters
    // Not cached, deliberately. A script's table grows a column whenever the
    // script says so, and this statement names every column — `SELECT *`,
    // `RETURNING *` — so a new one changes its result type. Postgres refuses to
    // run a cached plan whose result type has changed, and sqlx does not evict
    // the statement when it does: the connection holding it fails this query
    // from then on, and the error reaches the script as something it will read
    // as an empty table. Caching a plan over a schema the caller may change is
    // unsound however rarely it bites.
    let mut sql_query = sqlx::query(sqlx::AssertSqlSafe(sql.as_str())).persistent(false);
    for value in &bound {
        sql_query = bind_value(sql_query, value)?;
    }
    sql_query = sql_query.bind(limit_val);

    let rows = sql_query.fetch_all(&mut *conn).await.map_err(|e| {
        error!("Database error querying table: {}", e);
        AppError::Database {
            message: format!("Query error: {}", e),
            source: None,
        }
    })?;

    Ok(rows.iter().map(|row| row_to_json(row, &columns)).collect())
}

/// Insert a row into a script-owned table
pub(super) async fn db_insert_row(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
    data: &HashMap<String, serde_json::Value>,
) -> AppResult<serde_json::Value> {
    let columns = TableColumns::load(conn, script_uri, logical_table_name).await?;

    if data.is_empty() {
        return Err(AppError::Validation {
            field: "data".to_string(),
            reason: "No data provided for insert".to_string(),
        });
    }

    let bound = ordered_bindings(&columns, data)?;

    let mut column_list = Vec::new();
    let mut placeholders = Vec::new();
    for (position, value) in bound.iter().enumerate() {
        column_list.push(quote_identifier(value.column));
        placeholders.push(value.placeholder(position + 1));
    }

    let sql = format!(
        "INSERT INTO {} ({}) VALUES ({}) RETURNING *",
        quote_identifier(&columns.physical_name),
        column_list.join(", "),
        placeholders.join(", ")
    );

    // Not cached, deliberately. A script's table grows a column whenever the
    // script says so, and this statement names every column — `SELECT *`,
    // `RETURNING *` — so a new one changes its result type. Postgres refuses to
    // run a cached plan whose result type has changed, and sqlx does not evict
    // the statement when it does: the connection holding it fails this query
    // from then on, and the error reaches the script as something it will read
    // as an empty table. Caching a plan over a schema the caller may change is
    // unsound however rarely it bites.
    let mut sql_query = sqlx::query(sqlx::AssertSqlSafe(sql.as_str())).persistent(false);
    for value in &bound {
        sql_query = bind_value(sql_query, value)?;
    }

    let row = sql_query.fetch_one(&mut *conn).await.map_err(|e| {
        error!("Database error inserting row: {}", e);
        AppError::Database {
            message: format!("Insert error: {}", e),
            source: None,
        }
    })?;

    Ok(row_to_json(&row, &columns))
}

/// Update a row in a script-owned table
pub(super) async fn db_update_row(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
    id: i32,
    data: &HashMap<String, serde_json::Value>,
) -> AppResult<serde_json::Value> {
    let columns = TableColumns::load(conn, script_uri, logical_table_name).await?;

    if data.is_empty() {
        return Err(AppError::Validation {
            field: "data".to_string(),
            reason: "No data provided for update".to_string(),
        });
    }

    let bound = ordered_bindings(&columns, data)?;

    let mut param_count = 0usize;
    let set_clauses: Vec<String> = bound
        .iter()
        .map(|value| {
            param_count += 1;
            format!(
                "{} = {}",
                quote_identifier(value.column),
                value.placeholder(param_count)
            )
        })
        .collect();

    param_count += 1;
    let sql = format!(
        "UPDATE {} SET {} WHERE id = ${}::int4 RETURNING *",
        quote_identifier(&columns.physical_name),
        set_clauses.join(", "),
        param_count
    );

    // Not cached, deliberately. A script's table grows a column whenever the
    // script says so, and this statement names every column — `SELECT *`,
    // `RETURNING *` — so a new one changes its result type. Postgres refuses to
    // run a cached plan whose result type has changed, and sqlx does not evict
    // the statement when it does: the connection holding it fails this query
    // from then on, and the error reaches the script as something it will read
    // as an empty table. Caching a plan over a schema the caller may change is
    // unsound however rarely it bites.
    let mut sql_query = sqlx::query(sqlx::AssertSqlSafe(sql.as_str())).persistent(false);
    for value in &bound {
        sql_query = bind_value(sql_query, value)?;
    }
    sql_query = sql_query.bind(id);

    let row = sql_query.fetch_one(&mut *conn).await.map_err(|e| {
        error!("Database error updating row: {}", e);
        AppError::Database {
            message: format!("Update error: {}", e),
            source: None,
        }
    })?;

    Ok(row_to_json(&row, &columns))
}

/// Delete a row from a script-owned table by ID
pub(super) async fn db_delete_row(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
    id: i32,
) -> AppResult<bool> {
    let physical_table_name = get_physical_table_name(conn, script_uri, logical_table_name).await?;

    let sql = format!(
        "DELETE FROM {} WHERE id = $1::int4",
        quote_identifier(&physical_table_name)
    );

    let result = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
        .bind(id)
        .execute(&mut *conn)
        .await
        .map_err(|e| {
            error!("Database error deleting row: {}", e);
            AppError::Database {
                message: format!("Delete error: {}", e),
                source: None,
            }
        })?;

    Ok(result.rows_affected() > 0)
}

/// Upsert a row into a script-owned table using INSERT … ON CONFLICT DO UPDATE.
///
/// `key_columns` names the columns that form the conflict target; a unique
/// index on those columns must exist (create one with `db_add_unique_index`).
/// `data` must contain values for all columns, including the key columns.
pub(super) async fn db_upsert_row(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
    key_columns: &[String],
    data: &HashMap<String, serde_json::Value>,
) -> AppResult<serde_json::Value> {
    let columns = TableColumns::load(conn, script_uri, logical_table_name).await?;

    if data.is_empty() {
        return Err(AppError::Validation {
            field: "data".to_string(),
            reason: "No data provided for upsert".to_string(),
        });
    }
    if key_columns.is_empty() {
        return Err(AppError::Validation {
            field: "key_columns".to_string(),
            reason: "At least one key column must be specified".to_string(),
        });
    }

    // Validate key columns
    for kc in key_columns {
        validate_identifier(kc).map_err(|e| AppError::Validation {
            field: "key_column".to_string(),
            reason: e.to_string(),
        })?;
    }

    // Build column/placeholder lists (deterministic order via sorted keys)
    let bound = ordered_bindings(&columns, data)?;

    let mut col_list = Vec::new();
    let mut placeholder_list = Vec::new();
    for (position, value) in bound.iter().enumerate() {
        col_list.push(quote_identifier(value.column));
        placeholder_list.push(value.placeholder(position + 1));
    }

    // SET clause: update all non-key columns
    let key_set: std::collections::HashSet<&str> = key_columns.iter().map(|s| s.as_str()).collect();
    let set_clauses: Vec<String> = bound
        .iter()
        .filter(|value| !key_set.contains(value.column))
        .map(|value| {
            format!(
                "{} = EXCLUDED.{}",
                quote_identifier(value.column),
                quote_identifier(value.column)
            )
        })
        .collect();

    let conflict_target = key_columns
        .iter()
        .map(|c| quote_identifier(c))
        .collect::<Vec<_>>()
        .join(", ");

    let sql = if set_clauses.is_empty() {
        // All columns are key columns — DO NOTHING is the right action
        format!(
            "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT ({}) DO NOTHING RETURNING *",
            quote_identifier(&columns.physical_name),
            col_list.join(", "),
            placeholder_list.join(", "),
            conflict_target,
        )
    } else {
        format!(
            "INSERT INTO {} ({}) VALUES ({}) ON CONFLICT ({}) DO UPDATE SET {} RETURNING *",
            quote_identifier(&columns.physical_name),
            col_list.join(", "),
            placeholder_list.join(", "),
            conflict_target,
            set_clauses.join(", "),
        )
    };

    // Not cached, deliberately. A script's table grows a column whenever the
    // script says so, and this statement names every column — `SELECT *`,
    // `RETURNING *` — so a new one changes its result type. Postgres refuses to
    // run a cached plan whose result type has changed, and sqlx does not evict
    // the statement when it does: the connection holding it fails this query
    // from then on, and the error reaches the script as something it will read
    // as an empty table. Caching a plan over a schema the caller may change is
    // unsound however rarely it bites.
    let mut sql_query = sqlx::query(sqlx::AssertSqlSafe(sql.as_str())).persistent(false);
    for value in &bound {
        sql_query = bind_value(sql_query, value)?;
    }

    let row = sql_query.fetch_one(&mut *conn).await.map_err(|e| {
        error!("Database error upserting row: {}", e);
        AppError::Database {
            message: format!("Upsert error: {}", e),
            source: None,
        }
    })?;

    Ok(row_to_json(&row, &columns))
}

/// Delete rows from a script-owned table matching the given filter conditions.
///
/// Supports the same filter syntax as `db_query_table` (equality and range
/// operators). Returns the number of rows deleted.
pub(super) async fn db_delete_where(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
    filters: &HashMap<String, serde_json::Value>,
) -> AppResult<u64> {
    let columns = TableColumns::load(conn, script_uri, logical_table_name).await?;

    if filters.is_empty() {
        return Err(AppError::Validation {
            field: "filters".to_string(),
            reason: "deleteWhere requires at least one filter to prevent accidental full-table delete. Use dropTable to remove all rows.".to_string(),
        });
    }

    let conditions = parse_filter_conditions(filters)?;

    let bound: Vec<BoundValue<'_>> = conditions
        .iter()
        .map(|c| BoundValue::new(&columns, &c.column, &c.value))
        .collect();

    let mut param_count = 0usize;
    let clauses: Vec<String> = conditions
        .iter()
        .zip(&bound)
        .map(|(c, b)| {
            param_count += 1;
            let op_str = match c.op {
                FilterOp::Eq => "=",
                FilterOp::Gt => ">",
                FilterOp::Gte => ">=",
                FilterOp::Lt => "<",
                FilterOp::Lte => "<=",
                FilterOp::Ne => "!=",
            };
            format!(
                "{} {} {}",
                quote_identifier(&c.column),
                op_str,
                b.placeholder(param_count)
            )
        })
        .collect();

    let sql = format!(
        "DELETE FROM {} WHERE {}",
        quote_identifier(&columns.physical_name),
        clauses.join(" AND ")
    );

    let mut sql_query = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()));
    for value in &bound {
        sql_query = bind_value(sql_query, value)?;
    }

    let result = sql_query.execute(&mut *conn).await.map_err(|e| {
        error!("Database error in deleteWhere: {}", e);
        AppError::Database {
            message: format!("Delete error: {}", e),
            source: None,
        }
    })?;

    Ok(result.rows_affected())
}

/// Add a unique index on one or more columns of a script-owned table.
///
/// This is required before using those columns as a conflict target in
/// `db_upsert_row`. Index names are derived from the physical table name and
/// columns to avoid collisions.
pub(super) async fn db_add_unique_index(
    conn: &mut PgConnection,
    script_uri: &str,
    logical_table_name: &str,
    columns: &[String],
) -> AppResult<()> {
    let physical_table_name = get_physical_table_name(conn, script_uri, logical_table_name).await?;

    if columns.is_empty() {
        return Err(AppError::Validation {
            field: "columns".to_string(),
            reason: "At least one column must be specified".to_string(),
        });
    }

    for col in columns {
        validate_identifier(col).map_err(|e| AppError::Validation {
            field: "column".to_string(),
            reason: e.to_string(),
        })?;
    }

    // Build a deterministic index name
    let cols_slug = columns.join("_");
    // Truncate to stay within PostgreSQL's 63-char identifier limit
    let index_name = format!(
        "{}_uniq_{}",
        &physical_table_name[..physical_table_name.len().min(40)],
        &cols_slug[..cols_slug.len().min(20)]
    );

    let col_list = columns
        .iter()
        .map(|c| quote_identifier(c))
        .collect::<Vec<_>>()
        .join(", ");

    let sql = format!(
        "CREATE UNIQUE INDEX IF NOT EXISTS {} ON {} ({})",
        quote_identifier(&index_name),
        quote_identifier(&physical_table_name),
        col_list
    );

    sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
        .execute(&mut *conn)
        .await
        .map_err(|e| {
            error!("Database error adding unique index: {}", e);
            AppError::Database {
                message: format!("Index error: {}", e),
                source: None,
            }
        })?;

    debug!(
        "Created unique index '{}' on table '{}' for script '{}'",
        index_name, logical_table_name, script_uri
    );

    Ok(())
}

// ============================================================================
// Script Database Schema Public API
// ============================================================================

/// Create a new table for a script
pub fn create_script_table(script_uri: &str, logical_table_name: &str) -> AppResult<String> {
    let repo = get_repository();
    run_bounded(async {
        repo.create_script_table(script_uri, logical_table_name)
            .await
    })
}

/// Add a column to a script-owned table
pub fn add_column_to_script_table(
    script_uri: &str,
    logical_table_name: &str,
    column_name: &str,
    column_type: ColumnType,
    nullable: bool,
    default_value: Option<&str>,
) -> AppResult<()> {
    let repo = get_repository();
    run_bounded(async {
        repo.add_column_to_script_table(
            script_uri,
            logical_table_name,
            column_name,
            column_type,
            nullable,
            default_value,
        )
        .await
    })
}

/// Add a reference column (INTEGER with FK) to a script-owned table
pub fn add_reference_column(
    script_uri: &str,
    logical_table_name: &str,
    column_name: &str,
    referenced_logical_table_name: &str,
    nullable: bool,
) -> AppResult<()> {
    let repo = get_repository();
    run_bounded(async {
        repo.add_reference_column(
            script_uri,
            logical_table_name,
            column_name,
            referenced_logical_table_name,
            nullable,
        )
        .await
    })
}

/// Drop a column from a script-owned table
pub fn drop_column(
    script_uri: &str,
    logical_table_name: &str,
    column_name: &str,
) -> AppResult<bool> {
    let repo = get_repository();
    run_bounded(async {
        repo.drop_column(script_uri, logical_table_name, column_name)
            .await
    })
}

/// Drop a script-owned table
pub fn drop_script_table(script_uri: &str, logical_table_name: &str) -> AppResult<bool> {
    let repo = get_repository();
    run_bounded(async { repo.drop_script_table(script_uri, logical_table_name).await })
}

// ============================================================================
// Script Database Introspection and Data Operations (Synchronous Wrappers)
// ============================================================================

/// List all tables owned by a script
pub fn list_script_tables(script_uri: &str) -> AppResult<Vec<TableInfo>> {
    let repo = get_repository();
    run_bounded(async { repo.list_script_tables(script_uri).await })
}

/// Get detailed schema for a table
pub fn get_table_schema(script_uri: &str, logical_table_name: &str) -> AppResult<TableSchema> {
    let repo = get_repository();
    run_bounded(async { repo.get_table_schema(script_uri, logical_table_name).await })
}

/// Get foreign key relationships for a table
pub fn get_foreign_keys(
    script_uri: &str,
    logical_table_name: &str,
) -> AppResult<Vec<ForeignKeyInfo>> {
    let repo = get_repository();
    run_bounded(async { repo.get_foreign_keys(script_uri, logical_table_name).await })
}

/// Query rows from a script-owned table
pub fn query_table(
    script_uri: &str,
    logical_table_name: &str,
    filters: Option<&HashMap<String, serde_json::Value>>,
    options: &QueryOptions,
) -> AppResult<Vec<serde_json::Value>> {
    let repo = get_repository();
    run_bounded(async {
        repo.query_table(script_uri, logical_table_name, filters, options)
            .await
    })
}

/// Insert a row into a script-owned table
pub fn insert_row(
    script_uri: &str,
    logical_table_name: &str,
    data: &HashMap<String, serde_json::Value>,
) -> AppResult<serde_json::Value> {
    let repo = get_repository();
    run_bounded(async { repo.insert_row(script_uri, logical_table_name, data).await })
}

/// Update a row in a script-owned table
pub fn update_row(
    script_uri: &str,
    logical_table_name: &str,
    id: i32,
    data: &HashMap<String, serde_json::Value>,
) -> AppResult<serde_json::Value> {
    let repo = get_repository();
    run_bounded(async {
        repo.update_row(script_uri, logical_table_name, id, data)
            .await
    })
}

/// Delete a row from a script-owned table
pub fn delete_row(script_uri: &str, logical_table_name: &str, id: i32) -> AppResult<bool> {
    let repo = get_repository();
    run_bounded(async { repo.delete_row(script_uri, logical_table_name, id).await })
}

/// Upsert a row into a script-owned table (INSERT … ON CONFLICT DO UPDATE)
pub fn upsert_row(
    script_uri: &str,
    logical_table_name: &str,
    key_columns: &[String],
    data: &HashMap<String, serde_json::Value>,
) -> AppResult<serde_json::Value> {
    let repo = get_repository();
    run_bounded(async {
        repo.upsert_row(script_uri, logical_table_name, key_columns, data)
            .await
    })
}

/// Delete rows from a script-owned table matching the given filters
pub fn delete_where(
    script_uri: &str,
    logical_table_name: &str,
    filters: &HashMap<String, serde_json::Value>,
) -> AppResult<u64> {
    let repo = get_repository();
    run_bounded(async {
        repo.delete_where(script_uri, logical_table_name, filters)
            .await
    })
}

/// Bring a script-owned table to the shape `spec` describes
pub fn ensure_script_table(
    script_uri: &str,
    logical_table_name: &str,
    spec: &TableSpec,
) -> AppResult<EnsuredTable> {
    let repo = get_repository();
    run_bounded(async {
        repo.ensure_script_table(script_uri, logical_table_name, spec)
            .await
    })
}
