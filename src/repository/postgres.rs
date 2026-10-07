//! The `Repository` trait and its Postgres implementation, and the process-wide instance.

use super::*;
use crate::db_schema_utils::ColumnType;
use crate::error::{AppError, AppResult};
use crate::log_retention::LogRetention;
use async_trait::async_trait;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::SystemTime;
use tracing::{error, warn};

/// Abstract repository interface
#[async_trait]
pub trait Repository: Send + Sync {
    // Script operations
    async fn get_script(&self, uri: &str) -> AppResult<Option<String>>;
    /// Which file of the script's tree is its root module.
    async fn root_path(&self, uri: &str) -> AppResult<Option<String>>;
    /// Create the script row if it is not there, with no content.
    async fn ensure_script_row(&self, uri: &str) -> AppResult<()>;
    async fn list_scripts(&self) -> AppResult<HashMap<String, String>>;
    async fn upsert_script(&self, uri: &str, content: &str, root: Option<&str>) -> AppResult<()>;
    async fn delete_script(&self, uri: &str) -> AppResult<bool>;
    /// Give a script a new name. Everything that names it follows in the same
    /// transaction (the foreign keys cascade; `logs` is moved explicitly).
    async fn rename_script(&self, from: &str, to: &str) -> AppResult<()>;
    async fn get_script_metadata(&self, uri: &str) -> AppResult<ScriptMetadata>;
    async fn get_all_script_metadata(&self) -> AppResult<Vec<ScriptMetadata>>;
    async fn update_script_init_status(
        &self,
        uri: &str,
        initialized: bool,
        init_error: Option<String>,
        registrations: Option<RouteRegistrations>,
    ) -> AppResult<()>;

    // Asset operations
    async fn get_asset(&self, script_uri: &str, uri: &str) -> AppResult<Option<Asset>>;
    async fn list_assets(&self, script_uri: &str) -> AppResult<HashMap<String, Asset>>;
    async fn upsert_asset(&self, asset: Asset) -> AppResult<()>;
    /// Write several of one script's assets as a unit: one transaction, one
    /// cache invalidation pass, one change notification.
    async fn upsert_assets(&self, script_uri: &str, assets: Vec<Asset>) -> AppResult<()>;
    async fn sync_assets(
        &self,
        script_uri: &str,
        assets: Vec<Asset>,
        delete: Vec<String>,
    ) -> AppResult<usize>;
    async fn delete_asset(&self, script_uri: &str, uri: &str) -> AppResult<bool>;

    // Log operations
    async fn insert_log(
        &self,
        script_uri: &str,
        message: &str,
        level: &str,
        context: &LogContext,
    ) -> AppResult<()>;
    async fn fetch_logs(&self, script_uri: &str) -> AppResult<Vec<LogEntry>>;
    async fn fetch_all_logs(&self) -> AppResult<Vec<LogEntry>>;
    async fn query_logs(&self, query: &LogQuery) -> AppResult<Vec<LogEntry>>;
    async fn clear_logs(&self, script_uri: &str) -> AppResult<()>;
    async fn prune_logs(&self, retention: LogRetention) -> AppResult<u64>;

    // Script storage operations
    async fn get_script_properties(&self, script_uri: &str, key: &str)
    -> AppResult<Option<String>>;
    async fn set_script_properties(
        &self,
        script_uri: &str,
        key: &str,
        value: &str,
    ) -> AppResult<()>;
    async fn remove_script_properties(&self, script_uri: &str, key: &str) -> AppResult<bool>;
    async fn clear_script_properties(&self, script_uri: &str) -> AppResult<()>;
    async fn list_script_properties_keys(&self, script_uri: &str) -> AppResult<Vec<String>>;

    // Personal storage operations
    async fn get_user_properties(
        &self,
        script_uri: &str,
        user_id: &str,
        key: &str,
    ) -> AppResult<Option<String>>;
    async fn set_user_properties(
        &self,
        script_uri: &str,
        user_id: &str,
        key: &str,
        value: &str,
    ) -> AppResult<()>;
    async fn remove_user_properties(
        &self,
        script_uri: &str,
        user_id: &str,
        key: &str,
    ) -> AppResult<bool>;
    async fn clear_user_properties(&self, script_uri: &str, user_id: &str) -> AppResult<()>;
    async fn list_user_properties_keys(
        &self,
        script_uri: &str,
        user_id: &str,
    ) -> AppResult<Vec<String>>;

    // Script secrets operations
    async fn get_script_secret(&self, script_uri: &str, key: &str) -> AppResult<Option<String>>;
    async fn set_script_secret(&self, script_uri: &str, key: &str, value: &str) -> AppResult<()>;
    async fn remove_script_secret(&self, script_uri: &str, key: &str) -> AppResult<bool>;
    async fn clear_script_secrets(&self, script_uri: &str) -> AppResult<()>;
    async fn list_script_secrets(&self, script_uri: &str) -> AppResult<Vec<String>>;

    // User secrets operations
    async fn get_user_secret(
        &self,
        script_uri: &str,
        user_id: &str,
        key: &str,
    ) -> AppResult<Option<String>>;
    async fn set_user_secret(
        &self,
        script_uri: &str,
        user_id: &str,
        key: &str,
        value: &str,
    ) -> AppResult<()>;
    async fn remove_user_secret(
        &self,
        script_uri: &str,
        user_id: &str,
        key: &str,
    ) -> AppResult<bool>;
    async fn clear_user_secrets(&self, script_uri: &str, user_id: &str) -> AppResult<()>;

    // Security operations

    // Ownership operations
    async fn add_script_owner(&self, uri: &str, user_id: &str) -> AppResult<()>;
    async fn remove_script_owner(&self, uri: &str, user_id: &str) -> AppResult<bool>;
    async fn get_script_owners(&self, uri: &str) -> AppResult<Vec<String>>;
    async fn user_owns_script(&self, uri: &str, user_id: &str) -> AppResult<bool>;
    async fn count_script_owners(&self, uri: &str) -> AppResult<i64>;

    // Host binding operations
    async fn get_script_hosts(&self, uri: &str) -> AppResult<Vec<String>>;
    async fn set_script_hosts(&self, uri: &str, hosts: &[String]) -> AppResult<()>;

    // Script database schema operations
    async fn create_script_table(
        &self,
        script_uri: &str,
        logical_table_name: &str,
    ) -> AppResult<String>;
    async fn add_column_to_script_table(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        column_name: &str,
        column_type: ColumnType,
        nullable: bool,
        default_value: Option<&str>,
    ) -> AppResult<()>;
    async fn add_reference_column(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        column_name: &str,
        referenced_logical_table_name: &str,
        nullable: bool,
    ) -> AppResult<()>;
    async fn drop_column(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        column_name: &str,
    ) -> AppResult<bool>;
    async fn drop_script_table(
        &self,
        script_uri: &str,
        logical_table_name: &str,
    ) -> AppResult<bool>;

    // Script database introspection operations
    async fn list_script_tables(&self, script_uri: &str) -> AppResult<Vec<TableInfo>>;
    async fn get_table_schema(
        &self,
        script_uri: &str,
        logical_table_name: &str,
    ) -> AppResult<TableSchema>;
    async fn get_foreign_keys(
        &self,
        script_uri: &str,
        logical_table_name: &str,
    ) -> AppResult<Vec<ForeignKeyInfo>>;

    // Script database data operations
    async fn query_table(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        filters: Option<&HashMap<String, serde_json::Value>>,
        options: &QueryOptions,
    ) -> AppResult<Vec<serde_json::Value>>;
    async fn insert_row(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        data: &HashMap<String, serde_json::Value>,
    ) -> AppResult<serde_json::Value>;
    async fn update_row(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        id: i32,
        data: &HashMap<String, serde_json::Value>,
    ) -> AppResult<serde_json::Value>;
    async fn delete_row(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        id: i32,
    ) -> AppResult<bool>;
    async fn upsert_row(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        key_columns: &[String],
        data: &HashMap<String, serde_json::Value>,
    ) -> AppResult<serde_json::Value>;
    async fn delete_where(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        filters: &HashMap<String, serde_json::Value>,
    ) -> AppResult<u64>;
    async fn ensure_script_table(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        spec: &TableSpec,
    ) -> AppResult<EnsuredTable>;
}

/// PostgreSQL implementation of the Repository trait
pub struct PostgresRepository {
    pub(super) pool: PgPool,
    pub(super) server_id: String,
}

impl PostgresRepository {
    pub fn new(pool: PgPool, server_id: String) -> Self {
        Self { pool, server_id }
    }
}

#[async_trait]
impl Repository for PostgresRepository {
    async fn get_script(&self, uri: &str) -> AppResult<Option<String>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_get_script(&mut **tx, uri).await
            }
            crate::database::TransactionExecutor::Pool(pool) => db_get_script(pool, uri).await,
        }
    }

    async fn ensure_script_row(&self, uri: &str) -> AppResult<()> {
        let executor = crate::database::get_current_executor(&self.pool);
        db_ensure_script_row(executor, uri).await?;
        send_script_notification(&self.pool, uri, "upserted", &self.server_id).await?;
        Ok(())
    }

    async fn root_path(&self, uri: &str) -> AppResult<Option<String>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_root_path(&mut **tx, uri).await
            }
            crate::database::TransactionExecutor::Pool(pool) => db_root_path(pool, uri).await,
        }
    }

    async fn list_scripts(&self) -> AppResult<HashMap<String, String>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_list_scripts(&mut **tx).await
            }
            crate::database::TransactionExecutor::Pool(pool) => db_list_scripts(pool).await,
        }
    }

    async fn upsert_script(&self, uri: &str, content: &str, root: Option<&str>) -> AppResult<()> {
        let executor = crate::database::get_current_executor(&self.pool);
        db_upsert_script(executor, uri, content, root).await?;
        // Naming the entrypoint may have removed another one, and a module
        // cache holding the removed name would go on answering for it.
        if root.is_some() {
            for name in crate::module_loader::ROOT_MODULE_NAMES {
                crate::module_loader::invalidate_asset(uri, name);
            }
        }
        note_script_write();

        // Send notification after successful upsert
        send_script_notification(&self.pool, uri, "upserted", &self.server_id).await?;

        // A pinned script serves a revision, so a write to its files is not a
        // change to what is running. Refreshing the cache or dropping the
        // prepared program here would swap the deployment for head — which is
        // the one thing pinning exists to prevent.
        if crate::deployments::pinned(uri).is_none() {
            // Refresh the cached source in place rather than evicting it:
            // eviction would also drop the script's route registrations,
            // 404ing every one of its routes until the re-init that follows
            // this upsert completes.
            refresh_cached_script_source(uri, content);
            crate::route_index::invalidate();
            crate::bytecode::invalidate(uri);
            // Only the root source changed; the script's imported modules are
            // still current, so the rebuild reads them from cache instead of
            // the database.
            crate::module_loader::invalidate_program(uri);
        }
        Ok(())
    }

    async fn delete_script(&self, uri: &str) -> AppResult<bool> {
        // First, drop all script-owned tables. This joins the caller's
        // transaction when there is one: DROP TABLE takes ACCESS EXCLUSIVE, and
        // taking it on a second connection would block on locks the caller
        // already holds.
        if let Ok(mut schema) = ScopedConn::for_schema(&self.pool).await {
            let dropped = db_drop_all_script_tables(schema.conn(), uri).await;
            let _ = schema.finish(dropped).await;
        }

        // Delete the script (within transaction if active)
        let executor = crate::database::get_current_executor(&self.pool);
        let result = match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_delete_script(&mut **tx, uri).await?
            }
            crate::database::TransactionExecutor::Pool(pool) => db_delete_script(pool, uri).await?,
        };

        if result {
            note_script_write();

            // Send notification after successful deletion
            send_script_notification(&self.pool, uri, "deleted", &self.server_id).await?;

            // Update in-memory cache
            if let Ok(mut guard) = safe_lock_scripts() {
                guard.remove(uri);
            }
            crate::revisions::forget_current(uri);
            crate::route_index::invalidate();
            crate::bytecode::invalidate(uri);
            crate::module_loader::invalidate(uri);
        }
        Ok(result)
    }

    async fn rename_script(&self, from: &str, to: &str) -> AppResult<()> {
        let db_err = |e: sqlx::Error| {
            error!("Database error renaming script: {}", e);
            AppError::Database {
                message: format!("Database error: {}", e),
                source: None,
            }
        };
        let mut tx = self.pool.begin().await.map_err(db_err)?;

        let taken: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM scripts WHERE uri = $1)")
            .bind(to)
            .fetch_one(&mut *tx)
            .await
            .map_err(db_err)?;
        if taken {
            return Err(AppError::Validation {
                field: "to".to_string(),
                reason: format!("Script already exists: {}", to),
            });
        }

        // The display name follows the identifier unless somebody chose
        // another, which is what `name` is for.
        let derived = |uri: &str| uri.rsplit('/').next().unwrap_or(uri).to_string();
        let renamed = sqlx::query(
            r#"
            UPDATE scripts
               SET uri = $2,
                   name = CASE WHEN name IS NULL OR name = $3 THEN $4 ELSE name END,
                   updated_at = NOW()
             WHERE uri = $1
            "#,
        )
        .bind(from)
        .bind(to)
        .bind(derived(from))
        .bind(derived(to))
        .execute(&mut *tx)
        .await
        .map_err(db_err)?;
        if renamed.rows_affected() == 0 {
            return Err(RepositoryError::ScriptNotFound(from.to_string()).into());
        }

        // Not a script's own table: the engine logs under `server`, so there
        // is no foreign key to follow.
        sqlx::query("UPDATE logs SET script_uri = $2 WHERE script_uri = $1")
            .bind(from)
            .bind(to)
            .execute(&mut *tx)
            .await
            .map_err(db_err)?;

        tx.commit().await.map_err(db_err)?;
        note_script_write();

        send_script_notification(&self.pool, from, "deleted", &self.server_id).await?;
        send_script_notification(&self.pool, to, "upserted", &self.server_id).await?;

        // Re-keyed in place rather than evicted, so the script keeps answering
        // its routes while it is initialised again under its new name:
        // eviction drops the registrations with the entry.
        if let Ok(mut guard) = safe_lock_scripts()
            && let Some(mut metadata) = guard.remove(from)
        {
            if metadata.name.as_deref() == Some(derived(from).as_str()) {
                metadata.name = Some(derived(to));
            }
            metadata.uri = to.to_string();
            guard.insert(to.to_string(), metadata);
        }
        for uri in [from, to] {
            crate::bytecode::invalidate(uri);
            crate::module_loader::invalidate(uri);
        }
        crate::route_index::invalidate();
        Ok(())
    }

    async fn get_script_metadata(&self, uri: &str) -> AppResult<ScriptMetadata> {
        // Check cache first
        if let Ok(guard) = safe_lock_scripts()
            && let Some(metadata) = guard.get(uri)
        {
            return Ok(metadata.clone());
        }

        // Fetch from DB. Taken before the read, so a write that lands while it
        // is in flight is visible to the fill below.
        let writes_before = script_write_count();
        let content = self
            .get_script(uri)
            .await?
            .ok_or_else(|| RepositoryError::ScriptNotFound(uri.to_string()))?;

        let executor = crate::database::get_current_executor(&self.pool);
        let owners = match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_get_script_owners(&mut **tx, uri).await?
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_get_script_owners(pool, uri).await?
            }
        };

        let script_hosts = match crate::database::get_current_executor(&self.pool) {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_get_script_hosts(&mut **tx, uri).await?
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_get_script_hosts(pool, uri).await?
            }
        };

        let mut metadata = ScriptMetadata::new(uri.to_string(), content);
        metadata.owners = owners;
        metadata.hosts = script_hosts;

        // Cache it, unless a write landed while the read above was in flight:
        // that write already put its own content in the database and the cache,
        // and installing what this read holds would put the cache permanently
        // behind the database. Skipping the fill costs the next reader one
        // query, which is the cheap side of the trade.
        if writes_before == script_write_count()
            && let Ok(mut guard) = safe_lock_scripts()
        {
            guard.insert(uri.to_string(), metadata.clone());
        }

        Ok(metadata)
    }

    async fn get_all_script_metadata(&self) -> AppResult<Vec<ScriptMetadata>> {
        // Fetch scripts from database only (no static scripts for Postgres)
        let db_scripts = self.list_scripts().await?;

        // Fetch all owners in one query
        let executor = crate::database::get_current_executor(&self.pool);
        let all_owners = match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_get_all_script_owners(&mut **tx).await?
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_get_all_script_owners(pool).await?
            }
        };

        // Fetch all host bindings in one query, same as owners above
        let all_hosts = match crate::database::get_current_executor(&self.pool) {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_get_all_script_hosts(&mut **tx).await?
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_get_all_script_hosts(pool).await?
            }
        };

        let all_created = match crate::database::get_current_executor(&self.pool) {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_get_all_script_created(&mut **tx).await?
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_get_all_script_created(pool).await?
            }
        };

        let mut metadata_list = Vec::new();

        // Scope for mutex lock
        {
            let mut guard = safe_lock_scripts()?;
            for (uri, content) in db_scripts {
                if let Some(cached) = guard.get_mut(&uri) {
                    // Use cached version to preserve runtime state, with the
                    // age the database has rather than the one this process
                    // happened to stamp when it first loaded it.
                    if let Some(created) = all_created.get(&uri) {
                        cached.created_at = *created;
                    }
                    metadata_list.push(cached.clone());
                } else {
                    // Create new metadata and cache it
                    let mut metadata = ScriptMetadata::new(uri.clone(), content);
                    // Set owners from bulk query
                    if let Some(owners) = all_owners.get(&uri) {
                        metadata.owners = owners.clone();
                    }
                    // Set host bindings from bulk query
                    if let Some(script_hosts) = all_hosts.get(&uri) {
                        metadata.hosts = script_hosts.clone();
                    }
                    if let Some(created) = all_created.get(&uri) {
                        metadata.created_at = *created;
                    }
                    guard.insert(uri.clone(), metadata.clone());
                    metadata_list.push(metadata);
                }
            }
        }

        Ok(metadata_list)
    }

    async fn update_script_init_status(
        &self,
        uri: &str,
        initialized: bool,
        init_error: Option<String>,
        registrations: Option<RouteRegistrations>,
    ) -> AppResult<()> {
        let mut guard = safe_lock_scripts()?;
        let metadata = match guard.get_mut(uri) {
            Some(metadata) => metadata,
            // Every script is a database script. The fallback that stood here
            // resolved a URI against scripts compiled into the binary, which
            // were the same two test fixtures the bootstrap inserted, behind
            // the same environment variable.
            None => return Err(RepositoryError::ScriptNotFound(uri.to_string()).into()),
        };

        // Registrations from a *failed* init() are partial by definition — the
        // script stopped registering wherever it broke. They are worth
        // installing only when there is no working table to lose, which is the
        // case on a script's first init: a route table that is missing entries
        // still beats a script that answers nothing. An already-serving table
        // is kept instead, so a broken redeploy degrades to "running the old
        // routes" rather than to a partial set.
        if let Some(regs) = registrations
            && (initialized || metadata.registrations.is_empty())
        {
            metadata.registrations = regs;
        }
        // Routing reads `initialized` as "has a usable route table" (see
        // `route_index::build_index`), so it stays set whenever registrations
        // are installed; `init_error` is what records a failure.
        metadata.initialized = initialized || !metadata.registrations.is_empty();
        metadata.init_error = init_error;
        if initialized {
            metadata.last_init_time = Some(SystemTime::now());
        }
        drop(guard);
        crate::route_index::invalidate();
        Ok(())
    }

    async fn get_asset(&self, script_uri: &str, uri: &str) -> AppResult<Option<Asset>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_get_asset(&mut **tx, script_uri, uri).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_get_asset(pool, script_uri, uri).await
            }
        }
    }

    async fn list_assets(&self, script_uri: &str) -> AppResult<HashMap<String, Asset>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_list_assets(&mut **tx, script_uri).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_list_assets(pool, script_uri).await
            }
        }
    }

    async fn upsert_asset(&self, asset: Asset) -> AppResult<()> {
        let executor = crate::database::get_current_executor(&self.pool);
        db_upsert_asset(executor, &asset).await?;

        // An imported asset is part of the owning script's prepared program, so
        // its change must invalidate the same caches a script edit does — both
        // locally and (via the refresh notification) on other cluster nodes.
        invalidate_script_asset_caches(&asset.script_uri, &asset.uri, Some(&asset.content));
        send_script_notification(&self.pool, &asset.script_uri, "upserted", &self.server_id)
            .await?;
        Ok(())
    }

    /// The batch counterpart of [`Repository::upsert_asset`].
    ///
    /// Two things make this more than a loop over that method. The rows go in
    /// under one transaction, so a failure partway through leaves none of them
    /// behind. And the cache invalidation and the `script_upserted`
    /// notification happen once for the whole set: per-file notifications
    /// would make every other cluster node reinitialize the script once per
    /// file, each time from a set that is still being written.
    async fn upsert_assets(&self, script_uri: &str, assets: Vec<Asset>) -> AppResult<()> {
        if assets.is_empty() {
            return Ok(());
        }

        if crate::database::get_current_transaction_active() {
            // The caller already owns a transaction (a script writing inside
            // `transaction()`). Joining it is what every other repository
            // method does here, and committing our own would end theirs early.
            for asset in &assets {
                let executor = crate::database::get_current_executor(&self.pool);
                db_upsert_asset(executor, asset).await?;
            }
        } else {
            let mut tx = self.pool.begin().await.map_err(|e| {
                error!("Database error opening asset batch transaction: {}", e);
                AppError::Database {
                    message: format!("Database error: {}", e),
                    source: None,
                }
            })?;
            for asset in &assets {
                db_upsert_asset(
                    crate::database::TransactionExecutor::Transaction(&mut tx),
                    asset,
                )
                .await?;
            }
            tx.commit().await.map_err(|e| {
                error!("Database error committing asset batch: {}", e);
                AppError::Database {
                    message: format!("Database error: {}", e),
                    source: None,
                }
            })?;
        }

        for asset in &assets {
            invalidate_script_asset_caches(script_uri, &asset.uri, Some(&asset.content));
        }
        send_script_notification(&self.pool, script_uri, "upserted", &self.server_id).await?;
        Ok(())
    }

    /// Write some of a script's assets and remove others, as one act.
    ///
    /// [`upsert_assets`](Self::upsert_assets) and
    /// [`delete_asset`](Self::delete_asset) each hold their own transaction, so
    /// a caller replacing a tree had to do the two in sequence and a process
    /// dying between them left files the source of truth no longer has. Here
    /// both halves share one transaction: the tree either becomes what the
    /// caller described or stays exactly as it was.
    ///
    /// Removals run after the writes so that a path being written and named for
    /// removal in the same call ends up removed, which is the reading that
    /// matches the caller saying "these files, and not those".
    async fn sync_assets(
        &self,
        script_uri: &str,
        assets: Vec<Asset>,
        delete: Vec<String>,
    ) -> AppResult<usize> {
        if assets.is_empty() && delete.is_empty() {
            return Ok(0);
        }

        let mut deleted = 0usize;

        if crate::database::get_current_transaction_active() {
            // The caller already owns a transaction; joining it is what every
            // other repository method does, and committing our own would end
            // theirs early.
            for asset in &assets {
                let executor = crate::database::get_current_executor(&self.pool);
                db_upsert_asset(executor, asset).await?;
            }
            for uri in &delete {
                let executor = crate::database::get_current_executor(&self.pool);
                let removed = match executor {
                    crate::database::TransactionExecutor::Transaction(tx) => {
                        db_delete_asset(&mut **tx, script_uri, uri).await?
                    }
                    crate::database::TransactionExecutor::Pool(pool) => {
                        db_delete_asset(pool, script_uri, uri).await?
                    }
                };
                if removed {
                    deleted += 1;
                }
            }
        } else {
            let mut tx = self.pool.begin().await.map_err(|e| {
                error!("Database error opening asset sync transaction: {}", e);
                AppError::Database {
                    message: format!("Database error: {}", e),
                    source: None,
                }
            })?;
            for asset in &assets {
                db_upsert_asset(
                    crate::database::TransactionExecutor::Transaction(&mut tx),
                    asset,
                )
                .await?;
            }
            for uri in &delete {
                if db_delete_asset(&mut *tx, script_uri, uri).await? {
                    deleted += 1;
                }
            }
            tx.commit().await.map_err(|e| {
                error!("Database error committing asset sync: {}", e);
                AppError::Database {
                    message: format!("Database error: {}", e),
                    source: None,
                }
            })?;
        }

        // Caches and peers are told only once the transaction has landed, so
        // nothing is invalidated on behalf of a write that rolled back.
        for asset in &assets {
            invalidate_script_asset_caches(script_uri, &asset.uri, Some(&asset.content));
        }
        for uri in &delete {
            invalidate_script_asset_caches(script_uri, uri, None);
        }
        send_script_notification(&self.pool, script_uri, "upserted", &self.server_id).await?;
        Ok(deleted)
    }

    async fn delete_asset(&self, script_uri: &str, uri: &str) -> AppResult<bool> {
        let executor = crate::database::get_current_executor(&self.pool);
        let result = match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_delete_asset(&mut **tx, script_uri, uri).await?
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_delete_asset(pool, script_uri, uri).await?
            }
        };

        if result {
            invalidate_script_asset_caches(script_uri, uri, None);
            send_script_notification(&self.pool, script_uri, "upserted", &self.server_id).await?;
        }
        Ok(result)
    }

    async fn insert_log(
        &self,
        script_uri: &str,
        message: &str,
        level: &str,
        context: &LogContext,
    ) -> AppResult<()> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_insert_log_message(&mut **tx, script_uri, message, level, context).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_insert_log_message(pool, script_uri, message, level, context).await
            }
        }
    }

    async fn fetch_logs(&self, script_uri: &str) -> AppResult<Vec<LogEntry>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_fetch_log_messages(&mut **tx, script_uri).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_fetch_log_messages(pool, script_uri).await
            }
        }
    }

    async fn fetch_all_logs(&self) -> AppResult<Vec<LogEntry>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_fetch_all_log_messages(&mut **tx).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_fetch_all_log_messages(pool).await
            }
        }
    }

    async fn query_logs(&self, query: &LogQuery) -> AppResult<Vec<LogEntry>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_query_log_messages(&mut **tx, query).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_query_log_messages(pool, query).await
            }
        }
    }

    async fn clear_logs(&self, script_uri: &str) -> AppResult<()> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_clear_log_messages(&mut **tx, script_uri).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_clear_log_messages(pool, script_uri).await
            }
        }
    }

    /// Always on the pool rather than any ambient transaction: the pass takes
    /// an advisory lock for its own transaction's lifetime, which only means
    /// anything if that transaction is the prune's own.
    async fn prune_logs(&self, retention: LogRetention) -> AppResult<u64> {
        db_prune_log_messages(&self.pool, retention).await
    }

    async fn get_script_properties(
        &self,
        script_uri: &str,
        key: &str,
    ) -> AppResult<Option<String>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_get_script_properties_item(&mut **tx, script_uri, key).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_get_script_properties_item(pool, script_uri, key).await
            }
        }
    }

    async fn set_script_properties(
        &self,
        script_uri: &str,
        key: &str,
        value: &str,
    ) -> AppResult<()> {
        let executor = crate::database::get_current_executor(&self.pool);
        db_set_script_properties_item(executor, script_uri, key, value).await
    }

    async fn remove_script_properties(&self, script_uri: &str, key: &str) -> AppResult<bool> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_remove_script_properties_item(&mut **tx, script_uri, key).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_remove_script_properties_item(pool, script_uri, key).await
            }
        }
    }

    async fn clear_script_properties(&self, script_uri: &str) -> AppResult<()> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_clear_script_properties(&mut **tx, script_uri).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_clear_script_properties(pool, script_uri).await
            }
        }
    }

    async fn list_script_properties_keys(&self, script_uri: &str) -> AppResult<Vec<String>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_list_script_properties_keys(&mut **tx, script_uri).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_list_script_properties_keys(pool, script_uri).await
            }
        }
    }

    async fn get_user_properties(
        &self,
        script_uri: &str,
        user_id: &str,
        key: &str,
    ) -> AppResult<Option<String>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_get_user_properties_item(&mut **tx, script_uri, user_id, key).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_get_user_properties_item(pool, script_uri, user_id, key).await
            }
        }
    }

    async fn set_user_properties(
        &self,
        script_uri: &str,
        user_id: &str,
        key: &str,
        value: &str,
    ) -> AppResult<()> {
        let executor = crate::database::get_current_executor(&self.pool);
        db_set_user_properties_item(executor, script_uri, user_id, key, value).await
    }

    async fn remove_user_properties(
        &self,
        script_uri: &str,
        user_id: &str,
        key: &str,
    ) -> AppResult<bool> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_remove_user_properties_item(&mut **tx, script_uri, user_id, key).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_remove_user_properties_item(pool, script_uri, user_id, key).await
            }
        }
    }

    async fn clear_user_properties(&self, script_uri: &str, user_id: &str) -> AppResult<()> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_clear_user_properties(&mut **tx, script_uri, user_id).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_clear_user_properties(pool, script_uri, user_id).await
            }
        }
    }

    async fn list_user_properties_keys(
        &self,
        script_uri: &str,
        user_id: &str,
    ) -> AppResult<Vec<String>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_list_user_properties_keys(&mut **tx, script_uri, user_id).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_list_user_properties_keys(pool, script_uri, user_id).await
            }
        }
    }

    async fn get_script_secret(&self, script_uri: &str, key: &str) -> AppResult<Option<String>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_get_script_secret(&mut **tx, script_uri, key).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_get_script_secret(pool, script_uri, key).await
            }
        }
    }

    async fn set_script_secret(&self, script_uri: &str, key: &str, value: &str) -> AppResult<()> {
        let executor = crate::database::get_current_executor(&self.pool);
        db_set_script_secret(executor, script_uri, key, value).await
    }

    async fn remove_script_secret(&self, script_uri: &str, key: &str) -> AppResult<bool> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_remove_script_secret(&mut **tx, script_uri, key).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_remove_script_secret(pool, script_uri, key).await
            }
        }
    }

    async fn clear_script_secrets(&self, script_uri: &str) -> AppResult<()> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_clear_script_secrets(&mut **tx, script_uri).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_clear_script_secrets(pool, script_uri).await
            }
        }
    }

    async fn list_script_secrets(&self, script_uri: &str) -> AppResult<Vec<String>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_list_script_secrets(&mut **tx, script_uri).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_list_script_secrets(pool, script_uri).await
            }
        }
    }

    async fn get_user_secret(
        &self,
        script_uri: &str,
        user_id: &str,
        key: &str,
    ) -> AppResult<Option<String>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_get_user_secret(&mut **tx, script_uri, user_id, key).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_get_user_secret(pool, script_uri, user_id, key).await
            }
        }
    }

    async fn set_user_secret(
        &self,
        script_uri: &str,
        user_id: &str,
        key: &str,
        value: &str,
    ) -> AppResult<()> {
        let executor = crate::database::get_current_executor(&self.pool);
        db_set_user_secret(executor, script_uri, user_id, key, value).await
    }

    async fn remove_user_secret(
        &self,
        script_uri: &str,
        user_id: &str,
        key: &str,
    ) -> AppResult<bool> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_remove_user_secret(&mut **tx, script_uri, user_id, key).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_remove_user_secret(pool, script_uri, user_id, key).await
            }
        }
    }

    async fn clear_user_secrets(&self, script_uri: &str, user_id: &str) -> AppResult<()> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_clear_user_secrets(&mut **tx, script_uri, user_id).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_clear_user_secrets(pool, script_uri, user_id).await
            }
        }
    }

    async fn add_script_owner(&self, uri: &str, user_id: &str) -> AppResult<()> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_add_script_owner(&mut **tx, uri, user_id).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_add_script_owner(pool, uri, user_id).await
            }
        }
    }

    async fn remove_script_owner(&self, uri: &str, user_id: &str) -> AppResult<bool> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_remove_script_owner(&mut **tx, uri, user_id).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_remove_script_owner(pool, uri, user_id).await
            }
        }
    }

    async fn get_script_owners(&self, uri: &str) -> AppResult<Vec<String>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_get_script_owners(&mut **tx, uri).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_get_script_owners(pool, uri).await
            }
        }
    }

    async fn user_owns_script(&self, uri: &str, user_id: &str) -> AppResult<bool> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_user_owns_script(&mut **tx, uri, user_id).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_user_owns_script(pool, uri, user_id).await
            }
        }
    }

    async fn count_script_owners(&self, uri: &str) -> AppResult<i64> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_count_script_owners(&mut **tx, uri).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_count_script_owners(pool, uri).await
            }
        }
    }

    async fn get_script_hosts(&self, uri: &str) -> AppResult<Vec<String>> {
        let executor = crate::database::get_current_executor(&self.pool);
        match executor {
            crate::database::TransactionExecutor::Transaction(tx) => {
                db_get_script_hosts(&mut **tx, uri).await
            }
            crate::database::TransactionExecutor::Pool(pool) => {
                db_get_script_hosts(pool, uri).await
            }
        }
    }

    async fn set_script_hosts(&self, uri: &str, hosts: &[String]) -> AppResult<()> {
        let executor = crate::database::get_current_executor(&self.pool);
        db_set_script_hosts(executor, uri, hosts).await?;

        // The cached metadata carries the old bindings, and the route index was
        // built from them, so both have to go.
        if let Ok(mut guard) = safe_lock_scripts()
            && let Some(metadata) = guard.get_mut(uri)
        {
            metadata.hosts = hosts.to_vec();
        }
        crate::route_index::invalidate();

        // Other instances cache the old bindings and would keep publishing the
        // script where it used to be. Reuse the upsert channel: their handler
        // refreshes the bindings along with the source.
        send_script_notification(&self.pool, uri, "upserted", &self.server_id).await?;

        Ok(())
    }

    async fn create_script_table(
        &self,
        script_uri: &str,
        logical_table_name: &str,
    ) -> AppResult<String> {
        let mut schema =
            ScopedConn::for_schema_of(&self.pool, script_uri, logical_table_name).await?;
        let created = db_create_script_table(schema.conn(), script_uri, logical_table_name).await;
        schema.finish(created).await
    }

    async fn add_column_to_script_table(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        column_name: &str,
        column_type: ColumnType,
        nullable: bool,
        default_value: Option<&str>,
    ) -> AppResult<()> {
        let mut schema =
            ScopedConn::for_schema_of(&self.pool, script_uri, logical_table_name).await?;
        let added = db_add_column_to_script_table(
            schema.conn(),
            script_uri,
            logical_table_name,
            column_name,
            column_type,
            nullable,
            default_value,
        )
        .await;
        schema.finish(added).await
    }

    async fn add_reference_column(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        column_name: &str,
        referenced_logical_table_name: &str,
        nullable: bool,
    ) -> AppResult<()> {
        let mut schema =
            ScopedConn::for_schema_of(&self.pool, script_uri, logical_table_name).await?;
        let added = db_add_reference_column(
            schema.conn(),
            script_uri,
            logical_table_name,
            column_name,
            referenced_logical_table_name,
            nullable,
        )
        .await;
        schema.finish(added).await
    }

    async fn drop_column(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        column_name: &str,
    ) -> AppResult<bool> {
        let mut schema =
            ScopedConn::for_schema_of(&self.pool, script_uri, logical_table_name).await?;
        let dropped =
            db_drop_column(schema.conn(), script_uri, logical_table_name, column_name).await;
        schema.finish(dropped).await
    }

    async fn drop_script_table(
        &self,
        script_uri: &str,
        logical_table_name: &str,
    ) -> AppResult<bool> {
        let mut schema =
            ScopedConn::for_schema_of(&self.pool, script_uri, logical_table_name).await?;
        let dropped = db_drop_script_table(schema.conn(), script_uri, logical_table_name).await;
        schema.finish(dropped).await
    }

    async fn list_script_tables(&self, script_uri: &str) -> AppResult<Vec<TableInfo>> {
        let mut schema = ScopedConn::for_schema(&self.pool).await?;
        let listed = db_list_script_tables(schema.conn(), script_uri).await;
        schema.finish(listed).await
    }

    async fn get_table_schema(
        &self,
        script_uri: &str,
        logical_table_name: &str,
    ) -> AppResult<TableSchema> {
        let mut schema = ScopedConn::for_schema(&self.pool).await?;
        let fetched = db_get_table_schema(schema.conn(), script_uri, logical_table_name).await;
        schema.finish(fetched).await
    }

    async fn get_foreign_keys(
        &self,
        script_uri: &str,
        logical_table_name: &str,
    ) -> AppResult<Vec<ForeignKeyInfo>> {
        let mut schema = ScopedConn::for_schema(&self.pool).await?;
        let fetched = db_get_foreign_keys(schema.conn(), script_uri, logical_table_name).await;
        schema.finish(fetched).await
    }

    async fn query_table(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        filters: Option<&HashMap<String, serde_json::Value>>,
        options: &QueryOptions,
    ) -> AppResult<Vec<serde_json::Value>> {
        let mut scope = ScopedConn::for_statement(&self.pool).await?;
        let queried = db_query_table(
            scope.conn(),
            script_uri,
            logical_table_name,
            filters,
            options,
        )
        .await;
        scope.finish(queried).await
    }

    async fn insert_row(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        data: &HashMap<String, serde_json::Value>,
    ) -> AppResult<serde_json::Value> {
        let mut scope = ScopedConn::for_statement(&self.pool).await?;
        let inserted = db_insert_row(scope.conn(), script_uri, logical_table_name, data).await;
        scope.finish(inserted).await
    }

    async fn update_row(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        id: i32,
        data: &HashMap<String, serde_json::Value>,
    ) -> AppResult<serde_json::Value> {
        let mut scope = ScopedConn::for_statement(&self.pool).await?;
        let updated = db_update_row(scope.conn(), script_uri, logical_table_name, id, data).await;
        scope.finish(updated).await
    }

    async fn delete_row(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        id: i32,
    ) -> AppResult<bool> {
        let mut scope = ScopedConn::for_statement(&self.pool).await?;
        let deleted = db_delete_row(scope.conn(), script_uri, logical_table_name, id).await;
        scope.finish(deleted).await
    }

    async fn upsert_row(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        key_columns: &[String],
        data: &HashMap<String, serde_json::Value>,
    ) -> AppResult<serde_json::Value> {
        let mut scope = ScopedConn::for_statement(&self.pool).await?;
        let upserted = db_upsert_row(
            scope.conn(),
            script_uri,
            logical_table_name,
            key_columns,
            data,
        )
        .await;
        scope.finish(upserted).await
    }

    async fn delete_where(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        filters: &HashMap<String, serde_json::Value>,
    ) -> AppResult<u64> {
        let mut scope = ScopedConn::for_statement(&self.pool).await?;
        let deleted = db_delete_where(scope.conn(), script_uri, logical_table_name, filters).await;
        scope.finish(deleted).await
    }

    async fn ensure_script_table(
        &self,
        script_uri: &str,
        logical_table_name: &str,
        spec: &TableSpec,
    ) -> AppResult<EnsuredTable> {
        let mut schema =
            ScopedConn::for_schema_of(&self.pool, script_uri, logical_table_name).await?;
        let ensured =
            db_ensure_script_table(schema.conn(), script_uri, logical_table_name, spec).await;
        schema.finish(ensured).await
    }
}

/// Global repository instance
pub(super) static GLOBAL_REPOSITORY: OnceLock<PostgresRepository> = OnceLock::new();

/// Initialize the global repository
pub fn initialize_repository(repo: PostgresRepository) -> bool {
    GLOBAL_REPOSITORY.set(repo).is_ok()
}

/// Get the global repository
/// Returns the global repository if it has been initialized, or `None` otherwise.
/// Use this in contexts where the repository may not be available (e.g. tests without a DB).
pub fn get_repository_opt() -> Option<&'static PostgresRepository> {
    GLOBAL_REPOSITORY.get()
}

pub fn get_repository() -> &'static PostgresRepository {
    GLOBAL_REPOSITORY.get_or_init(|| {
        let db = crate::database::get_global_database()
            .expect("Database must be initialized before repository");

        // Fallback initialization with empty server_id (shouldn't happen in normal flow)
        warn!("Repository not initialized, using fallback with empty server_id");
        PostgresRepository::new(db.pool().clone(), String::new())
    })
}
