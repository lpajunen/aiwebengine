//! The PostgreSQL server the engine starts for itself.
//!
//! A desktop install has no database to point at: one user, one machine, no
//! container runtime and nothing to administer. This module is what fills that
//! gap, and what it deliberately is *not* is a second storage backend. It
//! starts a real PostgreSQL on loopback and hands [`crate::database`] a
//! connection string; everything downstream of the connection string in
//! `RepositoryConfig` — the `sqlx::query!` macros in `repository.rs`, the
//! advisory locks in `revisions.rs`, `LISTEN`/`NOTIFY`, `FOR UPDATE SKIP
//! LOCKED` — is byte-for-byte the code a server deployment runs. That is the
//! property worth protecting: a solution developed against a desktop install
//! has to run unchanged on a cluster, and it cannot if the two are running
//! different storage.
//!
//! The whole of it is behind the `embedded-postgres` feature, off by default,
//! because a server deployment should not carry the weight of code that starts
//! a database it does not own. With the feature off this module still compiles
//! — [`resolve`] is a passthrough and [`start`] is an error that names the
//! missing feature — so no call site needs a `#[cfg]`.

use crate::config::RepositoryConfig;
use crate::error::{AppError, AppResult};

/// Whether this build can start a database of its own.
///
/// Read by `Config::validate`, so `repository.embedded` on a build without the
/// feature is refused at startup and by `--validate-config`, rather than
/// silently falling through to a `database_url` nobody meant.
pub const SUPPORTED: bool = cfg!(feature = "embedded-postgres");

/// The repository configuration to actually connect with.
///
/// Unchanged for every deployment that names its own database, which is why
/// this can sit on the shared path: `repository.embedded` is off, and the
/// caller gets its configuration back.
pub async fn resolve(config: &RepositoryConfig) -> AppResult<RepositoryConfig> {
    if !config.embedded {
        return Ok(config.clone());
    }

    let connection_string = start(config).await?;
    let mut resolved = config.clone();
    resolved.connection_string = connection_string;
    Ok(resolved)
}

#[cfg(not(feature = "embedded-postgres"))]
async fn start(_config: &RepositoryConfig) -> AppResult<String> {
    Err(AppError::config(
        "repository.embedded is set, but this build has no embedded database. \
         Rebuild with --features embedded-postgres-bundled, or set \
         repository.embedded = false and point database_url at a PostgreSQL server.",
    ))
}

/// Stop the embedded server, if this process started one.
///
/// A no-op everywhere else, including on a build without the feature, so the
/// shutdown path needs no `#[cfg]` either.
#[cfg(not(feature = "embedded-postgres"))]
pub async fn stop() {}

#[cfg(feature = "embedded-postgres")]
pub use supervisor::stop;

#[cfg(feature = "embedded-postgres")]
use supervisor::start;

#[cfg(feature = "embedded-postgres")]
mod supervisor {
    use super::{AppError, AppResult, RepositoryConfig};
    use postgresql_embedded::{PostgreSQL, Settings};
    use std::path::PathBuf;
    use std::sync::OnceLock;
    use std::time::Duration;
    use tokio::sync::Mutex;
    use tracing::{info, warn};

    /// The database the engine creates inside the cluster it starts.
    const DATABASE_NAME: &str = "aiwebengine";

    /// How long any one `initdb`, `pg_ctl` or `createdb` may take.
    ///
    /// The crate's default is five seconds, which is a fine bound on a command
    /// against a running server and far too short for the two that run on a
    /// first launch: `initdb` writes a cluster, and the start before it may
    /// have just extracted a hundred megabytes of binaries. A first run that
    /// fails on a slow disk is a desktop install that never starts at all.
    const COMMAND_TIMEOUT: Duration = Duration::from_secs(120);

    /// The running server, and the URL it is reachable at.
    struct Started {
        postgresql: PostgreSQL,
        connection_string: String,
    }

    /// Held for the life of the process.
    ///
    /// `PostgreSQL` stops the server in its own `Drop`, and a `static` is never
    /// dropped — so [`stop`] is what actually shuts it down on the graceful
    /// path. The ungraceful path is left to PostgreSQL's WAL recovery on the
    /// next start, which is what a crash-safe database is for; a supervisor
    /// that tried to be more certain than that would be claiming a guarantee it
    /// cannot make about a killed process.
    static SERVER: OnceLock<Mutex<Option<Started>>> = OnceLock::new();

    fn server() -> &'static Mutex<Option<Started>> {
        SERVER.get_or_init(|| Mutex::new(None))
    }

    pub(super) async fn start(config: &RepositoryConfig) -> AppResult<String> {
        let mut slot = server().lock().await;

        // Idempotent because three entry points reach it — the server, and the
        // `--grant-role` and `--set-password` commands, which a desktop install
        // needs as much as a deployment does and which have no server running
        // to borrow a connection from.
        if let Some(started) = slot.as_ref() {
            return Ok(started.connection_string.clone());
        }

        let root = PathBuf::from(config.embedded_data_dir.trim());
        std::fs::create_dir_all(&root).map_err(|e| {
            AppError::config(format!(
                "Failed to create the embedded database directory {}: {}",
                root.display(),
                e
            ))
        })?;

        let mut settings = base_settings();
        settings.installation_dir = root.join("install");
        settings.data_dir = root.join("cluster");
        settings.password_file = root.join(".pgpass");
        settings.password = password_for(&settings.password_file, &settings.password);
        settings.host = "127.0.0.1".to_string();
        settings.port = config.embedded_port;
        // The data directory *is* the installation, so nothing about it is
        // temporary. Left at the crate's default, `Drop` would delete the
        // cluster — every script, asset and secret in it — on the way out.
        settings.temporary = false;
        settings.timeout = Some(COMMAND_TIMEOUT);
        // Passed on the command line, where it outranks postgresql.conf.
        // PostgreSQL's own default is already `localhost`, and the conf file
        // `initdb` writes leaves that line commented out — so loopback here is
        // inherited rather than chosen, and this is the one setting whose drift
        // would put a database holding every secret in the install on the
        // network. Stated rather than inherited.
        settings
            .configuration
            .insert("listen_addresses".to_string(), "127.0.0.1".to_string());

        info!(
            "Starting the embedded PostgreSQL server in {}",
            root.display()
        );

        let mut postgresql = PostgreSQL::new(settings);
        postgresql
            .setup()
            .await
            .map_err(|e| database_error("install or initialise", &e))?;
        postgresql
            .start()
            .await
            .map_err(|e| database_error("start", &e))?;

        // The crate creates the password file with whatever the process umask
        // says, which is world-readable on a default macOS or Linux account.
        // It holds the superuser password to a database that will hold every
        // secret in the install, so it is narrowed to its owner once it exists.
        restrict_password_file(&postgresql.settings().password_file);

        let exists = postgresql
            .database_exists(DATABASE_NAME)
            .await
            .map_err(|e| database_error("query the databases of", &e))?;
        if !exists {
            postgresql
                .create_database(DATABASE_NAME)
                .await
                .map_err(|e| database_error("create a database in", &e))?;
        }

        let connection_string = postgresql.settings().url(DATABASE_NAME);
        info!(
            "✓ Embedded PostgreSQL listening on 127.0.0.1:{}",
            postgresql.settings().port
        );

        *slot = Some(Started {
            postgresql,
            connection_string: connection_string.clone(),
        });

        Ok(connection_string)
    }

    /// Stop the embedded server, if this process started one.
    pub async fn stop() {
        let mut slot = server().lock().await;
        let Some(started) = slot.take() else {
            return;
        };

        match started.postgresql.stop().await {
            Ok(()) => info!("✓ Embedded PostgreSQL stopped"),
            // Reported rather than propagated: this runs on the way out, after
            // the shutdown signals have already gone, and there is nothing left
            // to abort. What a failure costs is WAL recovery on the next start.
            Err(e) => warn!("Failed to stop the embedded PostgreSQL server: {}", e),
        }
    }

    /// The crate's defaults, minus the two temporary directories it creates
    /// eagerly to hold them.
    ///
    /// `Settings::new` makes a temporary data directory and a temporary home
    /// for the password file, and marks both to survive the process — so
    /// overriding them, which is the entire point of a persistent install,
    /// leaves two empty directories behind on every single start. They are
    /// removed with `remove_dir` rather than `remove_dir_all`, so anything that
    /// unexpectedly has contents is left alone.
    fn base_settings() -> Settings {
        let settings = Settings::new();
        let _ = std::fs::remove_dir(&settings.data_dir);
        if let Some(parent) = settings.password_file.parent() {
            let _ = std::fs::remove_dir(parent);
        }
        settings
    }

    /// The password the cluster will actually accept.
    ///
    /// `Settings::new` generates a fresh random password every time, and the
    /// crate writes the password file only when it initialises a cluster — so
    /// on the second and every later start, the password in hand is one the
    /// cluster `initdb` created has never heard of, and the engine fails to
    /// authenticate against its own database. Reading it back is what makes the
    /// cluster persistent rather than single-use; the generated one is used
    /// only on the first run, where it becomes the file's contents.
    fn password_for(password_file: &std::path::Path, generated: &str) -> String {
        match std::fs::read_to_string(password_file) {
            Ok(stored) => stored.trim().to_string(),
            Err(_) => generated.to_string(),
        }
    }

    /// Narrow the password file to its owner, on the platforms that have the
    /// concept. Best-effort: a file the engine cannot chmod is one it is about
    /// to read successfully anyway, and refusing to start over it would trade a
    /// working install for a permission bit.
    fn restrict_password_file(path: &std::path::Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
                warn!("Could not restrict {} to its owner: {}", path.display(), e);
            }
        }
        #[cfg(not(unix))]
        let _ = path;
    }

    fn database_error(action: &str, error: &postgresql_embedded::Error) -> AppError {
        AppError::Database {
            message: format!(
                "Failed to {} the embedded PostgreSQL server: {}",
                action, error
            ),
            source: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The property that lets `resolve` sit on the shared startup path: every
    /// deployment that names its own database gets its configuration back
    /// untouched, so the embedded case costs them nothing but a boolean check.
    #[tokio::test]
    async fn resolve_is_a_passthrough_when_not_embedded() {
        let config = RepositoryConfig {
            embedded: false,
            connection_string: "postgresql://someone@example.test:5432/aiwebengine".to_string(),
            ..Default::default()
        };

        let resolved = resolve(&config).await.expect("passthrough cannot fail");

        assert_eq!(resolved.connection_string, config.connection_string);
        assert!(!resolved.embedded);
    }

    /// Asking a server build for an embedded database is an error naming the
    /// missing feature, never a fall-through to `database_url` — which on a
    /// desktop configuration is the placeholder the shipped config carries.
    #[cfg(not(feature = "embedded-postgres"))]
    #[tokio::test]
    async fn embedded_without_the_feature_is_refused() {
        let config = RepositoryConfig {
            embedded: true,
            ..Default::default()
        };

        let error = resolve(&config)
            .await
            .expect_err("a build with no embedded database cannot start one");

        assert!(
            error.to_string().contains("embedded-postgres"),
            "the error should name the feature to rebuild with, got: {error}"
        );
    }

    /// The same refusal from `--validate-config`, which is where an operator
    /// finds out before a deployment rather than during one.
    #[cfg(not(feature = "embedded-postgres"))]
    #[test]
    fn config_validation_refuses_embedded_without_the_feature() {
        let mut config = crate::config::Config::default();
        config.repository.embedded = true;

        let error = config
            .validate()
            .expect_err("validation should refuse an embedded database this build cannot start");

        assert!(error.to_string().contains("repository.embedded"));
    }
}
