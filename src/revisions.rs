//! Revisions of a script's files.
//!
//! Every write to a script — its root source or any of its assets — records
//! what the script consisted of once the write landed. The record is a full
//! manifest, not a delta, and the content behind it is addressed by digest and
//! shared, so a revision of a script whose one module changed costs one blob
//! and a row per file.
//!
//! The unit is the script rather than the file, because that is the unit a
//! change actually has: assets are keyed by their owning script, one write
//! already stores several of them in one transaction, and the changes worth
//! undoing span modules. A per-file history is a query over these manifests
//! ([`file_history`]); the reverse is not reconstructable.

use sqlx::{PgConnection, Row};

use crate::error::{AppError, AppResult};
use crate::repository;

/// How a revision came to be. Recorded so a history reads as a sequence of
/// acts rather than of anonymous content changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// A whole-file write through `POST /engine/assets`.
    Post,
    /// An atomic multi-file write through `/engine/assets/batch`.
    Batch,
    /// A string patch through `PATCH /engine/assets`.
    Patch,
    /// An asset removed.
    Delete,
    /// The script's own root source was written.
    Script,
    /// A revision restored from an earlier one.
    Revert,
    /// Written by the engine at startup rather than by a caller.
    Bootstrap,
}

impl Origin {
    pub fn as_str(self) -> &'static str {
        match self {
            Origin::Post => "post",
            Origin::Batch => "batch",
            Origin::Patch => "patch",
            Origin::Delete => "delete",
            Origin::Script => "script",
            Origin::Revert => "revert",
            Origin::Bootstrap => "bootstrap",
        }
    }
}

/// One file as a revision held it. Carries the file's identity and size, not
/// its content — a manifest listing should not read blobs it is not asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionFile {
    pub uri: String,
    pub name: Option<String>,
    pub mimetype: String,
    pub sha256: String,
    pub bytes: i32,
}

/// A revision, without its manifest.
#[derive(Debug, Clone)]
pub struct Revision {
    pub revision: i32,
    pub parent: Option<i32>,
    pub root_sha256: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub created_by: Option<String>,
    pub origin: String,
    pub label: Option<String>,
    /// Whether the script's `init()` succeeded on the write that produced this
    /// revision. `None` until the write path reports it — an init that has not
    /// run yet is not the same as one that failed.
    pub init_ok: Option<bool>,
    pub init_error: Option<String>,
    /// Files in the manifest, and their total size.
    pub file_count: i64,
    pub total_bytes: i64,
}

fn db_error(context: &str, e: sqlx::Error) -> AppError {
    tracing::error!("Database error {}: {}", context, e);
    AppError::Database {
        message: format!("Database error {}: {}", context, e),
        source: None,
    }
}

/// Run `f` against a connection inside a transaction, joining the caller's
/// when one is active and opening one otherwise.
///
/// Joining matters: a script writing inside `transaction()` that then rolls
/// back must not leave a revision behind describing content no one can read.
///
/// For reads, use [`with_read_connection`] — a `BEGIN`/`COMMIT` pair around a
/// single `SELECT` is two round trips spent to protect nothing, and this
/// module's reads run often enough for that to be the difference between a
/// server start costing three queries and costing a thousand.
async fn with_transaction<T, F>(f: F) -> AppResult<T>
where
    F: for<'c> FnOnce(
        &'c mut PgConnection,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = AppResult<T>> + Send + 'c>,
    >,
{
    let Some(db) = repository::get_db_pool() else {
        return Err(AppError::Database {
            message: "No database configured".to_string(),
            source: None,
        });
    };
    let pool = db.pool();

    if crate::database::get_current_transaction_active() {
        match crate::database::get_current_executor(pool) {
            crate::database::TransactionExecutor::Transaction(tx) => f(tx).await,
            crate::database::TransactionExecutor::Pool(pool) => {
                let mut conn = pool
                    .acquire()
                    .await
                    .map_err(|e| db_error("acquiring connection", e))?;
                f(&mut conn).await
            }
        }
    } else {
        let mut tx = pool
            .begin()
            .await
            .map_err(|e| db_error("opening revision transaction", e))?;
        let result = f(&mut tx).await;
        match result {
            Ok(value) => {
                tx.commit()
                    .await
                    .map_err(|e| db_error("committing revision", e))?;
                Ok(value)
            }
            Err(e) => {
                let _ = tx.rollback().await;
                Err(e)
            }
        }
    }
}

/// Run `f` against a connection without opening a transaction of its own.
///
/// Still joins the caller's transaction when there is one, so a script reading
/// its own history inside `transaction()` sees what it has written there.
async fn with_read_connection<T, F>(f: F) -> AppResult<T>
where
    F: for<'c> FnOnce(
        &'c mut PgConnection,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = AppResult<T>> + Send + 'c>,
    >,
{
    let Some(db) = repository::get_db_pool() else {
        return Err(AppError::Database {
            message: "No database configured".to_string(),
            source: None,
        });
    };
    let pool = db.pool();

    if crate::database::get_current_transaction_active()
        && let crate::database::TransactionExecutor::Transaction(tx) =
            crate::database::get_current_executor(pool)
    {
        return f(tx).await;
    }

    let mut conn = pool
        .acquire()
        .await
        .map_err(|e| db_error("acquiring connection", e))?;
    f(&mut conn).await
}

/// Record what `script_uri` consists of right now as its next revision.
///
/// Returns the new revision number, or `None` when the script's content is
/// identical to what the previous revision already holds — a write that
/// changed nothing is not a revision, and recording it would push genuine
/// history out of any retention window that counts rows.
///
/// The snapshot reads the live rows rather than taking the content from the
/// caller, so it records every write path — including deletes and patches —
/// without each of them having to describe its own result. When no transaction
/// is active the write it follows has already committed, so a second writer
/// landing in between would be captured by this snapshot instead of by its
/// own; the recorded state is still one the script genuinely had, and the
/// per-script advisory lock keeps the numbering itself consistent.
pub async fn record(
    script_uri: &str,
    origin: Origin,
    created_by: Option<&str>,
) -> AppResult<Option<i32>> {
    record_with_parent(script_uri, origin, created_by, None).await
}

/// [`record`], naming the revision this one was computed against.
///
/// Only a revert needs this. Its parent is the revision it restored, not the
/// one it happened to follow, and recording that is what lets a history be
/// read as a graph — "48 is 41 again" — rather than as a line that doubles
/// back without saying so.
pub async fn record_with_parent(
    script_uri: &str,
    origin: Origin,
    created_by: Option<&str>,
    parent: Option<i32>,
) -> AppResult<Option<i32>> {
    let script_uri = script_uri.to_string();
    let created_by = created_by.map(str::to_string);
    with_transaction(move |conn| {
        Box::pin(async move {
            record_in(conn, &script_uri, origin, created_by.as_deref(), parent).await
        })
    })
    .await
}

/// One file of the live state being snapshotted.
///
/// Deliberately without content: the digests are computed by Postgres and the
/// bytes are copied blob-side without ever entering this process.
struct FileEntry {
    uri: String,
    name: Option<String>,
    mimetype: String,
    sha256: String,
}

async fn record_in(
    conn: &mut PgConnection,
    script_uri: &str,
    origin: Origin,
    created_by: Option<&str>,
    parent_override: Option<i32>,
) -> AppResult<Option<i32>> {
    // Serialise revision numbering per script. Two writes landing together
    // would otherwise read the same `MAX(revision)` and one would fail on the
    // unique constraint; the lock is released when the transaction ends.
    // `hashtext` collisions between two scripts cost a little serialisation
    // and nothing else.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
        .bind(script_uri)
        .execute(&mut *conn)
        .await
        .map_err(|e| db_error("locking script for revision", e))?;

    // Store the root's bytes and report their digest in one statement.
    //
    // A revision is recorded after every write, including a patch that touched
    // three lines, so what it costs is what a write costs. Hashing in the
    // database and copying blob-side keeps that cost off the wire entirely:
    // nothing here reads a byte of the script's content, however large the
    // tree is.
    // `ON CONFLICT DO UPDATE` rather than `DO NOTHING`, for a reason that has
    // nothing to do with the column it sets. `DO NOTHING` leaves an existing
    // blob untouched and so unlocked, and a collector running beside this one
    // could judge that blob unreferenced — its last citing revision having
    // just been pruned — and delete it between here and the manifest insert
    // below, which would then fail on the foreign key and lose the revision.
    // Touching the row locks it for the rest of this transaction and moves it
    // out of the collector's grace period.
    let root_sha: Option<String> = sqlx::query_scalar(
        "WITH root AS (
             SELECT convert_to(content, 'UTF8') AS bytes FROM scripts WHERE uri = $1
         ), stored AS (
             INSERT INTO asset_blobs (sha256, bytes, content)
             SELECT encode(sha256(bytes), 'hex'), octet_length(bytes), bytes FROM root
             ON CONFLICT (sha256) DO UPDATE SET created_at = NOW()
             RETURNING 1
         )
         SELECT encode(sha256(bytes), 'hex') FROM root",
    )
    .bind(script_uri)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| db_error("storing revision root", e))?;

    // A revision of a script that is not there is not a revision of anything.
    // The delete path reaches this when the script itself was removed: its
    // assets went with it, and the history it already has is what remains.
    let Some(root_sha) = root_sha else {
        return Ok(None);
    };

    // The same shape for the assets: their blobs are stored and their identity
    // reported by one statement, so both see one snapshot of the table. Read
    // separately, a file rewritten in between would be described by a manifest
    // row whose digest names a blob nothing ever stored.
    let rows = sqlx::query(
        "WITH current_files AS (
             SELECT uri, name, mimetype, content, encode(sha256(content), 'hex') AS sha256
             FROM assets WHERE script_uri = $1
         ), stored AS (
             INSERT INTO asset_blobs (sha256, bytes, content)
             SELECT DISTINCT ON (sha256) sha256, octet_length(content), content
             FROM current_files
             ORDER BY sha256
             ON CONFLICT (sha256) DO UPDATE SET created_at = NOW()
             RETURNING 1
         )
         SELECT uri, name, mimetype, sha256 FROM current_files ORDER BY uri",
    )
    .bind(script_uri)
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| db_error("storing revision blobs", e))?;

    let files: Vec<FileEntry> = rows
        .into_iter()
        .map(|row| FileEntry {
            uri: row.get("uri"),
            name: row.get("name"),
            mimetype: row.get("mimetype"),
            sha256: row.get("sha256"),
        })
        .collect();

    let previous = sqlx::query(
        "SELECT id, revision, root_sha256 FROM script_revisions
         WHERE script_uri = $1 ORDER BY revision DESC LIMIT 1",
    )
    .bind(script_uri)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|e| db_error("reading previous revision", e))?;

    // `latest` numbers the new revision; `parent` says what it was computed
    // against. They are the same for an ordinary write and differ for a
    // revert, which follows head but descends from the revision it restored.
    // What the script's tables look like right now. Recorded beside the files
    // rather than derived later: the schema a revision ran against is only
    // knowable while that revision is current.
    let tables = read_table_schemas(conn, script_uri).await?;

    let latest = match &previous {
        Some(row) => {
            let previous_id: i64 = row.get("id");
            let previous_root: String = row.get("root_sha256");
            if previous_root == root_sha && manifest_matches(conn, previous_id, &files).await? {
                return Ok(None);
            }
            let revision: i32 = row.get("revision");
            Some(revision)
        }
        None => None,
    };

    let parent = parent_override.or(latest);
    let next = latest.unwrap_or(0) + 1;
    let revision_id: i64 = sqlx::query_scalar(
        "INSERT INTO script_revisions
             (script_uri, revision, parent, root_sha256, created_by, origin, tables)
         VALUES ($1, $2, $3, $4, $5, $6, $7)
         RETURNING id",
    )
    .bind(script_uri)
    .bind(next)
    .bind(parent)
    .bind(&root_sha)
    .bind(created_by)
    .bind(origin.as_str())
    .bind(serde_json::Value::Object(tables))
    .fetch_one(&mut *conn)
    .await
    .map_err(|e| db_error("recording revision", e))?;

    if !files.is_empty() {
        let uris: Vec<String> = files.iter().map(|f| f.uri.clone()).collect();
        let shas: Vec<String> = files.iter().map(|f| f.sha256.clone()).collect();
        let mimetypes: Vec<String> = files.iter().map(|f| f.mimetype.clone()).collect();
        let names: Vec<Option<String>> = files.iter().map(|f| f.name.clone()).collect();

        sqlx::query(
            "INSERT INTO script_revision_files (revision_id, uri, sha256, mimetype, name)
             SELECT $1, * FROM UNNEST($2::text[], $3::text[], $4::text[], $5::text[])",
        )
        .bind(revision_id)
        .bind(&uris)
        .bind(&shas)
        .bind(&mimetypes)
        .bind(&names)
        .execute(&mut *conn)
        .await
        .map_err(|e| db_error("recording revision manifest", e))?;
    }

    // This instance wrote it, so this instance is running it — which is what a
    // log line written from here should be attributed to.
    remember_current(script_uri, next);

    tracing::debug!(
        script = script_uri,
        revision = next,
        origin = origin.as_str(),
        files = files.len(),
        "Recorded script revision"
    );

    Ok(Some(next))
}

/// Whether revision `revision_id`'s manifest is exactly `files`.
///
/// Compared by digest, so equal answers mean equal bytes without reading a
/// blob. `name` is deliberately not compared: it is a display label, and
/// relabelling a file changes nothing about what the script runs.
async fn manifest_matches(
    conn: &mut PgConnection,
    revision_id: i64,
    files: &[FileEntry],
) -> AppResult<bool> {
    let rows = sqlx::query(
        "SELECT uri, sha256, mimetype FROM script_revision_files
         WHERE revision_id = $1 ORDER BY uri",
    )
    .bind(revision_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| db_error("reading revision manifest", e))?;

    if rows.len() != files.len() {
        return Ok(false);
    }

    // Both sides are ordered by `uri`, so one pass compares them.
    Ok(rows.iter().zip(files).all(|(row, file)| {
        let uri: String = row.get("uri");
        let sha256: String = row.get("sha256");
        let mimetype: String = row.get("mimetype");
        uri == file.uri && sha256 == file.sha256 && mimetype == file.mimetype
    }))
}

// ============================================================================
// Reading revisions
// ============================================================================

/// The projection every revision listing reads.
///
/// Spelled out in each query rather than assembled from pieces: sqlx only
/// accepts SQL it can see at compile time, which is a constraint worth
/// keeping rather than defeating with a wrapper that asserts the string is
/// safe.
fn revision_from_row(row: &sqlx::postgres::PgRow) -> Revision {
    Revision {
        revision: row.get("revision"),
        parent: row.get("parent"),
        root_sha256: row.get("root_sha256"),
        created_at: row.get("created_at"),
        created_by: row.get("created_by"),
        origin: row.get("origin"),
        label: row.get("label"),
        init_ok: row.get("init_ok"),
        init_error: row.get("init_error"),
        file_count: row.get("file_count"),
        total_bytes: row.get("total_bytes"),
    }
}

/// The newest revision number of `script_uri`, or `None` when it has no
/// history yet.
pub async fn head(script_uri: &str) -> AppResult<Option<i32>> {
    let script_uri = script_uri.to_string();
    with_read_connection(move |conn| {
        Box::pin(async move {
            sqlx::query_scalar("SELECT MAX(revision) FROM script_revisions WHERE script_uri = $1")
                .bind(&script_uri)
                .fetch_one(conn)
                .await
                .map_err(|e| db_error("reading head revision", e))
        })
    })
    .await
}

/// The newest revision whose write left the script initialising cleanly.
///
/// The rollback target an operator means by "put it back to when it worked",
/// and the one thing this history offers that a version control system cannot:
/// the engine ran the code and knows how it went.
pub async fn last_good(script_uri: &str) -> AppResult<Option<i32>> {
    let script_uri = script_uri.to_string();
    with_read_connection(move |conn| {
        Box::pin(async move {
            sqlx::query_scalar(
                "SELECT MAX(revision) FROM script_revisions
                 WHERE script_uri = $1 AND init_ok IS TRUE",
            )
            .bind(&script_uri)
            .fetch_one(conn)
            .await
            .map_err(|e| db_error("reading last good revision", e))
        })
    })
    .await
}

/// A script's revisions, newest first.
pub async fn list(script_uri: &str, limit: i64) -> AppResult<Vec<Revision>> {
    let script_uri = script_uri.to_string();
    let limit = limit.clamp(1, 1000);
    with_read_connection(move |conn| {
        Box::pin(async move {
            let rows = sqlx::query(
                "SELECT r.revision, r.parent, r.root_sha256, r.created_at, r.created_by,
                        r.origin, r.label, r.init_ok, r.init_error,
                        COUNT(f.uri) AS file_count,
                        COALESCE(SUM(b.bytes), 0)::bigint AS total_bytes
                 FROM script_revisions r
                 LEFT JOIN script_revision_files f ON f.revision_id = r.id
                 LEFT JOIN asset_blobs b ON b.sha256 = f.sha256
                 WHERE r.script_uri = $1
                 GROUP BY r.id
                 ORDER BY r.revision DESC
                 LIMIT $2",
            )
            .bind(&script_uri)
            .bind(limit)
            .fetch_all(conn)
            .await
            .map_err(|e| db_error("listing revisions", e))?;
            Ok(rows.iter().map(revision_from_row).collect())
        })
    })
    .await
}

/// One revision, without its manifest.
pub async fn get(script_uri: &str, revision: i32) -> AppResult<Option<Revision>> {
    let script_uri = script_uri.to_string();
    with_read_connection(move |conn| {
        Box::pin(async move {
            let row = sqlx::query(
                "SELECT r.revision, r.parent, r.root_sha256, r.created_at, r.created_by,
                        r.origin, r.label, r.init_ok, r.init_error,
                        COUNT(f.uri) AS file_count,
                        COALESCE(SUM(b.bytes), 0)::bigint AS total_bytes
                 FROM script_revisions r
                 LEFT JOIN script_revision_files f ON f.revision_id = r.id
                 LEFT JOIN asset_blobs b ON b.sha256 = f.sha256
                 WHERE r.script_uri = $1 AND r.revision = $2
                 GROUP BY r.id",
            )
            .bind(&script_uri)
            .bind(revision)
            .fetch_optional(conn)
            .await
            .map_err(|e| db_error("reading revision", e))?;
            Ok(row.as_ref().map(revision_from_row))
        })
    })
    .await
}

/// The revision a label names, or `None` when nothing carries it.
pub async fn by_label(script_uri: &str, label: &str) -> AppResult<Option<i32>> {
    let script_uri = script_uri.to_string();
    let label = label.to_string();
    with_read_connection(move |conn| {
        Box::pin(async move {
            sqlx::query_scalar(
                "SELECT revision FROM script_revisions WHERE script_uri = $1 AND label = $2",
            )
            .bind(&script_uri)
            .bind(&label)
            .fetch_optional(conn)
            .await
            .map_err(|e| db_error("resolving revision label", e))
        })
    })
    .await
}

/// The manifest of one revision: every file it contained, ordered by path.
pub async fn files(script_uri: &str, revision: i32) -> AppResult<Option<Vec<RevisionFile>>> {
    let script_uri = script_uri.to_string();
    with_read_connection(move |conn| {
        Box::pin(async move {
            let revision_id: Option<i64> = sqlx::query_scalar(
                "SELECT id FROM script_revisions WHERE script_uri = $1 AND revision = $2",
            )
            .bind(&script_uri)
            .bind(revision)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| db_error("resolving revision", e))?;

            let Some(revision_id) = revision_id else {
                return Ok(None);
            };

            let rows = sqlx::query(
                "SELECT f.uri, f.name, f.mimetype, f.sha256, b.bytes
                 FROM script_revision_files f
                 JOIN asset_blobs b ON b.sha256 = f.sha256
                 WHERE f.revision_id = $1
                 ORDER BY f.uri",
            )
            .bind(revision_id)
            .fetch_all(&mut *conn)
            .await
            .map_err(|e| db_error("reading revision manifest", e))?;

            Ok(Some(
                rows.into_iter()
                    .map(|row| RevisionFile {
                        uri: row.get("uri"),
                        name: row.get("name"),
                        mimetype: row.get("mimetype"),
                        sha256: row.get("sha256"),
                        bytes: row.get("bytes"),
                    })
                    .collect(),
            ))
        })
    })
    .await
}

/// One file as a revision held it: its content and MIME type.
///
/// `None` distinguishes nothing about *why* it is absent — the revision may
/// not exist, or may simply not have contained the file. Both mean the same
/// thing to a caller resolving an import against this revision.
pub async fn read_file(
    script_uri: &str,
    revision: i32,
    uri: &str,
) -> AppResult<Option<(Vec<u8>, String)>> {
    let script_uri = script_uri.to_string();
    let uri = uri.to_string();
    with_read_connection(move |conn| {
        Box::pin(async move {
            let row = sqlx::query(
                "SELECT b.content, f.mimetype
                 FROM script_revisions r
                 JOIN script_revision_files f ON f.revision_id = r.id
                 JOIN asset_blobs b ON b.sha256 = f.sha256
                 WHERE r.script_uri = $1 AND r.revision = $2 AND f.uri = $3",
            )
            .bind(&script_uri)
            .bind(revision)
            .bind(&uri)
            .fetch_optional(conn)
            .await
            .map_err(|e| db_error("reading revision file", e))?;

            Ok(row.map(|row| (row.get("content"), row.get("mimetype"))))
        })
    })
    .await
}

/// Several of a revision's files at once.
///
/// One query rather than one per file. A diff or a revert touches every file
/// that moved, and asking for them one at a time is a round trip each — which
/// is what the manifest comparison already saved by telling the caller exactly
/// which ones it needs.
pub async fn read_files(
    script_uri: &str,
    revision: i32,
    paths: &[String],
) -> AppResult<std::collections::HashMap<String, (Vec<u8>, String)>> {
    if paths.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let script_uri = script_uri.to_string();
    let paths = paths.to_vec();
    with_read_connection(move |conn| {
        Box::pin(async move {
            let rows = sqlx::query(
                "SELECT f.uri, b.content, f.mimetype
                 FROM script_revisions r
                 JOIN script_revision_files f ON f.revision_id = r.id
                 JOIN asset_blobs b ON b.sha256 = f.sha256
                 WHERE r.script_uri = $1 AND r.revision = $2 AND f.uri = ANY($3)",
            )
            .bind(&script_uri)
            .bind(revision)
            .bind(&paths)
            .fetch_all(conn)
            .await
            .map_err(|e| db_error("reading revision files", e))?;

            Ok(rows
                .into_iter()
                .map(|row| {
                    let uri: String = row.get("uri");
                    (uri, (row.get("content"), row.get("mimetype")))
                })
                .collect())
        })
    })
    .await
}

/// The manifests of several revisions at once, keyed by revision number.
///
/// The listing endpoint offers every revision's file list in one response, and
/// fetching them a revision at a time turns one answer into fifty round trips.
pub async fn files_for(
    script_uri: &str,
    revisions: &[i32],
) -> AppResult<std::collections::HashMap<i32, Vec<RevisionFile>>> {
    if revisions.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let script_uri = script_uri.to_string();
    let revisions = revisions.to_vec();
    with_read_connection(move |conn| {
        Box::pin(async move {
            let rows = sqlx::query(
                "SELECT r.revision, f.uri, f.name, f.mimetype, f.sha256, b.bytes
                 FROM script_revisions r
                 JOIN script_revision_files f ON f.revision_id = r.id
                 JOIN asset_blobs b ON b.sha256 = f.sha256
                 WHERE r.script_uri = $1 AND r.revision = ANY($2)
                 ORDER BY r.revision DESC, f.uri",
            )
            .bind(&script_uri)
            .bind(&revisions)
            .fetch_all(conn)
            .await
            .map_err(|e| db_error("reading revision manifests", e))?;

            let mut manifests: std::collections::HashMap<i32, Vec<RevisionFile>> =
                std::collections::HashMap::new();
            for row in rows {
                manifests
                    .entry(row.get("revision"))
                    .or_default()
                    .push(RevisionFile {
                        uri: row.get("uri"),
                        name: row.get("name"),
                        mimetype: row.get("mimetype"),
                        sha256: row.get("sha256"),
                        bytes: row.get("bytes"),
                    });
            }
            Ok(manifests)
        })
    })
    .await
}

/// The root source as a revision held it.
pub async fn root_content(script_uri: &str, revision: i32) -> AppResult<Option<String>> {
    let script_uri = script_uri.to_string();
    with_read_connection(move |conn| {
        Box::pin(async move {
            let content: Option<Vec<u8>> = sqlx::query_scalar(
                "SELECT b.content
                 FROM script_revisions r
                 JOIN asset_blobs b ON b.sha256 = r.root_sha256
                 WHERE r.script_uri = $1 AND r.revision = $2",
            )
            .bind(&script_uri)
            .bind(revision)
            .fetch_optional(conn)
            .await
            .map_err(|e| db_error("reading revision root", e))?;

            match content {
                Some(bytes) => String::from_utf8(bytes)
                    .map(Some)
                    .map_err(|_| AppError::Database {
                        message: "Stored root source is not valid UTF-8".to_string(),
                        source: None,
                    }),
                None => Ok(None),
            }
        })
    })
    .await
}

/// Every revision in which `asset_uri` changed, newest first.
///
/// A file's history, derived from the manifests rather than stored beside
/// them. Revisions in which the file was untouched are left out: the caller
/// asked what happened to this file, and repeating an unchanged digest for
/// every revision of every other file would bury it.
pub async fn file_history(
    script_uri: &str,
    asset_uri: &str,
    limit: i64,
) -> AppResult<Vec<(i32, RevisionFile)>> {
    let script_uri = script_uri.to_string();
    let asset_uri = asset_uri.to_string();
    let limit = limit.clamp(1, 1000);
    with_read_connection(move |conn| {
        Box::pin(async move {
            let rows = sqlx::query(
                "SELECT r.revision, f.uri, f.name, f.mimetype, f.sha256, b.bytes
                 FROM script_revisions r
                 JOIN script_revision_files f ON f.revision_id = r.id
                 JOIN asset_blobs b ON b.sha256 = f.sha256
                 WHERE r.script_uri = $1 AND f.uri = $2
                 ORDER BY r.revision DESC
                 LIMIT $3",
            )
            .bind(&script_uri)
            .bind(&asset_uri)
            .bind(limit)
            .fetch_all(conn)
            .await
            .map_err(|e| db_error("reading file history", e))?;

            // Collapse runs of the same digest: consecutive revisions that did
            // not touch this file report the same content, and listing each of
            // them answers a question nobody asked.
            let mut history: Vec<(i32, RevisionFile)> = Vec::new();
            for row in rows {
                let file = RevisionFile {
                    uri: row.get("uri"),
                    name: row.get("name"),
                    mimetype: row.get("mimetype"),
                    sha256: row.get("sha256"),
                    bytes: row.get("bytes"),
                };
                if history
                    .last()
                    .is_some_and(|(_, previous)| previous.sha256 == file.sha256)
                {
                    continue;
                }
                history.push((row.get("revision"), file));
            }
            Ok(history)
        })
    })
    .await
}

// ============================================================================
// Annotating revisions
// ============================================================================

/// Record how the script's `init()` went on the write that produced `revision`.
pub async fn set_init_outcome(
    script_uri: &str,
    revision: i32,
    ok: bool,
    error: Option<&str>,
) -> AppResult<()> {
    let script_uri = script_uri.to_string();
    let error = error.map(str::to_string);
    with_transaction(move |conn| {
        Box::pin(async move {
            sqlx::query(
                "UPDATE script_revisions SET init_ok = $3, init_error = $4
                 WHERE script_uri = $1 AND revision = $2",
            )
            .bind(&script_uri)
            .bind(revision)
            .bind(ok)
            .bind(error.as_deref())
            .execute(conn)
            .await
            .map_err(|e| db_error("recording init outcome", e))?;
            Ok(())
        })
    })
    .await
}

/// Name a revision, or clear its name with `None`.
///
/// A label is applied to a revision that already exists, which is the whole
/// point: whether a change was worth marking is known afterwards, not before.
/// Moving a label to another revision is allowed — labels name a state worth
/// returning to, and that state can be superseded.
pub async fn set_label(script_uri: &str, revision: i32, label: Option<&str>) -> AppResult<bool> {
    let script_uri = script_uri.to_string();
    let label = label.map(str::to_string);
    with_transaction(move |conn| {
        Box::pin(async move {
            if let Some(label) = label.as_deref() {
                // Free the name first: it is unique per script, and the caller
                // asked for this revision to carry it.
                sqlx::query(
                    "UPDATE script_revisions SET label = NULL
                     WHERE script_uri = $1 AND label = $2 AND revision <> $3",
                )
                .bind(&script_uri)
                .bind(label)
                .bind(revision)
                .execute(&mut *conn)
                .await
                .map_err(|e| db_error("clearing revision label", e))?;
            }

            let result = sqlx::query(
                "UPDATE script_revisions SET label = $3 WHERE script_uri = $1 AND revision = $2",
            )
            .bind(&script_uri)
            .bind(revision)
            .bind(label.as_deref())
            .execute(&mut *conn)
            .await
            .map_err(|e| db_error("labelling revision", e))?;

            Ok(result.rows_affected() > 0)
        })
    })
    .await
}

// ============================================================================
// Blocking wrappers
// ============================================================================

/// [`record`] from a synchronous context, reporting failure rather than
/// propagating it.
///
/// The write this follows has already landed. Failing the caller's request
/// because the *history* could not be written would report a change as
/// rejected when the files are stored and serving — so a failure here is
/// logged and the write stands without a revision.
pub fn record_blocking(script_uri: &str, origin: Origin, created_by: Option<&str>) -> Option<i32> {
    record_blocking_with_parent(script_uri, origin, created_by, None)
}

/// [`record_with_parent`] from a synchronous context, reporting failure rather
/// than propagating it.
pub fn record_blocking_with_parent(
    script_uri: &str,
    origin: Origin,
    created_by: Option<&str>,
    parent: Option<i32>,
) -> Option<i32> {
    match crate::database::run_blocking(record_with_parent(script_uri, origin, created_by, parent))
    {
        Ok(revision) => revision,
        Err(e) => {
            tracing::warn!(
                script = script_uri,
                origin = origin.as_str(),
                "Failed to record revision; the write itself stands: {}",
                e
            );
            None
        }
    }
}

/// Record how `init()` went, against the script's newest revision.
///
/// One statement rather than a read of head followed by an update of it: this
/// runs after every init, and every script's init runs at every startup, so
/// the difference is a round trip per script per boot.
///
/// The `IS DISTINCT FROM` guard means a boot that changed nothing writes
/// nothing — the common case, where a script initialises exactly as it did
/// last time.
///
/// Called after the write has been answered, so there is no one left to tell:
/// a revision missing its init outcome reads as "not known", which is what it
/// then is.
pub async fn annotate_init(script_uri: &str, ok: bool, error: Option<&str>) {
    let script_uri_owned = script_uri.to_string();
    let error_owned = error.map(str::to_string);
    let result = with_transaction(move |conn| {
        Box::pin(async move {
            sqlx::query(
                "UPDATE script_revisions SET init_ok = $2, init_error = $3
                 WHERE script_uri = $1
                   AND revision = (
                       SELECT MAX(revision) FROM script_revisions WHERE script_uri = $1
                   )
                   AND (init_ok IS DISTINCT FROM $2 OR init_error IS DISTINCT FROM $3)",
            )
            .bind(&script_uri_owned)
            .bind(ok)
            .bind(error_owned.as_deref())
            .execute(conn)
            .await
            .map_err(|e| db_error("recording init outcome", e))?;
            Ok(())
        })
    })
    .await;

    if let Err(e) = result {
        tracing::warn!(
            script = script_uri,
            "Failed to record init outcome on revision: {}",
            e
        );
    }
}

/// Give every script that has no history a revision recording its current
/// state.
///
/// Run once at startup, as a backfill for scripts that predate revisions —
/// not a per-boot snapshot, which is why it touches only scripts with no
/// revisions at all. Without it a script acquires its first revision only when
/// someone changes it, by which point the state they might have wanted back is
/// the one just overwritten.
///
/// Set-based on purpose. Asking each script in turn whether it has a revision
/// is a query per script per boot, and an engine with a few hundred scripts
/// pays that on every start of every instance.
/// `scope` restricts the pass to one script; `None` covers every script, which
/// is what startup does.
pub async fn backfill_missing(scope: Option<&str>) {
    let scope = scope.map(str::to_string);
    if let Err(e) = backfill_missing_inner(scope).await {
        tracing::warn!("Failed to record baseline revisions: {}", e);
    }
}

async fn backfill_missing_inner(scope: Option<String>) -> AppResult<()> {
    with_transaction(move |conn| {
        Box::pin(async move {
            // Blobs first, in their own statement: a revision row references
            // its root digest, and an insert of both in one statement would
            // rely on the order two CTEs happen to run in.
            //
            // The union covers root sources and assets together, so a root
            // whose bytes match an asset's is one blob — inserting them from
            // two CTEs of one statement could not have used ON CONFLICT to
            // find that out, since neither sees the other's rows.
            sqlx::query(
                "WITH missing AS (
                     SELECT s.uri, s.content FROM scripts s
                     WHERE NOT EXISTS (
                         SELECT 1 FROM script_revisions r WHERE r.script_uri = s.uri
                     )
                     AND ($1::text IS NULL OR s.uri = $1)
                 ), all_bytes AS (
                     SELECT convert_to(m.content, 'UTF8') AS bytes FROM missing m
                     UNION
                     SELECT a.content FROM assets a JOIN missing m ON m.uri = a.script_uri
                 )
                 INSERT INTO asset_blobs (sha256, bytes, content)
                 SELECT DISTINCT ON (encode(sha256(bytes), 'hex'))
                        encode(sha256(bytes), 'hex'), octet_length(bytes), bytes
                 FROM all_bytes
                 ORDER BY 1
                 ON CONFLICT (sha256) DO UPDATE SET created_at = NOW()",
            )
            .bind(scope.as_deref())
            .execute(&mut *conn)
            .await
            .map_err(|e| db_error("storing baseline blobs", e))?;

            // `ON CONFLICT DO NOTHING` rather than a guard: two instances
            // starting together read the same set of scripts without history,
            // and the one that loses simply has nothing to record.
            let created: Vec<(i64, String)> = sqlx::query_as(
                "INSERT INTO script_revisions
                     (script_uri, revision, parent, root_sha256, origin, tables)
                 SELECT s.uri, 1, NULL,
                        encode(sha256(convert_to(s.content, 'UTF8')), 'hex'), 'bootstrap',
                        -- The schema this baseline ran against, so a revert to
                        -- it can say how the data has moved since. Left out,
                        -- every backfilled revision would report that it
                        -- predates schema recording.
                        COALESCE(
                            (SELECT jsonb_object_agg(t.logical_table_name, t.schema_json)
                             FROM script_tables t WHERE t.script_uri = s.uri),
                            '{}'::jsonb
                        )
                 FROM scripts s
                 WHERE NOT EXISTS (
                     SELECT 1 FROM script_revisions r WHERE r.script_uri = s.uri
                 )
                 AND ($1::text IS NULL OR s.uri = $1)
                 ON CONFLICT (script_uri, revision) DO NOTHING
                 RETURNING id, script_uri",
            )
            .bind(scope.as_deref())
            .fetch_all(&mut *conn)
            .await
            .map_err(|e| db_error("recording baseline revisions", e))?;

            if created.is_empty() {
                return Ok(());
            }

            let ids: Vec<i64> = created.iter().map(|(id, _)| *id).collect();
            sqlx::query(
                "INSERT INTO script_revision_files (revision_id, uri, sha256, mimetype, name)
                 SELECT r.id, a.uri, encode(sha256(a.content), 'hex'), a.mimetype, a.name
                 FROM script_revisions r
                 JOIN assets a ON a.script_uri = r.script_uri
                 WHERE r.id = ANY($1)",
            )
            .bind(&ids)
            .execute(&mut *conn)
            .await
            .map_err(|e| db_error("recording baseline manifests", e))?;

            tracing::info!(
                scripts = created.len(),
                "Recorded baseline revisions for scripts with no history"
            );
            Ok(())
        })
    })
    .await
}

// ============================================================================
// Reverting
// ============================================================================

/// What restoring a revision would change.
#[derive(Debug, Clone, Default)]
pub struct RevertPlan {
    /// Files the target revision holds whose stored content differs, or which
    /// are not stored at all.
    pub writes: Vec<RevisionFile>,
    /// Files stored now that the target revision did not contain.
    ///
    /// Restoring without these would leave a tree that is neither the old
    /// version nor the new one — a module deleted in the change stays behind,
    /// shadowing nothing and imported by nobody, until it is imported again by
    /// the next person who assumes it is current.
    pub deletes: Vec<String>,
    /// Whether the script's root source differs from the target's.
    pub root_changes: bool,
}

impl RevertPlan {
    /// Whether the deployed files already are the target revision.
    pub fn is_empty(&self) -> bool {
        self.writes.is_empty() && self.deletes.is_empty() && !self.root_changes
    }
}

/// Work out what restoring `target` would change, without changing anything.
///
/// Compared by digest against what is stored, so a file the target holds and
/// the deployment already has is not rewritten. That keeps a revert to a
/// nearby revision as small as the change that caused it, and keeps the
/// revision it records honest about what moved.
pub async fn plan_revert(script_uri: &str, target: i32) -> AppResult<Option<RevertPlan>> {
    let Some(files) = files(script_uri, target).await? else {
        return Ok(None);
    };
    let Some(target_root) = root_content(script_uri, target).await? else {
        return Ok(None);
    };

    let script_uri_owned = script_uri.to_string();
    let current = with_read_connection(move |conn| {
        Box::pin(async move {
            let rows = sqlx::query(
                "SELECT uri, encode(sha256(content), 'hex') AS sha256
                 FROM assets WHERE script_uri = $1",
            )
            .bind(&script_uri_owned)
            .fetch_all(&mut *conn)
            .await
            .map_err(|e| db_error("reading deployed files", e))?;

            let root: Option<String> = sqlx::query_scalar(
                "SELECT encode(sha256(convert_to(content, 'UTF8')), 'hex')
                 FROM scripts WHERE uri = $1",
            )
            .bind(&script_uri_owned)
            .fetch_optional(&mut *conn)
            .await
            .map_err(|e| db_error("reading deployed root", e))?;

            let stored: std::collections::HashMap<String, String> = rows
                .into_iter()
                .map(|row| (row.get("uri"), row.get("sha256")))
                .collect();
            Ok((stored, root))
        })
    })
    .await?;

    let (stored, stored_root) = current;

    let target_root_sha = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(target_root.as_bytes());
        hex::encode(hasher.finalize())
    };

    let mut plan = RevertPlan {
        root_changes: stored_root.as_deref() != Some(target_root_sha.as_str()),
        ..RevertPlan::default()
    };

    for file in &files {
        if stored.get(&file.uri) != Some(&file.sha256) {
            plan.writes.push(file.clone());
        }
    }

    let held: std::collections::HashSet<&str> =
        files.iter().map(|file| file.uri.as_str()).collect();
    plan.deletes = stored
        .keys()
        .filter(|uri| !held.contains(uri.as_str()))
        .cloned()
        .collect();
    plan.deletes.sort();

    Ok(Some(plan))
}

/// The content a revert needs to write, read from the target revision.
pub async fn revert_content(
    script_uri: &str,
    target: i32,
    plan: &RevertPlan,
) -> AppResult<(Option<String>, Vec<(String, String, Vec<u8>)>)> {
    let root = if plan.root_changes {
        root_content(script_uri, target).await?
    } else {
        None
    };

    let paths: Vec<String> = plan.writes.iter().map(|file| file.uri.clone()).collect();
    let contents = read_files(script_uri, target, &paths).await?;

    // Ordered by the plan rather than by whatever the map yields, so a revert
    // writes files in the order it reported them.
    let writes = plan
        .writes
        .iter()
        .filter_map(|file| {
            contents
                .get(&file.uri)
                .map(|(content, mimetype)| (file.uri.clone(), mimetype.clone(), content.clone()))
        })
        .collect();

    Ok((root, writes))
}

// ============================================================================
// Retention
// ============================================================================

/// What a pruning pass is allowed to remove.
///
/// Recording a revision on every write is what makes the history worth having,
/// and it is also what makes it grow without bound: an agent editing a script
/// through `PATCH /engine/assets` writes far more often than a person does.
/// The policy is the answer to "which of these will nobody want", and every
/// clause is a way of being wrong about that.
#[derive(Debug, Clone, Copy)]
pub struct Retention {
    /// Keep every revision younger than this.
    pub keep_days: i32,
    /// Keep this many of the newest per script regardless of age, so a script
    /// nobody has touched in a year still has a history when someone returns
    /// to it.
    pub keep_per_script: i32,
    /// How long a blob is left alone after the last write that cited it.
    ///
    /// Not a retention policy so much as the width of the window in which a
    /// collector must not act — see the delete in [`prune`]. Wide on purpose:
    /// an uncollected orphan costs one pass of waiting, and collecting one too
    /// early costs a write its revision.
    pub blob_grace_secs: f64,
}

impl Default for Retention {
    fn default() -> Self {
        Self {
            keep_days: 30,
            keep_per_script: 50,
            blob_grace_secs: 3600.0,
        }
    }
}

/// What a pruning pass removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PruneOutcome {
    pub revisions: u64,
    pub blobs: u64,
}

/// Delete revisions the policy does not protect, then the blobs nothing cites.
///
/// Three things are never deleted, whatever the policy says:
///
/// - a **labelled** revision, because a label is someone having said out loud
///   that this one is worth returning to;
/// - the newest revision that **initialised cleanly**, because it is the floor
///   a rollback lands on, and a retention policy that removed it would take
///   away the answer exactly when the question gets asked;
/// - the **newest** revision of every script, which `keep_per_script` covers
///   for any sane value and which the `>= 1` clamp guarantees for the rest.
///
/// Age and count are both required before anything goes: a revision has to be
/// older than `keep_days` *and* outside the newest `keep_per_script` of its
/// script. Either alone deletes history someone is plausibly still using.
/// `scope` restricts the pass to one script; `None` prunes every script, which
/// is what the background pruner does. An operator clearing out one script's
/// churn should not have to wait for the whole engine's turn.
pub async fn prune(scope: Option<&str>, retention: Retention) -> AppResult<PruneOutcome> {
    let revisions = prune_revisions(scope, retention).await?;
    // Collected separately, and after the revisions are committed. The sweep
    // is opportunistic — see [`collect_blobs`] — and a pass that cannot
    // collect must not undo the pruning that has already succeeded.
    let blobs = collect_blobs(retention).await;

    if revisions > 0 || blobs > 0 {
        tracing::info!(
            revisions = revisions,
            blobs = blobs,
            "Pruned script revision history"
        );
    }

    Ok(PruneOutcome { revisions, blobs })
}

async fn prune_revisions(scope: Option<&str>, retention: Retention) -> AppResult<u64> {
    let keep_days = retention.keep_days.max(0);
    let keep_per_script = retention.keep_per_script.max(1);
    let scope = scope.map(str::to_string);

    with_transaction(move |conn| {
        Box::pin(async move {
            // One instance prunes at a time. Two doing it together is not
            // unsafe — the deletes are idempotent — but they would scan the
            // same rows to find the same answer, and a collector is only worth
            // running if it is cheap.
            let acquired: bool = sqlx::query_scalar(
                "SELECT pg_try_advisory_xact_lock(hashtext('revision-prune' || COALESCE($1, '')))",
            )
            .bind(scope.as_deref())
            .fetch_one(&mut *conn)
            .await
            .map_err(|e| db_error("locking for prune", e))?;

            if !acquired {
                tracing::debug!("Another instance is pruning revisions; skipping this pass");
                return Ok(0);
            }

            // `good` is aggregated once rather than asked per row. As a
            // correlated subquery it ran an index lookup for every revision in
            // the table, which is fine at hundreds and needless at any size.
            Ok(sqlx::query(
                "WITH good AS (
                     SELECT script_uri, MAX(revision) AS revision
                     FROM script_revisions
                     WHERE init_ok IS TRUE AND ($3::text IS NULL OR script_uri = $3)
                     GROUP BY script_uri
                 ), ranked AS (
                     SELECT r.id,
                            r.script_uri,
                            r.revision,
                            r.label,
                            r.created_at,
                            row_number() OVER (
                                PARTITION BY r.script_uri ORDER BY r.revision DESC
                            ) AS recency
                     FROM script_revisions r
                     WHERE $3::text IS NULL OR r.script_uri = $3
                 )
                 DELETE FROM script_revisions
                 WHERE id IN (
                     SELECT ranked.id
                     FROM ranked
                     LEFT JOIN good ON good.script_uri = ranked.script_uri
                     WHERE ranked.label IS NULL
                       AND (good.revision IS NULL OR ranked.revision <> good.revision)
                       AND ranked.recency > $1
                       AND ranked.created_at < NOW() - make_interval(days => $2)
                 )",
            )
            .bind(keep_per_script)
            .bind(keep_days)
            .bind(scope.as_deref())
            .execute(&mut *conn)
            .await
            .map_err(|e| db_error("pruning revisions", e))?
            .rows_affected())
        })
    })
    .await
}

/// Delete content no revision cites any more.
///
/// Always the whole table, whatever script was pruned: blobs are shared across
/// revisions and across scripts, so "this script's revision is gone" says
/// nothing about whether its bytes are still someone else's.
///
/// `FOR UPDATE SKIP LOCKED` is what makes this safe to run while writes are
/// landing. Recording a revision touches every blob it cites before inserting
/// the manifest rows that reference them, so a blob being claimed is a blob
/// this transaction cannot lock — and skipping it is exactly right, because
/// the writer is about to make it referenced. Without the lock the collector
/// decided and deleted in two steps, and a writer arriving in between turned
/// the foreign key into a refusal that took the whole statement with it.
///
/// The age guard stays as a second line: a blob nothing has touched for an
/// hour is not one any in-flight write is about to claim.
///
/// Returns 0 rather than an error when it cannot run. Collection is
/// housekeeping; a pass that achieves nothing is not a failure worth
/// reporting, and whatever it skipped is collected by the next one.
async fn collect_blobs(retention: Retention) -> u64 {
    let grace = retention.blob_grace_secs.max(0.0);
    let result = with_transaction(move |conn| {
        Box::pin(async move {
            sqlx::query(
                "WITH candidates AS (
                     SELECT b.sha256 FROM asset_blobs b
                     WHERE b.created_at < NOW() - make_interval(secs => $1)
                       AND NOT EXISTS (
                           SELECT 1 FROM script_revision_files f WHERE f.sha256 = b.sha256
                       )
                       AND NOT EXISTS (
                           SELECT 1 FROM script_revisions r WHERE r.root_sha256 = b.sha256
                       )
                     FOR UPDATE SKIP LOCKED
                 )
                 DELETE FROM asset_blobs
                 WHERE sha256 IN (SELECT sha256 FROM candidates)",
            )
            .bind(grace)
            .execute(conn)
            .await
            .map(|done| done.rows_affected())
            .map_err(|e| db_error("collecting revision blobs", e))
        })
    })
    .await;

    match result {
        Ok(collected) => collected,
        Err(e) => {
            tracing::debug!("Blob collection did not run this pass: {}", e);
            0
        }
    }
}

/// Run a pruning pass on an interval until shutdown.
///
/// Started alongside the scheduler worker and stopped by the same graceful
/// shutdown, so an engine that is going down is not left holding a half-run
/// collection.
///
/// Every instance runs this; the advisory lock in [`prune`] means only one of
/// them does the work on any given tick.
pub fn spawn_pruner(
    config: crate::config::RevisionsConfig,
    shutdown: tokio::sync::oneshot::Receiver<()>,
) {
    if !config.prune_enabled {
        tracing::info!("Revision pruning is disabled; history will grow without bound");
        return;
    }

    let retention = Retention {
        keep_days: config.retention_days.min(i32::MAX as u32) as i32,
        keep_per_script: config.keep_per_script.min(i32::MAX as u32) as i32,
        ..Retention::default()
    };
    let period = std::time::Duration::from_secs(config.prune_interval_secs.max(60));

    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(period);
        // The first tick fires immediately, which would put a prune in the
        // middle of startup — the moment an engine has least to spare and the
        // history has least to gain.
        ticker.tick().await;

        let mut shutdown = shutdown;
        loop {
            tokio::select! {
                _ = ticker.tick() => {
                    if let Err(e) = prune(None, retention).await {
                        tracing::warn!("Revision pruning pass failed: {}", e);
                    }
                }
                _ = &mut shutdown => {
                    tracing::debug!("Revision pruner stopping");
                    return;
                }
            }
        }
    });

    tracing::info!(
        retention_days = retention.keep_days,
        keep_per_script = retention.keep_per_script,
        interval_secs = period.as_secs(),
        "Revision pruner started"
    );
}

// ============================================================================
// Diffing
// ============================================================================

/// How long a diff may get before it stops being an answer.
///
/// A caller asking what changed between two revisions wants to read the
/// result. Past this the response is a data dump, and the files it could not
/// show are still listed by name so nothing goes missing silently.
const MAX_DIFF_BYTES: usize = 512 * 1024;

/// What happened to one file between two revisions.
#[derive(Debug, Clone)]
pub struct FileDiff {
    pub uri: String,
    /// `added`, `removed`, or `modified`.
    pub status: &'static str,
    pub from_sha256: Option<String>,
    pub to_sha256: Option<String>,
    /// Unified diff, or `None` when one could not be produced.
    pub diff: Option<String>,
    /// Why there is no diff, when there is none.
    pub note: Option<String>,
}

/// What changed between two revisions.
#[derive(Debug, Clone)]
pub struct RevisionDiff {
    pub from: i32,
    pub to: i32,
    pub files: Vec<FileDiff>,
    /// True when the ceiling stopped further diffs being rendered. The files
    /// are still listed; only their contents are missing.
    pub truncated: bool,
}

/// The root's path within the script, which is how its imports name it.
fn root_path(script_uri: &str) -> String {
    crate::module_loader::root_module_path(script_uri).unwrap_or_else(|_| script_uri.to_string())
}

/// What changed between two revisions of a script.
///
/// The counterpart to editing without sending a file. An agent that has just
/// rewritten four modules can ask what it actually changed instead of reading
/// all four back, and someone deciding whether to revert can see what they
/// would be undoing rather than inferring it from a list of digests.
///
/// Files whose digest is the same in both revisions are not read at all: the
/// blobs are shared, so equal digests are equal bytes and there is nothing to
/// render.
pub async fn diff(
    script_uri: &str,
    from: i32,
    to: i32,
    context_lines: usize,
) -> AppResult<Option<RevisionDiff>> {
    let Some(from_files) = files(script_uri, from).await? else {
        return Ok(None);
    };
    let Some(to_files) = files(script_uri, to).await? else {
        return Ok(None);
    };

    let root = root_path(script_uri);
    let from_root = get(script_uri, from).await?.map(|r| r.root_sha256);
    let to_root = get(script_uri, to).await?.map(|r| r.root_sha256);

    let index = |list: &[RevisionFile]| -> std::collections::BTreeMap<String, String> {
        list.iter()
            .map(|file| (file.uri.clone(), file.sha256.clone()))
            .collect()
    };
    let mut before = index(&from_files);
    let mut after = index(&to_files);

    // The root is a file of the script like any other from the caller's side —
    // it is what its imports are relative to — so it belongs in the same list
    // rather than in a section of its own.
    if let Some(sha) = from_root {
        before.insert(root.clone(), sha);
    }
    if let Some(sha) = to_root {
        after.insert(root.clone(), sha);
    }

    let mut paths: Vec<&String> = before.keys().chain(after.keys()).collect();
    paths.sort_unstable();
    paths.dedup();

    // Both sides of every file that moved, in one query each. The manifest
    // comparison above already said exactly which files those are, and asking
    // for them one at a time would give that saving straight back.
    let moved: Vec<String> = paths
        .iter()
        .filter(|path| before.get(**path) != after.get(**path))
        .filter(|path| ***path != root)
        .map(|path| (*path).clone())
        .collect();
    let old_files = read_files(script_uri, from, &moved).await?;
    let new_files = read_files(script_uri, to, &moved).await?;

    let mut rendered = 0usize;
    let mut truncated = false;
    let mut diffs = Vec::new();

    for path in paths {
        let from_sha = before.get(path);
        let to_sha = after.get(path);
        if from_sha == to_sha {
            continue;
        }

        let status = match (from_sha, to_sha) {
            (None, Some(_)) => "added",
            (Some(_), None) => "removed",
            _ => "modified",
        };

        let mut entry = FileDiff {
            uri: path.clone(),
            status,
            from_sha256: from_sha.cloned(),
            to_sha256: to_sha.cloned(),
            diff: None,
            note: None,
        };

        if rendered >= MAX_DIFF_BYTES {
            truncated = true;
            entry.note = Some("Diff omitted: the response reached its size ceiling".to_string());
            diffs.push(entry);
            continue;
        }

        let is_root = *path == root;
        let old_text = match from_sha {
            Some(_) => revision_text(script_uri, from, path, is_root, &old_files).await?,
            None => Some(String::new()),
        };
        let new_text = match to_sha {
            Some(_) => revision_text(script_uri, to, path, is_root, &new_files).await?,
            None => Some(String::new()),
        };

        match (old_text, new_text) {
            (Some(old), Some(new)) => {
                let rendered_diff = similar::TextDiff::from_lines(&old, &new)
                    .unified_diff()
                    .context_radius(context_lines)
                    .header(&format!("r{}", from), &format!("r{}", to))
                    .to_string();
                rendered = rendered.saturating_add(rendered_diff.len());
                entry.diff = Some(rendered_diff);
            }
            _ => {
                entry.note =
                    Some("No diff: the file is not UTF-8 text in one of the revisions".to_string());
            }
        }

        diffs.push(entry);
    }

    Ok(Some(RevisionDiff {
        from,
        to,
        files: diffs,
        truncated,
    }))
}

/// One file of a revision as text, or `None` when it is not UTF-8.
///
/// Takes the already-fetched contents rather than reading again; only the root
/// is read here, since it lives outside the manifest.
async fn revision_text(
    script_uri: &str,
    revision: i32,
    path: &str,
    is_root: bool,
    fetched: &std::collections::HashMap<String, (Vec<u8>, String)>,
) -> AppResult<Option<String>> {
    if is_root {
        return root_content(script_uri, revision).await;
    }
    Ok(fetched
        .get(path)
        .and_then(|(content, _)| String::from_utf8(content.clone()).ok()))
}

// ============================================================================
// What this instance is running
// ============================================================================

/// The revision each script is at, as far as this instance is concerned.
///
/// Deliberately not a lookup of `MAX(revision)` per use. A log line asks this
/// question, and a query per line would cost more than writing the line does.
///
/// It is also the more honest answer. What a log line should record is the
/// version whose code produced it — which is the version *this* instance is
/// running, not necessarily the newest one stored. An instance that has not
/// yet processed another node's write is still serving the older revision, and
/// its output belongs to that one.
static CURRENT: std::sync::OnceLock<std::sync::RwLock<std::collections::HashMap<String, i32>>> =
    std::sync::OnceLock::new();

fn current_map() -> &'static std::sync::RwLock<std::collections::HashMap<String, i32>> {
    CURRENT.get_or_init(|| std::sync::RwLock::new(std::collections::HashMap::new()))
}

/// The revision `script_uri` is running here, if it is known.
pub fn current(script_uri: &str) -> Option<i32> {
    current_map()
        .read()
        .ok()
        .and_then(|map| map.get(script_uri).copied())
}

/// Record that this instance is now running `revision` of `script_uri`.
pub fn remember_current(script_uri: &str, revision: i32) {
    if let Ok(mut map) = current_map().write() {
        map.insert(script_uri.to_string(), revision);
    }
}

/// Forget a script, because it is gone.
pub fn forget_current(script_uri: &str) {
    if let Ok(mut map) = current_map().write() {
        map.remove(script_uri);
    }
}

/// Learn `script_uri`'s newest revision from the database and remember it.
///
/// For the paths that pick up someone else's write — a notification from
/// another instance — where this instance only now starts running the new
/// code.
pub async fn refresh_current(script_uri: &str) {
    match head(script_uri).await {
        Ok(Some(revision)) => remember_current(script_uri, revision),
        Ok(None) => forget_current(script_uri),
        Err(e) => tracing::debug!(
            script = script_uri,
            "Could not refresh the current revision: {}",
            e
        ),
    }
}

/// Learn every script's newest revision in one query.
///
/// Run at startup, so lines logged before a script's first write of this boot
/// are still attributed. One query rather than one per script, for the same
/// reason the baseline backfill is one pass.
pub async fn load_current() {
    let result = with_read_connection(move |conn| {
        Box::pin(async move {
            sqlx::query(
                "SELECT script_uri, MAX(revision) AS revision
                 FROM script_revisions GROUP BY script_uri",
            )
            .fetch_all(conn)
            .await
            .map_err(|e| db_error("loading current revisions", e))
        })
    })
    .await;

    match result {
        Ok(rows) => {
            if let Ok(mut map) = current_map().write() {
                for row in rows {
                    let uri: String = row.get("script_uri");
                    let revision: Option<i32> = row.get("revision");
                    if let Some(revision) = revision {
                        map.insert(uri, revision);
                    }
                }
            }
        }
        Err(e) => tracing::warn!("Could not load current revisions: {}", e),
    }
}

// ============================================================================
// Schema fingerprints
// ============================================================================

/// The script's table schemas as they stand, keyed by logical table name.
async fn read_table_schemas(
    conn: &mut PgConnection,
    script_uri: &str,
) -> AppResult<serde_json::Map<String, serde_json::Value>> {
    let rows = sqlx::query(
        "SELECT logical_table_name, schema_json FROM script_tables
         WHERE script_uri = $1 ORDER BY logical_table_name",
    )
    .bind(script_uri)
    .fetch_all(&mut *conn)
    .await
    .map_err(|e| db_error("reading script table schemas", e))?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let name: String = row.get("logical_table_name");
            let schema: serde_json::Value = row.get("schema_json");
            (name, schema)
        })
        .collect())
}

/// The table schemas a revision recorded, or `None` for one recorded before
/// revisions carried them.
pub async fn schema_at(script_uri: &str, revision: i32) -> AppResult<Option<serde_json::Value>> {
    let script_uri = script_uri.to_string();
    with_read_connection(move |conn| {
        Box::pin(async move {
            sqlx::query_scalar(
                "SELECT tables FROM script_revisions WHERE script_uri = $1 AND revision = $2",
            )
            .bind(&script_uri)
            .bind(revision)
            .fetch_optional(conn)
            .await
            .map(Option::flatten)
            .map_err(|e| db_error("reading revision schema", e))
        })
    })
    .await
}

/// The script's table schemas right now.
pub async fn schema_now(script_uri: &str) -> AppResult<serde_json::Value> {
    let script_uri = script_uri.to_string();
    with_read_connection(move |conn| {
        Box::pin(async move {
            read_table_schemas(conn, &script_uri)
                .await
                .map(serde_json::Value::Object)
        })
    })
    .await
}

fn column_names(table: &serde_json::Value) -> Vec<String> {
    table
        .get("columns")
        .and_then(|columns| columns.as_array())
        .map(|columns| {
            columns
                .iter()
                .filter_map(|column| column.get("name").and_then(|name| name.as_str()))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// How the script's tables now differ from what `target` recorded.
///
/// Advisory, always. Restoring code to an earlier version says nothing about
/// what should happen to the data that version's successors wrote, and a
/// revert that dropped a column to match old code would destroy data in order
/// to restore code. What the engine can do is say what it noticed, and leave
/// the data to whoever knows what it is for.
///
/// An empty list means the schema is the one this revision ran against. A
/// revision recorded before schemas were captured reports that instead of
/// pretending to have compared.
pub fn compare_schema(
    revision: i32,
    recorded: Option<&serde_json::Value>,
    current: &serde_json::Value,
) -> Vec<String> {
    let Some(recorded) = recorded.and_then(|value| value.as_object()) else {
        return vec![format!(
            "Revision {} predates schema recording, so what its modules expect of the \
             database cannot be compared with what is there now",
            revision
        )];
    };
    let Some(current) = current.as_object() else {
        return Vec::new();
    };

    let mut warnings = Vec::new();

    for (name, then) in recorded {
        let Some(now) = current.get(name) else {
            // The most serious case: the restored modules read a table that is
            // not there at all.
            warnings.push(format!(
                "Table `{}` existed at revision {} and does not now; its modules will \
                 not find it",
                name, revision
            ));
            continue;
        };

        let then_columns = column_names(then);
        let now_columns = column_names(now);

        for column in &then_columns {
            if !now_columns.contains(column) {
                warnings.push(format!(
                    "Table `{}` is missing column `{}`, which revision {} expects",
                    name, column, revision
                ));
            }
        }
        for column in &now_columns {
            if !then_columns.contains(column) {
                warnings.push(format!(
                    "Table `{}` has column `{}`, added after revision {}; that revision's \
                     modules do not write it",
                    name, column, revision
                ));
            }
        }
    }

    for name in current.keys() {
        if !recorded.contains_key(name) {
            warnings.push(format!(
                "Table `{}` did not exist at revision {}; nothing in that revision reads it",
                name, revision
            ));
        }
    }

    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn table(columns: &[&str]) -> serde_json::Value {
        json!({
            "columns": columns
                .iter()
                .map(|name| json!({ "name": name, "type": "TEXT" }))
                .collect::<Vec<_>>()
        })
    }

    #[test]
    fn an_unchanged_schema_warns_about_nothing() {
        let recorded = json!({ "matches": table(&["id", "score"]) });
        let current = json!({ "matches": table(&["id", "score"]) });

        assert!(compare_schema(41, Some(&recorded), &current).is_empty());
    }

    #[test]
    fn a_column_added_since_is_reported_as_one_the_revision_does_not_write() {
        let recorded = json!({ "matches": table(&["id"]) });
        let current = json!({ "matches": table(&["id", "round"]) });

        let warnings = compare_schema(41, Some(&recorded), &current);
        assert_eq!(warnings.len(), 1, "{:?}", warnings);
        assert!(warnings[0].contains("`round`"), "{}", warnings[0]);
        assert!(
            warnings[0].contains("added after revision 41"),
            "{}",
            warnings[0]
        );
    }

    #[test]
    fn a_column_the_revision_expects_and_cannot_find_is_reported() {
        let recorded = json!({ "matches": table(&["id", "score"]) });
        let current = json!({ "matches": table(&["id"]) });

        let warnings = compare_schema(41, Some(&recorded), &current);
        assert_eq!(warnings.len(), 1, "{:?}", warnings);
        assert!(
            warnings[0].contains("missing column `score`"),
            "{}",
            warnings[0]
        );
    }

    #[test]
    fn a_table_that_is_gone_is_the_one_worth_saying_loudest() {
        let recorded = json!({ "matches": table(&["id"]) });
        let current = json!({});

        let warnings = compare_schema(41, Some(&recorded), &current);
        assert_eq!(warnings.len(), 1, "{:?}", warnings);
        assert!(warnings[0].contains("does not now"), "{}", warnings[0]);
    }

    #[test]
    fn a_table_added_since_is_reported_as_one_nothing_reads() {
        let recorded = json!({});
        let current = json!({ "rounds": table(&["id"]) });

        let warnings = compare_schema(41, Some(&recorded), &current);
        assert_eq!(warnings.len(), 1, "{:?}", warnings);
        assert!(
            warnings[0].contains("did not exist at revision 41"),
            "{}",
            warnings[0]
        );
    }

    #[test]
    fn a_revision_with_no_fingerprint_says_so_rather_than_claiming_a_match() {
        let current = json!({ "matches": table(&["id"]) });

        let warnings = compare_schema(41, None, &current);
        assert_eq!(warnings.len(), 1, "{:?}", warnings);
        assert!(
            warnings[0].contains("predates schema recording"),
            "reporting 'no differences' for a revision that recorded nothing would be \
             a claim the engine cannot make: {}",
            warnings[0]
        );
    }
}
