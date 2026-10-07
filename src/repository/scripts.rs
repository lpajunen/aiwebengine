//! Script rows: the entrypoint, ownership, host bindings, init status, rename and delete.

use super::*;
use crate::error::{AppError, AppResult};
use crate::scheduler;
use sqlx::Row;
use std::collections::HashMap;
use std::time::SystemTime;
use tracing::{debug, error, warn};

/// Script metadata for tracking initialization status and registrations
#[derive(Debug, Clone)]
pub struct ScriptMetadata {
    pub uri: String,
    pub name: Option<String>,
    pub content: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub initialized: bool,
    pub init_error: Option<String>,
    pub last_init_time: Option<SystemTime>,
    /// Cached route registrations from init() function
    pub registrations: RouteRegistrations,
    pub owners: Vec<String>,
    /// Hosts this script's registrations are published on, as stored in
    /// `script_hosts`. Empty means the default host; a `*` entry means every
    /// configured host. Resolve with [`crate::hosts::effective_hosts`] rather
    /// than reading it directly.
    pub hosts: Vec<String>,
}

impl ScriptMetadata {
    /// Create a new script metadata instance
    pub fn new(uri: String, content: String) -> Self {
        let now = SystemTime::now();
        // Extract name from URI (last segment after /)
        let name = uri.rsplit('/').next().map(String::from);
        Self {
            uri,
            name,
            content,
            created_at: now,
            updated_at: now,
            initialized: false,
            init_error: None,
            last_init_time: None,
            registrations: HashMap::new(),
            owners: Vec::new(),
            hosts: Vec::new(),
        }
    }

    /// Mark script as initialized successfully
    pub fn mark_initialized(&mut self) {
        self.initialized = true;
        self.init_error = None;
        self.last_init_time = Some(SystemTime::now());
    }

    /// Mark script as initialized successfully with registrations
    pub fn mark_initialized_with_registrations(&mut self, registrations: RouteRegistrations) {
        self.initialized = true;
        self.init_error = None;
        self.last_init_time = Some(SystemTime::now());
        self.registrations = registrations;
    }

    /// Mark script initialization as failed
    pub fn mark_init_failed(&mut self, error: String) {
        self.initialized = false;
        self.init_error = Some(error);
        self.last_init_time = Some(SystemTime::now());
    }

    /// Update script content, keeping the route registrations installed by the
    /// last successful `init()`.
    ///
    /// Registrations live only in this in-memory metadata, and routing skips
    /// any script whose registrations are empty (see `route_index::build_index`).
    /// Clearing them here made every route of a script 404 from the moment its
    /// source was upserted until the re-init that follows finished — seconds for
    /// a small script, indefinitely for one whose init() times out. Keeping the
    /// previous table means a deploy serves the *new* source through the *old*
    /// route map for that window, and `update_script_init_status` swaps in the
    /// new map atomically when init() succeeds.
    pub fn update_content(&mut self, new_content: String) {
        self.content = new_content;
        self.updated_at = SystemTime::now();
        // The pending init() has not run against this source yet; `init_error`
        // describes the previous one, so drop it. `initialized` and
        // `registrations` stay as-is so routing keeps working meanwhile.
        self.init_error = None;
    }
}

/// The root module names, as a bindable array.
///
/// The order is significant: a tree holding both `main.ts` and `main.js` has
/// one root, and it is the first of [`crate::module_loader::ROOT_MODULE_NAMES`]
/// it holds. `array_position` against this array is how the SQL below says the
/// same thing.
pub(super) fn root_names() -> Vec<String> {
    crate::module_loader::ROOT_MODULE_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .collect()
}

/// Which file of `uri`'s tree is its root module, or `None` when it holds none.
pub(super) async fn db_root_path<'e, E>(executor: E, uri: &str) -> AppResult<Option<String>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    sqlx::query_scalar(
        r#"
        SELECT uri FROM assets
        WHERE script_uri = $1 AND uri = ANY($2::text[])
        ORDER BY array_position($2::text[], uri)
        LIMIT 1
        "#,
    )
    .bind(uri)
    .bind(root_names())
    .fetch_optional(executor)
    .await
    .map_err(|e| {
        error!("Database error resolving root module for {}: {}", uri, e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })
}

/// A root's stored bytes as source text.
///
/// The root lives in `assets.content`, which is `BYTEA`, so it can hold bytes
/// that are not text — a caller may write
/// anything to any path in the tree, including this one. That is reported
/// rather than lost: a lossy decode would hand the bundler a root with
/// replacement characters in it and blame the resulting syntax error on the
/// author.
pub(super) fn root_source(uri: &str, bytes: Vec<u8>) -> AppResult<String> {
    String::from_utf8(bytes).map_err(|_| AppError::Database {
        message: format!(
            "The root module of '{}' is not valid UTF-8, so it is not source text",
            uri
        ),
        source: None,
    })
}

/// Create the script row if it is not there.
///
/// The identity half of [`db_upsert_script`], for a caller whose content is
/// arriving as ordinary files of the tree rather than as a root to write here.
pub(super) async fn db_ensure_script_row(
    mut executor: crate::database::TransactionExecutor<'_>,
    uri: &str,
) -> AppResult<()> {
    let now = chrono::Utc::now();
    let name = uri.rsplit('/').next().unwrap_or(uri);

    const ENSURE: &str = r#"
        INSERT INTO scripts (uri, name, created_at, updated_at)
        VALUES ($1, $2, $3, $3)
        ON CONFLICT (uri) DO NOTHING
        "#;

    match executor {
        crate::database::TransactionExecutor::Transaction(ref mut tx) => {
            sqlx::query(ENSURE)
                .bind(uri)
                .bind(name)
                .bind(now)
                .execute(&mut ***tx)
                .await
        }
        crate::database::TransactionExecutor::Pool(pool) => {
            sqlx::query(ENSURE)
                .bind(uri)
                .bind(name)
                .bind(now)
                .execute(pool)
                .await
        }
    }
    .map_err(store_error)?;

    Ok(())
}

/// Database-backed upsert script.
///
/// Two statements rather than one, because a script is two things: the
/// `scripts` row that *is* the script — its identity, the row every foreign
/// key points at — and the file in its tree that holds its source. The row has
/// to exist before the file, since `assets.script_uri` references it.
///
/// Each statement is an upsert because an UPDATE
/// followed by an INSERT when it matched nothing are not a unit, and between
/// them another instance creating the same script leaves a row where the
/// UPDATE found none.
pub(super) async fn db_upsert_script(
    executor: crate::database::TransactionExecutor<'_>,
    uri: &str,
    content: &str,
    root: Option<&str>,
) -> AppResult<()> {
    debug!(
        "db_upsert_script called: uri={}, root={:?}, content_len={}",
        uri,
        root,
        content.len()
    );
    if let Some(name) = root
        && !crate::module_loader::is_root_module_name(name)
    {
        return Err(RepositoryError::InvalidData(format!(
            "'{}' is not an entrypoint name (one of {})",
            name,
            crate::module_loader::ROOT_MODULE_NAMES.join(", ")
        ))
        .into());
    }

    // Two statements, three when the caller names the entrypoint: one unit
    // either way, so a pool caller gets a transaction of its own rather than
    // a window in which the script has two entrypoints or none.
    match executor {
        crate::database::TransactionExecutor::Transaction(tx) => {
            upsert_script_rows(tx, uri, content, root).await?;
        }
        crate::database::TransactionExecutor::Pool(pool) => {
            let mut tx = pool.begin().await.map_err(store_error)?;
            upsert_script_rows(&mut tx, uri, content, root).await?;
            tx.commit().await.map_err(store_error)?;
        }
    }

    debug!("✓ Successfully stored script in database: {}", uri);
    Ok(())
}

/// The statements behind [`db_upsert_script`], on one connection.
///
/// `name` is only filled in when the row has none, which is what the UPDATE
/// did with `COALESCE(name, $4)`: it is a display label a caller may have
/// customised, and a redeploy is not a reason to overwrite it.
///
/// Which file the source lands in:
///
/// - **Named** (`root` is `Some`): exactly that file, and every other
///   entrypoint name is removed. Naming the file is how an author says what
///   language the entrypoint is in — writing `main.ts` into a script that has
///   `main.js` is a rename, not a second program, and resolution would
///   otherwise keep whichever sorts first.
/// - **Unnamed**: whichever entrypoint the tree already holds, and the name
///   derived from the URI when it holds none. This is what a caller that
///   sends only source (`/engine/write_file`, a batch's `content`) means.
pub(super) async fn upsert_script_rows(
    conn: &mut sqlx::PgConnection,
    uri: &str,
    content: &str,
    root: Option<&str>,
) -> AppResult<()> {
    let now = chrono::Utc::now();

    // Extract name from URI (last segment after /)
    let name = uri.rsplit('/').next().unwrap_or(uri);

    sqlx::query(
        r#"
        INSERT INTO scripts (uri, name, created_at, updated_at)
        VALUES ($1, $2, $3, $3)
        ON CONFLICT (uri) DO UPDATE
        SET updated_at = EXCLUDED.updated_at,
            name = COALESCE(scripts.name, EXCLUDED.name)
        "#,
    )
    .bind(uri)
    .bind(name)
    .bind(now)
    .execute(&mut *conn)
    .await
    .map_err(store_error)?;

    let target = root.unwrap_or_else(|| crate::module_loader::default_root_module_name(uri));
    let mimetype = if target.ends_with(".ts") || target.ends_with(".tsx") {
        "text/typescript"
    } else {
        "text/javascript"
    };

    if let Some(root) = root {
        sqlx::query(
            r#"
            DELETE FROM assets
            WHERE script_uri = $1 AND uri = ANY($2::text[]) AND uri <> $3
            "#,
        )
        .bind(uri)
        .bind(root_names())
        .bind(root)
        .execute(&mut *conn)
        .await
        .map_err(store_error)?;

        sqlx::query(
            r#"
            INSERT INTO assets (script_uri, uri, mimetype, content, created_at, updated_at)
            VALUES ($1, $2, $3, $4, $5, $5)
            ON CONFLICT (script_uri, uri) DO UPDATE
            SET content = EXCLUDED.content,
                mimetype = EXCLUDED.mimetype,
                updated_at = EXCLUDED.updated_at
            "#,
        )
        .bind(uri)
        .bind(root)
        .bind(mimetype)
        .bind(content.as_bytes())
        .bind(now)
        .execute(&mut *conn)
        .await
        .map_err(store_error)?;
        return Ok(());
    }

    // The root goes to whichever root name the tree already holds, and to the
    // one derived from the URI when it holds none. Resolving the target inside
    // the statement rather than in a read before it keeps the write one
    // round trip and leaves no window in which the answer changes: a
    // concurrent writer resolves the same way, because both would derive the
    // same default from the same URI.
    //
    // `mimetype` is set on insert and left alone on conflict. The conflict
    // arm may be updating a `main.ts` while `$4` describes the `main.js` the
    // default would have created, and the stored row already says what it is.
    sqlx::query(
        r#"
        INSERT INTO assets (script_uri, uri, mimetype, content, created_at, updated_at)
        SELECT $1,
               COALESCE(
                   (SELECT a.uri FROM assets a
                    WHERE a.script_uri = $1 AND a.uri = ANY($2::text[])
                    ORDER BY array_position($2::text[], a.uri)
                    LIMIT 1),
                   $3),
               $4, $5, $6, $6
        ON CONFLICT (script_uri, uri) DO UPDATE
        SET content = EXCLUDED.content,
            updated_at = EXCLUDED.updated_at
        "#,
    )
    .bind(uri)
    .bind(root_names())
    .bind(target)
    .bind(mimetype)
    .bind(content.as_bytes())
    .bind(now)
    .execute(&mut *conn)
    .await
    .map_err(store_error)?;
    Ok(())
}

pub(super) fn store_error(e: sqlx::Error) -> AppError {
    error!("Database error storing script: {}", e);
    AppError::Database {
        message: format!("Database error: {}", e),
        source: None,
    }
}

/// Database-backed get script.
///
/// `None` means the script is not there. A script whose tree holds no root
/// module answers with empty source rather than with absence: the script
/// exists, is owned by someone, has a history and is listed in the editor —
/// it just has nothing to run, which is a state to show them rather than one
/// to hide by making the script disappear.
pub(super) async fn db_get_script<'e, E>(executor: E, uri: &str) -> AppResult<Option<String>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let row = sqlx::query(
        r#"
        SELECT (
            SELECT a.content FROM assets a
            WHERE a.script_uri = s.uri AND a.uri = ANY($2::text[])
            ORDER BY array_position($2::text[], a.uri)
            LIMIT 1
        ) AS content
        FROM scripts s WHERE s.uri = $1
        "#,
    )
    .bind(uri)
    .bind(root_names())
    .fetch_optional(executor)
    .await
    .map_err(|e| {
        error!("Database error getting script: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    match row {
        Some(row) => {
            let content: Option<Vec<u8>> = row.try_get("content").map_err(|e| {
                error!("Database error getting content: {}", e);
                AppError::Database {
                    message: format!("Database error: {}", e),
                    source: None,
                }
            })?;
            match content {
                Some(bytes) => root_source(uri, bytes).map(Some),
                None => Ok(Some(String::new())),
            }
        }
        None => Ok(None),
    }
}

/// Database-backed list all scripts
pub(super) async fn db_list_scripts<'e, E>(executor: E) -> AppResult<HashMap<String, String>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows = sqlx::query(
        r#"
        SELECT s.uri, (
            SELECT a.content FROM assets a
            WHERE a.script_uri = s.uri AND a.uri = ANY($1::text[])
            ORDER BY array_position($1::text[], a.uri)
            LIMIT 1
        ) AS content
        FROM scripts s ORDER BY s.uri
        "#,
    )
    .bind(root_names())
    .fetch_all(executor)
    .await
    .map_err(|e| {
        error!("Database error listing scripts: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let mut scripts = HashMap::new();
    for row in rows {
        let uri: String = row.try_get("uri").map_err(|e| {
            error!("Database error getting uri: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
        let content: Option<Vec<u8>> = row.try_get("content").map_err(|e| {
            error!("Database error getting content: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
        // One script whose root is not text must not stop the listing that
        // boots every other one. It is reported and left with no source, which
        // is the same state a script with no root module is in.
        let content = match content {
            Some(bytes) => root_source(&uri, bytes).unwrap_or_else(|e| {
                warn!("{}", e);
                String::new()
            }),
            None => String::new(),
        };
        scripts.insert(uri, content);
    }

    Ok(scripts)
}

/// Database-backed delete script
pub(super) async fn db_delete_script<'e, E>(executor: E, uri: &str) -> AppResult<bool>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let result = sqlx::query(
        r#"
        DELETE FROM scripts WHERE uri = $1
        "#,
    )
    .bind(uri)
    .execute(executor)
    .await
    .map_err(|e| {
        error!("Database error deleting script: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let existed = result.rows_affected() > 0;
    if existed {
        debug!("Deleted script from database: {}", uri);
    } else {
        debug!("Script not found in database for deletion: {}", uri);
    }

    Ok(existed)
}

/// Database-backed add script owner
pub(super) async fn db_add_script_owner<'e, E>(
    executor: E,
    uri: &str,
    user_id: &str,
) -> AppResult<()>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    // Insert ownership record (ignore if already exists)
    sqlx::query(
        r#"
        INSERT INTO script_owners (script_uri, user_id, created_at)
        VALUES ($1, $2, NOW())
        ON CONFLICT (script_uri, user_id) DO NOTHING
        "#,
    )
    .bind(uri)
    .bind(user_id)
    .execute(executor)
    .await
    .map_err(|e| {
        error!("Database error adding script owner: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    debug!("Added owner {} to script {}", user_id, uri);
    Ok(())
}

/// Database-backed remove script owner
pub(super) async fn db_remove_script_owner<'e, E>(
    executor: E,
    uri: &str,
    user_id: &str,
) -> AppResult<bool>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let result = sqlx::query(
        r#"
        DELETE FROM script_owners WHERE script_uri = $1 AND user_id = $2
        "#,
    )
    .bind(uri)
    .bind(user_id)
    .execute(executor)
    .await
    .map_err(|e| {
        error!("Database error removing script owner: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let existed = result.rows_affected() > 0;
    if existed {
        debug!("Removed owner {} from script {}", user_id, uri);
    } else {
        debug!("Owner {} was not found for script {}", user_id, uri);
    }

    Ok(existed)
}

/// Database-backed get script owners
pub(super) async fn db_get_script_owners<'e, E>(executor: E, uri: &str) -> AppResult<Vec<String>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows = sqlx::query(
        r#"
        SELECT user_id
        FROM script_owners
        WHERE script_uri = $1
        ORDER BY created_at ASC
        "#,
    )
    .bind(uri)
    .fetch_all(executor)
    .await
    .map_err(|e| {
        error!("Database error getting script owners: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let owners = rows
        .into_iter()
        .map(|row| {
            row.try_get("user_id").map_err(|e| {
                error!("Database error parsing user_id: {}", e);
                AppError::Database {
                    message: format!("Database error: {}", e),
                    source: None,
                }
            })
        })
        .collect::<Result<Vec<String>, AppError>>()?;

    Ok(owners)
}

/// Database-backed check if user owns script
pub(super) async fn db_user_owns_script<'e, E>(
    executor: E,
    uri: &str,
    user_id: &str,
) -> AppResult<bool>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let row = sqlx::query(
        r#"
        SELECT EXISTS(
            SELECT 1
            FROM script_owners
            WHERE script_uri = $1 AND user_id = $2
        ) as owns
        "#,
    )
    .bind(uri)
    .bind(user_id)
    .fetch_one(executor)
    .await
    .map_err(|e| {
        error!("Database error checking script ownership: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let owns: bool = row.try_get("owns").map_err(|e| {
        error!("Database error parsing ownership check: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    Ok(owns)
}

/// Database-backed count script owners
pub(super) async fn db_count_script_owners<'e, E>(executor: E, uri: &str) -> AppResult<i64>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let row = sqlx::query(
        r#"
        SELECT COUNT(*) as count
        FROM script_owners
        WHERE script_uri = $1
        "#,
    )
    .bind(uri)
    .fetch_one(executor)
    .await
    .map_err(|e| {
        error!("Database error counting script owners: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let count: i64 = row.try_get("count").map_err(|e| {
        error!("Database error parsing owner count: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    Ok(count)
}

/// Database-backed get all script owners (returns HashMap of uri -> owners)
pub(super) async fn db_get_all_script_owners<'e, E>(
    executor: E,
) -> AppResult<HashMap<String, Vec<String>>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows = sqlx::query(
        r#"
        SELECT script_uri, user_id
        FROM script_owners
        ORDER BY script_uri, created_at ASC
        "#,
    )
    .fetch_all(executor)
    .await
    .map_err(|e| {
        error!("Database error getting all script owners: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let mut owners_map: HashMap<String, Vec<String>> = HashMap::new();
    for row in rows {
        let uri: String = row.try_get("script_uri").map_err(|e| {
            error!("Database error parsing script_uri: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
        let user_id: String = row.try_get("user_id").map_err(|e| {
            error!("Database error parsing user_id: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;

        owners_map.entry(uri).or_default().push(user_id);
    }

    Ok(owners_map)
}

/// Database-backed get the hosts a script is bound to
pub(super) async fn db_get_script_hosts<'e, E>(executor: E, uri: &str) -> AppResult<Vec<String>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows = sqlx::query(
        r#"
        SELECT host
        FROM script_hosts
        WHERE script_uri = $1
        ORDER BY host ASC
        "#,
    )
    .bind(uri)
    .fetch_all(executor)
    .await
    .map_err(|e| {
        error!("Database error getting script hosts: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    rows.into_iter()
        .map(|row| {
            row.try_get("host").map_err(|e| {
                error!("Database error parsing host: {}", e);
                AppError::Database {
                    message: format!("Database error: {}", e),
                    source: None,
                }
            })
        })
        .collect()
}

/// Database-backed get all script host bindings (uri -> hosts)
/// When each script was first stored, by URI.
///
/// The route index ranks scripts by this: the one that was here first keeps a
/// contested path. `ScriptMetadata::new` stamps the moment the metadata was
/// *loaded*, which on every restart is the same instant for every script and
/// says nothing about their order.
pub(super) async fn db_get_all_script_created<'e, E>(
    executor: E,
) -> AppResult<HashMap<String, std::time::SystemTime>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows = sqlx::query("SELECT uri, created_at FROM scripts")
        .fetch_all(executor)
        .await
        .map_err(|e| {
            error!("Database error getting script creation times: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;

    let mut created = HashMap::new();
    for row in rows {
        let uri: String = row.try_get("uri").map_err(|e| AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        })?;
        let at: chrono::DateTime<chrono::Utc> =
            row.try_get("created_at").map_err(|e| AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            })?;
        created.insert(uri, at.into());
    }
    Ok(created)
}

pub(super) async fn db_get_all_script_hosts<'e, E>(
    executor: E,
) -> AppResult<HashMap<String, Vec<String>>>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let rows = sqlx::query(
        r#"
        SELECT script_uri, host
        FROM script_hosts
        ORDER BY script_uri, host ASC
        "#,
    )
    .fetch_all(executor)
    .await
    .map_err(|e| {
        error!("Database error getting all script hosts: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    let mut hosts_map: HashMap<String, Vec<String>> = HashMap::new();
    for row in rows {
        let uri: String = row.try_get("script_uri").map_err(|e| {
            error!("Database error parsing script_uri: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
        let host: String = row.try_get("host").map_err(|e| {
            error!("Database error parsing host: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
        hosts_map.entry(uri).or_default().push(host);
    }

    Ok(hosts_map)
}

/// Database-backed replace of a script's host bindings.
///
/// Replaces rather than merges: the caller states the complete set, so an
/// empty list clears the bindings and returns the script to the default host.
pub(super) async fn db_set_script_hosts(
    mut executor: crate::database::TransactionExecutor<'_>,
    uri: &str,
    hosts: &[String],
) -> AppResult<()> {
    let delete = sqlx::query("DELETE FROM script_hosts WHERE script_uri = $1").bind(uri);
    match executor {
        crate::database::TransactionExecutor::Transaction(ref mut tx) => {
            delete.execute(&mut ***tx).await
        }
        crate::database::TransactionExecutor::Pool(pool) => delete.execute(pool).await,
    }
    .map_err(|e| {
        error!("Database error clearing script hosts: {}", e);
        AppError::Database {
            message: format!("Database error: {}", e),
            source: None,
        }
    })?;

    for host in hosts {
        let insert = sqlx::query(
            r#"
            INSERT INTO script_hosts (script_uri, host, created_at)
            VALUES ($1, $2, NOW())
            ON CONFLICT (script_uri, host) DO NOTHING
            "#,
        )
        .bind(uri)
        .bind(host);

        match executor {
            crate::database::TransactionExecutor::Transaction(ref mut tx) => {
                insert.execute(&mut ***tx).await
            }
            crate::database::TransactionExecutor::Pool(pool) => insert.execute(pool).await,
        }
        .map_err(|e| {
            error!("Database error inserting script host: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        })?;
    }

    Ok(())
}

/// Fetch scripts from repository with proper error handling
pub fn fetch_scripts() -> HashMap<String, String> {
    let repo = get_repository();

    // Use run_blocking to call async repository method
    let result = run_bounded(async { repo.list_scripts().await });

    match result {
        Ok(scripts) => {
            debug!("Loaded {} scripts from repository", scripts.len());
            scripts
        }
        Err(e) => {
            error!("Failed to fetch scripts: {}", e);
            HashMap::new()
        }
    }
}

/// Fetch a single script by URI with proper error handling.
///
/// Fast path: the in-memory metadata cache (`DYNAMIC_SCRIPTS`) already holds
/// each script's source `content`. It is populated when the route index is
/// (re)built via `get_all_script_metadata` and evicted on every upsert/delete —
/// locally and through the cross-instance notification handlers — so a present
/// entry is authoritative. Serving content from it avoids a Postgres round-trip
/// on the hot request path.
///
/// The cache is bypassed while a transaction is active so a script still reads
/// its own uncommitted writes (an in-flight upsert has already evicted the
/// entry, so a concurrent reader without the transaction correctly falls through
/// to committed state).
pub fn fetch_script(uri: &str) -> Option<String> {
    if !crate::database::get_current_transaction_active()
        && let Ok(guard) = safe_lock_scripts()
        && let Some(metadata) = guard.get(uri)
    {
        return Some(metadata.content.clone());
    }

    // The database holds head. For a pinned script that is not what it serves,
    // and this fallback is reached exactly when the cache cannot say otherwise
    // — a cold instance, or a read inside a transaction. Returning head here
    // would pair a pinned script's modules with a root from a version they
    // were never written for, which is the failure pinning exists to prevent
    // and the hardest kind to notice.
    let result = run_bounded(async { Ok(read_served_source(uri).await) });

    match result {
        Ok(Some(script)) => {
            debug!("Loaded script from repository: {}", uri);
            Some(script)
        }
        Ok(None) => None,
        Err(e) => {
            warn!("Failed to fetch script {}: {}", uri, e);
            None
        }
    }
}

/// Which file of `script_uri`'s tree is its root module.
///
/// `None` when the tree holds none of the root names, which is a script that
/// has not been written yet or one whose entrypoint was removed. Callers that
/// need a name regardless go through
/// [`crate::module_loader::root_module_path_in`], which falls back to the name
/// a first write would use.
pub fn find_root_asset(script_uri: &str) -> Option<String> {
    // Tolerant of there being no repository at all, unlike most accessors
    // here. This one is reached from the module loader, whose unit tests
    // resolve specifiers without a database; "no repository" and "no tree"
    // are the same answer to this question.
    let repo = get_repository_opt()?;
    match run_bounded(async { repo.root_path(script_uri).await }) {
        Ok(path) => path,
        Err(e) => {
            warn!("Failed resolving the root module of {}: {}", script_uri, e);
            None
        }
    }
}

/// The script's root source as it is *stored*, whatever it currently serves.
///
/// [`fetch_script`] answers with the served source, which for a pinned script
/// is the pinned revision — right for the execution path, and wrong for every
/// caller that is editing. Those callers read a file, change part of it and
/// write the result back to head: based on the pin, the write silently
/// replaced head with the pinned content plus the edits, so a patch landing
/// after a batch reverted the batch. It also disagreed with the asset side,
/// which has always read head, so the two halves of a pinned script's tree
/// were being edited from different versions of it.
///
/// This goes to the database rather than to the in-memory cache, because the
/// cache holds what the script serves. Authoring is not the request path, so
/// the read costs nothing that matters.
pub fn fetch_script_head(uri: &str) -> Option<String> {
    let repo = get_repository();
    match run_bounded(async { repo.get_script(uri).await }) {
        Ok(script) => script,
        Err(e) => {
            warn!("Failed to fetch stored script {}: {}", uri, e);
            None
        }
    }
}

/// Get metadata for a script
pub fn get_script_metadata(uri: &str) -> AppResult<ScriptMetadata> {
    let repo = get_repository();
    run_bounded(async { repo.get_script_metadata(uri).await })
}

/// Get metadata for all scripts
pub fn get_all_script_metadata() -> AppResult<Vec<ScriptMetadata>> {
    let repo = get_repository();
    run_bounded(async { repo.get_all_script_metadata().await })
}

pub fn mark_script_init_failed(uri: &str, error: String) -> AppResult<()> {
    let repo = get_repository();
    run_bounded(async {
        repo.update_script_init_status(uri, false, Some(error), None)
            .await
    })
}

pub fn mark_script_initialized(uri: &str) -> AppResult<()> {
    let repo = get_repository();
    run_bounded(async { repo.update_script_init_status(uri, true, None, None).await })
}

pub fn mark_script_initialized_with_registrations(
    uri: &str,
    registrations: RouteRegistrations,
) -> AppResult<()> {
    let repo = get_repository();
    run_bounded(async {
        repo.update_script_init_status(uri, true, None, Some(registrations))
            .await
    })
}

/// Upsert script with error handling
pub fn upsert_script(uri: &str, content: &str) -> AppResult<()> {
    run_blocking(upsert_script_async(uri, content))
}

/// Async variant of [`upsert_script`] for callers already in async context
pub async fn upsert_script_async(uri: &str, content: &str) -> AppResult<()> {
    if uri.trim().is_empty() {
        return Err(RepositoryError::InvalidData("URI cannot be empty".to_string()).into());
    }

    if content.len() > MAX_SCRIPT_CONTENT_BYTES {
        return Err(
            RepositoryError::InvalidData("Script content too large (>1MB)".to_string()).into(),
        );
    }

    let repo = get_repository();
    repo.upsert_script(uri, content, None).await
}

/// Make sure the script row exists, and give a new one its first owner.
///
/// The identity half of creating a script, with no content in it. The files
/// carry the content now, and `assets.script_uri` references this row, so a
/// change that writes a script's first file has to create the row before it
/// can write anything into the tree.
///
/// Idempotent: an existing script keeps its name, its timestamps and its
/// owners. Returns whether the row was already there, which is what a caller
/// reports as `inserted` against `updated`.
pub fn ensure_script(uri: &str, owner_user_id: Option<&str>) -> AppResult<bool> {
    let repo = get_repository();
    let existed = run_bounded(async { repo.get_script(uri).await })?.is_some();

    if !existed {
        run_bounded(async { repo.ensure_script_row(uri).await })?;
        note_script_write();
    }

    // Ownership is assigned when the script has none — for a new script that
    // is its creator, and for an existing one it is the backfill
    // `upsert_script_with_owner` has always done.
    if let Some(user_id) = owner_user_id {
        let owner_count = run_bounded(async { repo.count_script_owners(uri).await }).unwrap_or(0);
        if owner_count == 0 {
            run_bounded(async { repo.add_script_owner(uri, user_id).await })?;
        }
    }

    Ok(existed)
}

/// Upsert script and set owner if it's a new script
pub fn upsert_script_with_owner(
    uri: &str,
    content: &str,
    owner_user_id: Option<&str>,
) -> AppResult<()> {
    upsert_root_with_owner(uri, None, content, owner_user_id)
}

/// [`upsert_script_with_owner`], writing the source into the entrypoint file
/// `root` names rather than into whichever one the tree already has. See
/// `upsert_script_rows` for what naming it changes.
pub fn upsert_root_with_owner(
    uri: &str,
    root: Option<&str>,
    content: &str,
    owner_user_id: Option<&str>,
) -> AppResult<()> {
    debug!(
        "upsert_script_with_owner called: uri={}, owner_user_id={:?}, content_len={}",
        uri,
        owner_user_id,
        content.len()
    );

    if uri.trim().is_empty() {
        return Err(RepositoryError::InvalidData("URI cannot be empty".to_string()).into());
    }

    if content.len() > MAX_SCRIPT_CONTENT_BYTES {
        return Err(
            RepositoryError::InvalidData("Script content too large (>1MB)".to_string()).into(),
        );
    }

    // Check if script already exists
    let script_exists = fetch_script(uri).is_some();
    debug!(
        "Script existence check: uri={}, exists={}",
        uri, script_exists
    );

    // Upsert the script
    let repo = get_repository();
    run_bounded(async { repo.upsert_script(uri, content, root).await })?;

    // Assign ownership if needed:
    // - For NEW scripts: set the creator as owner
    // - For EXISTING scripts: set owner if they don't have any owners yet (backfill)
    if let Some(user_id) = owner_user_id {
        // Check if script has any owners
        let owner_count = run_bounded(async { repo.count_script_owners(uri).await }).unwrap_or(0);
        let has_owners = owner_count > 0;
        debug!(
            "Owner count check: uri={}, count={}, has_owners={}",
            uri, owner_count, has_owners
        );

        // If script has no owners, assign this user as the first owner
        if !has_owners {
            debug!("Attempting to add owner: uri={}, user_id={}", uri, user_id);
            run_bounded(async { repo.add_script_owner(uri, user_id).await })?;
            if script_exists {
                debug!(
                    "✓ Backfilled owner {} for existing script {} (had no owners)",
                    user_id, uri
                );
            } else {
                debug!("✓ Set initial owner {} for new script {}", user_id, uri);
            }
        } else {
            debug!(
                "Skipping ownership assignment - script already has {} owner(s): uri={}",
                owner_count, uri
            );
        }
    } else {
        debug!(
            "Skipping ownership assignment - no user_id provided: uri={}",
            uri
        );
    }

    Ok(())
}

/// Delete script with error handling
pub fn delete_script(uri: &str) -> bool {
    let repo = get_repository();

    let result = run_bounded(async { repo.delete_script(uri).await });

    match result {
        Ok(existed) => {
            if existed {
                scheduler::clear_script_jobs(uri);
                // And its queued work. Tasks deliberately survive `init()` —
                // a new version of a script must not discard work already
                // accepted — but there is no version left to run them, and a
                // task naming a script the worker cannot fetch would fail its
                // every attempt before giving up.
                let deleted_tasks = uri.to_string();
                if let Err(e) = run_bounded(async move {
                    crate::tasks::delete_for_script(&deleted_tasks)
                        .await
                        .map_err(|e| AppError::Database {
                            message: e.to_string(),
                            source: None,
                        })
                }) {
                    warn!("Failed to clear queued tasks for '{}': {}", uri, e);
                }
                // And the MCP handles naming it. A handle whose script is gone
                // could never reach a terminal status, so a client polling one
                // would do so until its TTL rather than being told.
                let deleted_handles = uri.to_string();
                if let Err(e) = run_bounded(async move {
                    crate::mcp_tasks::delete_for_script(&deleted_handles)
                        .await
                        .map_err(|e| AppError::Database {
                            message: e.to_string(),
                            source: None,
                        })
                }) {
                    warn!("Failed to clear MCP task handles for '{}': {}", uri, e);
                }
                debug!("Deleted script from repository: {}", uri);
            } else {
                debug!("Script not found in repository for deletion: {}", uri);
            }
            existed
        }
        Err(e) => {
            error!("Failed to delete script {}: {}", uri, e);
            false
        }
    }
}

/// Why a rename did not happen.
#[derive(Debug)]
pub enum RenameError {
    NotFound,
    /// The new name belongs to another script.
    Taken,
    Storage(String),
}

/// Rename a script and bring this instance's view of it along.
///
/// The database part is one statement's worth of work because every table that
/// names a script follows a change to `scripts.uri`. What is left is what the
/// database cannot reach: the in-memory state keyed by the old name. A script
/// that was pinned stays pinned, keeps its limits and its queued work, and is
/// initialised again under the new name by the caller so its registrations,
/// scheduled jobs and MCP tools are the new name's.
pub fn rename_script(from: &str, to: &str) -> Result<(), RenameError> {
    let repo = get_repository();
    let result = run_bounded(async { repo.rename_script(from, to).await });
    match result {
        Ok(()) => {}
        Err(AppError::Validation { .. }) => return Err(RenameError::Taken),
        Err(AppError::ScriptNotFound { .. }) => return Err(RenameError::NotFound),
        Err(e) => return Err(RenameError::Storage(e.to_string())),
    }

    scheduler::clear_script_jobs(from);
    crate::mcp::clear_script_mcp_registrations(from);
    crate::revisions::forget_current(from);
    crate::deployments::forget_pin(from);
    crate::script_limits::forget(from);
    crate::exposure::clear_for_script(from);
    let target = to.to_string();
    let _ = run_bounded(async move {
        crate::revisions::refresh_current(&target).await;
        crate::deployments::refresh(&target).await;
        crate::script_limits::refresh(&target).await;
        Ok(())
    });
    Ok(())
}

/// Add an owner to a script
pub fn add_script_owner(uri: &str, user_id: &str) -> AppResult<()> {
    let repo = get_repository();
    run_bounded(async { repo.add_script_owner(uri, user_id).await })
}

/// Remove an owner from a script
pub fn remove_script_owner(uri: &str, user_id: &str) -> AppResult<bool> {
    let repo = get_repository();
    run_bounded(async { repo.remove_script_owner(uri, user_id).await })
}

/// Get all owners of a script
pub fn get_script_owners(uri: &str) -> AppResult<Vec<String>> {
    let repo = get_repository();
    run_bounded(async { repo.get_script_owners(uri).await })
}

/// Get the hosts a script's registrations are published on.
///
/// Returns the stored bindings: empty means the default host, and a `*` entry
/// means every configured host. Resolve with [`crate::hosts::effective_hosts`].
pub fn get_script_hosts(uri: &str) -> AppResult<Vec<String>> {
    let repo = get_repository();
    run_bounded(async { repo.get_script_hosts(uri).await })
}

/// Replace the hosts a script's registrations are published on.
///
/// The list is the complete set, so passing an empty one returns the script to
/// the default host.
pub fn set_script_hosts(uri: &str, hosts: &[String]) -> AppResult<()> {
    let repo = get_repository();
    run_bounded(async { repo.set_script_hosts(uri, hosts).await })
}

/// Check if a user owns a script
pub fn user_owns_script(uri: &str, user_id: &str) -> AppResult<bool> {
    let repo = get_repository();
    run_bounded(async { repo.user_owns_script(uri, user_id).await })
}

/// Count the number of owners for a script
pub fn count_script_owners(uri: &str) -> AppResult<i64> {
    let repo = get_repository();
    run_bounded(async { repo.count_script_owners(uri).await })
}

/// Get repository statistics for monitoring
pub fn get_repository_stats() -> HashMap<String, usize> {
    let mut stats = HashMap::new();

    // Count scripts
    match safe_lock_scripts() {
        Ok(guard) => {
            stats.insert("dynamic_scripts".to_string(), guard.len());
        }
        Err(_) => {
            stats.insert("dynamic_scripts".to_string(), 0);
        }
    }

    // Count assets
    let asset_count = if let Some(db) = get_db_pool() {
        run_blocking(async {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM assets")
                .fetch_one(db.pool())
                .await
                .unwrap_or(0) as usize
        })
    } else {
        0
    };
    stats.insert("assets".to_string(), asset_count);

    // Count total log entries
    let log_count = if let Some(db) = get_db_pool() {
        run_blocking(async {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM logs")
                .fetch_one(db.pool())
                .await
                .unwrap_or(0) as usize
        })
    } else {
        0
    };
    stats.insert("log_entries".to_string(), log_count);

    // Count script storage entries
    let script_properties_count = if let Some(db) = get_db_pool() {
        run_blocking(async {
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM script_properties")
                .fetch_one(db.pool())
                .await
                .unwrap_or(0) as usize
        })
    } else {
        0
    };
    stats.insert(
        "script_properties_entries".to_string(),
        script_properties_count,
    );

    stats
}
