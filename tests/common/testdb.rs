//! One database per test process, recycled through a fixed set of slots.
//!
//! Every test in the suite used to run against whatever database
//! `DATABASE_URL` named — in practice the developer's own — and almost none of
//! them cleaned up after themselves. Two things followed, and between them
//! they are most of why a run was neither repeatable nor tidy.
//!
//! The database accumulated. Scripts, users, sessions, OAuth clients, logs,
//! revisions and the per-script dynamic tables of every run ever made stayed
//! where the test left them, mixed in with whatever `cargo run` had put there.
//!
//! And the leftovers were *executed*. `execute_startup_scripts` runs every
//! script it finds, so each new test server ran the accumulated scripts of
//! unrelated tests before the test under way had done anything. What a test
//! saw in the route index, the GraphQL registry and the log table therefore
//! depended on which tests had run before it — in this run and in every run
//! before it. Several test comments already work around one face of this by
//! hand: a rate-limit bucket keyed by a fixed string carries its drained state
//! into the next run, and a script an earlier test left behind answers the
//! route this one just registered.
//!
//! What replaces it: `DATABASE_URL` names a *server*, and the suite keeps its
//! own databases on it. A migrated template is built once; a process claims one
//! of a small number of numbered slots and recreates that slot from the
//! template, so it starts from a database holding nothing but the schema.
//! Nothing a test writes is visible to another test, no test can be influenced
//! by one that ran before it, and the developer's own database is never opened.
//!
//! Slots rather than a fresh database per process, because a test process
//! cannot clean up after itself: Rust runs no destructor at process exit, and a
//! test killed by a timeout would skip one anyway. A slot needs no cleanup. It
//! is claimed with a session advisory lock held on a connection that stays open
//! for the process's life, so the claim is released by the connection closing —
//! which happens whether the process returns, panics, or is killed. At rest the
//! server holds one database per slot ever used, a few hundred megabytes,
//! rather than one per test ever run.
//!
//! Set `AIWEBENGINE_TEST_DB_SHARED=1` to opt out and run against the database
//! `DATABASE_URL` names directly, for the rare case of wanting to inspect what
//! a failing test left behind.

use std::sync::OnceLock;
use std::time::Duration;
use tokio::sync::OnceCell;
use url::Url;

/// The database every slot is recreated from.
///
/// Built once per server and reused across runs: migrating is the expensive
/// part, and its result is a function of the migrations directory alone.
const TEMPLATE_DB: &str = "aiwebengine_test_template";

/// What a slot database is called, before its number.
const SLOT_PREFIX: &str = "aiwebengine_test_slot_";

/// How many slots a process will try before giving up.
///
/// Only as many slots are ever created as there are tests running at once, so
/// this is a ceiling on concurrency rather than on disk: a run with
/// `test-threads = 8` uses eight of them and creates eight databases.
const MAX_SLOTS: i32 = 64;

/// Lock namespaces, not values with meaning. The first is taken for as long as
/// a process holds its slot; the second only while the template is built.
const SLOT_LOCK_NAMESPACE: i32 = 0x_A11E_57DB_u32 as i32;
const TEMPLATE_LOCK: i64 = 0x_A11E_57DB_0001;

/// How many times to re-attempt work that lost a race with another process.
///
/// `CREATE DATABASE ... TEMPLATE` refuses while anything is connected to the
/// source, and the one thing that connects to the template is another process
/// migrating it. That window is short and does not recur.
const ATTEMPTS: u32 = 6;

/// The connection the slot claim is held on.
///
/// Kept for the process's lifetime on purpose, and this is the whole mechanism:
/// a session advisory lock lives exactly as long as its session, so the slot is
/// returned when this connection closes — which the operating system does when
/// the process ends, however it ends.
static CLAIM: OnceLock<tokio::sync::Mutex<sqlx::postgres::PgConnection>> = OnceLock::new();

/// The connection string every test in this process should use.
///
/// `None` when the database server will not answer — callers leave the globals
/// unset, and `should_skip_integration_tests` decides whether a test runs.
///
/// Resolved once per process. Under `cargo nextest` a process is one test, so
/// that is a database per test; under `cargo test` it is one per test binary,
/// and the in-process mutexes in `common` still serialise the tests inside one.
pub async fn connection_string() -> Option<&'static str> {
    static URL: OnceCell<Option<String>> = OnceCell::const_new();
    URL.get_or_init(provision).await.as_deref()
}

/// The database server the suite works on, as configured.
///
/// Read through `AppConfig` rather than from `DATABASE_URL` directly, so the
/// harness and the engine resolve it the same way, fallback included.
fn configured_url() -> String {
    aiwebengine::config::AppConfig::test_config_postgres(0)
        .repository
        .connection_string
}

async fn provision() -> Option<String> {
    let base = configured_url();

    // The escape hatch: run against the named database, the way the suite used
    // to. Whatever a test leaves behind then stays there to be looked at.
    if std::env::var("AIWEBENGINE_TEST_DB_SHARED").is_ok() {
        return Some(base);
    }

    // `postgres` rather than the configured database: `CREATE DATABASE` cannot
    // run from inside the database being created, and the configured one is
    // exactly what this module exists to leave alone.
    let maintenance = with_database(&base, "postgres")?;
    let mut conn =
        match <sqlx::postgres::PgConnection as sqlx::Connection>::connect(&maintenance).await {
            Ok(conn) => conn,
            Err(error) => {
                eprintln!("test database: cannot reach the database server: {error}");
                return None;
            }
        };

    if let Err(error) = ensure_template(&mut conn, &base).await {
        eprintln!("test database: cannot prepare '{TEMPLATE_DB}': {error}");
        return None;
    }

    let slot = claim_slot(&mut conn).await?;
    let name = format!("{SLOT_PREFIX}{slot}");

    if let Err(error) = reset_slot(&mut conn, &name).await {
        eprintln!("test database: cannot reset '{name}': {error}");
        return None;
    }

    // Held, not dropped: the claim ends when this connection does.
    let _ = CLAIM.set(tokio::sync::Mutex::new(conn));

    with_database(&base, &name)
}

/// The same server, a different database.
fn with_database(base: &str, name: &str) -> Option<String> {
    let mut url = Url::parse(base).ok()?;
    url.set_path(name);
    Some(url.to_string())
}

/// Take the lowest-numbered slot nobody else is holding.
///
/// `pg_try_advisory_lock` rather than the blocking form, so a slot another
/// process abandoned mid-run is picked up immediately: its lock went when its
/// connection did, and no timeout has to expire first.
async fn claim_slot(conn: &mut sqlx::postgres::PgConnection) -> Option<i32> {
    for slot in 0..MAX_SLOTS {
        let claimed: bool = sqlx::query_scalar("SELECT pg_try_advisory_lock($1, $2)")
            .bind(SLOT_LOCK_NAMESPACE)
            .bind(slot)
            .fetch_one(&mut *conn)
            .await
            .unwrap_or(false);
        if claimed {
            return Some(slot);
        }
    }
    eprintln!(
        "test database: all {MAX_SLOTS} slots are in use; is something running more than \
         {MAX_SLOTS} tests at once?"
    );
    None
}

/// Give the claimed slot back the schema and nothing else.
///
/// Recreated from the template rather than emptied table by table, because
/// "empty" is more than truncation: a script's `database.createTable` leaves a
/// table the schema never mentions, and a test that asserts on what tables
/// exist would see the previous test's. `WITH (FORCE)` because a process killed
/// by a timeout can leave connections that outlive it by a moment.
async fn reset_slot(
    conn: &mut sqlx::postgres::PgConnection,
    name: &str,
) -> Result<(), sqlx::Error> {
    // Identifiers, not values: neither can be bound as a parameter, and both
    // are built here rather than taken from anywhere a test can reach.
    sqlx::query(sqlx::AssertSqlSafe(format!(
        r#"DROP DATABASE IF EXISTS "{name}" WITH (FORCE)"#
    )))
    .execute(&mut *conn)
    .await?;

    // Retried, because `CREATE DATABASE ... TEMPLATE` refuses while anything is
    // connected to the source and the one thing that connects to the template
    // is another process migrating it. That window is short and does not recur,
    // so waiting is the whole remedy; every other error is returned as it is.
    let mut last = None;
    for attempt in 0..ATTEMPTS {
        let statement = format!(r#"CREATE DATABASE "{name}" TEMPLATE "{TEMPLATE_DB}""#);
        match sqlx::query(sqlx::AssertSqlSafe(statement))
            .execute(&mut *conn)
            .await
        {
            Ok(_) => return Ok(()),
            Err(error) if source_is_busy(&error) => {
                last = Some(error);
                tokio::time::sleep(Duration::from_millis(100 * u64::from(attempt + 1))).await;
            }
            Err(error) => return Err(error),
        }
    }
    Err(last.unwrap_or(sqlx::Error::PoolClosed))
}

fn source_is_busy(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(db) if db.code().as_deref() == Some("55006"))
}

/// Make sure the template exists and holds the current schema.
///
/// The check is a string comparison against a comment on the database, because
/// the alternative — connecting to the template to read `_sqlx_migrations` — is
/// the one thing that blocks every other process's `CREATE DATABASE`. The
/// comment says which migrations the template was built from; when a migration
/// is added it stops matching and the template is brought up to date, so a new
/// migration does not need anyone to remember to drop it by hand.
async fn ensure_template(
    conn: &mut sqlx::postgres::PgConnection,
    base: &str,
) -> anyhow::Result<()> {
    let wanted = migrations_fingerprint()?;
    if template_fingerprint(conn).await? == Some(wanted.clone()) {
        return Ok(());
    }

    // Only one process should build it, and the rest should wait rather than
    // race — this is the blocking form for that reason.
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(TEMPLATE_LOCK)
        .execute(&mut *conn)
        .await?;

    let built = build_template(conn, base, &wanted).await;

    let _ = sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(TEMPLATE_LOCK)
        .execute(&mut *conn)
        .await;

    built
}

async fn build_template(
    conn: &mut sqlx::postgres::PgConnection,
    base: &str,
    wanted: &str,
) -> anyhow::Result<()> {
    // Another process may have built it while this one waited for the lock.
    if template_fingerprint(conn).await? == Some(wanted.to_string()) {
        return Ok(());
    }

    let exists: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_database WHERE datname = $1)")
            .bind(TEMPLATE_DB)
            .fetch_one(&mut *conn)
            .await?;

    if !exists {
        sqlx::query(sqlx::AssertSqlSafe(format!(
            r#"CREATE DATABASE "{TEMPLATE_DB}""#
        )))
        .execute(&mut *conn)
        .await?;
    }

    migrate_template(base).await?;

    // Written only once the migrations have landed, so an interrupted build
    // leaves a template that says it is out of date rather than one that lies.
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "COMMENT ON DATABASE \"{TEMPLATE_DB}\" IS '{wanted}'"
    )))
    .execute(&mut *conn)
    .await?;

    Ok(())
}

async fn template_fingerprint(
    conn: &mut sqlx::postgres::PgConnection,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT shobj_description(oid, 'pg_database') FROM pg_database WHERE datname = $1",
    )
    .bind(TEMPLATE_DB)
    .fetch_optional(conn)
    .await
    .map(Option::flatten)
}

/// What the migrations directory currently holds, as a short string.
///
/// Both the count and the newest version, so that adding a migration and
/// replacing one both change it. Deliberately not a digest of the contents:
/// editing a migration that has already been applied is not something a
/// template rebuild could honour anyway, since `sqlx` would refuse the
/// checksum mismatch — and that refusal is the message worth seeing.
fn migrations_fingerprint() -> anyhow::Result<String> {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let mut count = 0usize;
    let mut newest = 0u64;
    for entry in std::fs::read_dir(&dir)? {
        let name = entry?.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".sql") {
            continue;
        }
        count += 1;
        if let Some(version) = name.split('_').next().and_then(|v| v.parse::<u64>().ok()) {
            newest = newest.max(version);
        }
    }
    anyhow::ensure!(count > 0, "no migrations found in {}", dir.display());
    Ok(format!(
        "aiwebengine test template: {count} migrations, newest {newest}"
    ))
}

/// Bring the template's schema up to date.
///
/// Through `Database` rather than `sqlx::migrate!` directly, so the template is
/// migrated by the same path a deployment uses — including clearing the session
/// guards, which a migration that rewrites a table needs.
///
/// The pool is closed before returning, and that is not tidiness: Postgres
/// refuses `CREATE DATABASE ... TEMPLATE` while any session is connected to the
/// source, so a pool left open here would block every slot reset in the run.
async fn migrate_template(base: &str) -> anyhow::Result<()> {
    let template_url = with_database(base, TEMPLATE_DB)
        .ok_or_else(|| anyhow::anyhow!("cannot name the template database"))?;

    let mut repository = aiwebengine::config::AppConfig::test_config_postgres(0).repository;
    repository.connection_string = template_url;
    // One connection: migrating is sequential, and every one of them has to be
    // gone before the first slot can be recreated.
    repository.max_connections = 1;

    let database = aiwebengine::database::Database::new(&repository).await?;
    let migrated = database.migrate().await;
    database.pool().close().await;
    migrated
}
