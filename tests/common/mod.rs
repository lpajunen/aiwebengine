use aiwebengine::{config, start_server_with_config};
use std::sync::Arc;
use std::sync::{Once, OnceLock};
use std::time::Duration;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore, oneshot};

static INIT: Once = Once::new();
static DB_INIT: Once = Once::new();
static DB_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

/// Global semaphore (capacity = 1) used to serialize integration tests.
///
/// Because `execute_startup_scripts()` reads *all* scripts from the shared
/// database and updates process-global state (`DYNAMIC_SCRIPTS`,
/// `GRAPHQL_REGISTRY`, per-script metadata), running multiple test servers
/// concurrently causes race conditions: server A picks up scripts that were
/// just inserted by test B, producing non-deterministic route registrations.
///
/// Holding a single permit for the full lifetime of each `TestServer`
/// guarantees that only one server is starting, running, and shutting down at
/// any given time, eliminating those races without requiring per-test database
/// isolation.
static TEST_SEMAPHORE: OnceLock<Arc<Semaphore>> = OnceLock::new();

/// A throwaway 32-byte key, base64-encoded, for the tests that run with
/// authentication on. Never used anywhere a real deployment reads a key from.
#[allow(dead_code)]
const TEST_ENCRYPTION_KEY: &str = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=";

fn get_test_semaphore() -> Arc<Semaphore> {
    TEST_SEMAPHORE
        .get_or_init(|| Arc::new(Semaphore::new(1)))
        .clone()
}

fn get_db_runtime() -> &'static tokio::runtime::Runtime {
    DB_RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap()
    })
}

pub fn init_tracing() {
    INIT.call_once(|| {
        tracing_subscriber::fmt().with_env_filter("debug").init();
    });
}

/// Initialize test database and repository.
///
/// Resolves the connection string exactly the way the test server does, via
/// `Config::test_config_postgres` — `DATABASE_URL` when set, the default local
/// connection string otherwise. Resolving it here rather than reading
/// `DATABASE_URL` directly keeps the harness and the server pointed at the same
/// database: previously an unset `DATABASE_URL` made this function return
/// without initializing anything, while the server still came up on the default
/// string, so a test that touched the repository *before* starting its server
/// panicked in `get_repository()` with no hint of the real cause.
///
/// Uses a persistent global runtime so the pool maintenance tasks stay alive.
/// Safe to call multiple times — only runs once per process.
#[allow(dead_code)]
pub fn init_test_db() {
    DB_INIT.call_once(|| {
        if std::env::var("DATABASE_URL").is_err() {
            eprintln!(
                "warning: DATABASE_URL is not set; falling back to the default local \
                 connection string. Run `source .env` (or use `make test`) to choose \
                 the database explicitly."
            );
        }

        /// The globals a test *server* runs against.
        ///
        /// Registers a server id, because the server generates one at startup
        /// and the repository has to stamp its notifications with the same one.
        async fn do_init() {
            let Some(pool) = open_database().await else {
                return;
            };
            let server_id = aiwebengine::notifications::generate_server_id();
            let _ = aiwebengine::notifications::initialize_server_id(server_id.clone());
            let _ = aiwebengine::repository::initialize_repository(
                aiwebengine::repository::PostgresRepository::new(pool, server_id),
            );
        }

        match tokio::runtime::Handle::try_current() {
            // Already inside a tokio runtime (e.g. called from `#[tokio::test]`).
            // `block_in_place` runs the setup without blocking the runtime thread.
            Ok(handle) => tokio::task::block_in_place(|| handle.block_on(do_init())),
            // No active runtime (a plain `#[test]`, or lazy init). The global
            // persistent runtime keeps the pool's maintenance tasks alive past
            // this scope.
            Err(_) => get_db_runtime().block_on(do_init()),
        }
    });
}

/// Check if database is available for integration tests
#[allow(dead_code)]
pub fn should_skip_integration_tests() -> bool {
    std::env::var("DATABASE_URL").is_err()
}

/// Bring up the process-global database and repository the suite shares.
///
/// Every integration test needs the same three things standing before it can
/// do anything: a pool, the global database, and the repository built on it.
/// Each test file used to carry its own copy of that — twenty-seven of them,
/// identical but for an import prefix — which made the way the suite reaches a
/// database a thing declared in twenty-seven places rather than one.
///
/// Idempotent, and once per test binary: `mod common` is compiled into each
/// one, so the cell below initialises exactly where the hand-rolled copies did.
///
/// This is also the one place the suite names a backend. A second backend
/// becomes a branch here and a `Repository` to construct, rather than an edit
/// to every file that touches a table.
#[allow(dead_code)]
pub async fn setup_env() {
    GLOBALS.get_or_init(build_globals).await;
}

static GLOBALS: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();

/// Stand up the global database, and hand back the pool to build a repository
/// on.
///
/// The one place the suite opens a connection. Built through `Database::new`
/// rather than from a bare pool, because that is what carries the session
/// guards: `lock_timeout`, `statement_timeout` and
/// `idle_in_transaction_session_timeout` ride in the startup packet, and a
/// pool assembled without them behaves differently under contention than the
/// server does. Both entry points below used to build their own, one guarded
/// and one not, and which a test got came down to which ran first.
///
/// `None` when the database will not come up. Callers leave the globals unset
/// rather than panicking: `should_skip_integration_tests` decides whether a
/// test runs, and it should get to make that call.
async fn open_database() -> Option<sqlx::PgPool> {
    let config = aiwebengine::config::AppConfig::test_config_postgres(0);
    let db = Arc::new(
        aiwebengine::database::Database::new(&config.repository)
            .await
            .ok()?,
    );
    let pool = db.pool().clone();
    aiwebengine::database::initialize_global_database(db);
    Some(pool)
}

/// Construct the globals for a test that drives the engine in-process.
///
/// No server id is registered, which is what the hand-rolled fixtures did and
/// what the tests using them are written against — `stream_registry` skips
/// sending a notification when there is no id, so registering one here would
/// quietly turn on cluster chatter in tests that never expected it. The
/// server-backed path below registers one because a real server does.
async fn build_globals() {
    let Some(pool) = open_database().await else {
        return;
    };
    aiwebengine::repository::initialize_repository(
        aiwebengine::repository::PostgresRepository::new(pool, "test".to_string()),
    );
}

/// Serialises the tests in one binary against each other.
///
/// Scripts, registrations, the repository and the script caches are all
/// process-global, so two tests running at once read each other's writes. Take
/// this at the top of any test that touches them and hold it for the test's
/// length.
///
/// One mutex per test binary, which is the same scope the copies in individual
/// test files had. It is unrelated to `TEST_SEMAPHORE`, which serialises whole
/// test *servers* rather than the tests inside one binary.
#[allow(dead_code)]
pub fn test_mutex() -> &'static Mutex<()> {
    static TEST_MUTEX: OnceLock<Mutex<()>> = OnceLock::new();
    TEST_MUTEX.get_or_init(|| Mutex::new(()))
}

/// Improved test server with proper shutdown support
pub struct TestServer {
    port: u16,
    shutdown_tx: Option<oneshot::Sender<()>>,
    /// Held for the server's entire lifetime so no other test server can start.
    _permit: OwnedSemaphorePermit,
}

impl TestServer {
    /// Start a test server with automatic port selection and shutdown support.
    ///
    /// Acquires the global serialization permit first so that only one server
    /// is running at a time — avoiding races in `execute_startup_scripts()`.
    #[allow(dead_code)]
    pub async fn start() -> anyhow::Result<Self> {
        // Serialize: wait until no other test server is running.
        let permit = get_test_semaphore()
            .acquire_owned()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to acquire test semaphore: {}", e))?;

        let mut test_config = config::Config::test_config_postgres(0);

        // Disable auth for tests by default to avoid overhead
        test_config.auth = None;

        // Set faster timeout for tests
        test_config.javascript.execution_timeout_ms = 5000; // 5 second timeout for tests

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

        let port = start_server_with_config(test_config, shutdown_rx).await?;

        Ok(Self {
            port,
            shutdown_tx: Some(shutdown_tx),
            _permit: permit,
        })
    }

    /// Start a test server with authentication on, for tests that walk a
    /// sign-in or an OAuth2 flow rather than script behaviour.
    ///
    /// The port is chosen before startup rather than by binding zero, because
    /// the engine's base URL has to name the host and port the test addresses
    /// it by: a token's audience is built from the configured host on one side
    /// and checked against the request's host on the other, and a base URL
    /// naming somewhere else makes every such comparison fail for a reason
    /// that has nothing to do with what is being tested.
    #[allow(dead_code)]
    pub async fn start_with_auth() -> anyhow::Result<Self> {
        use aiwebengine::auth::config::{AuthConfig, CookieConfig, InternalAuthConfig};

        let permit = get_test_semaphore()
            .acquire_owned()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to acquire test semaphore: {}", e))?;

        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
            listener.local_addr()?.port()
        };

        let mut test_config = config::Config::test_config_postgres(port);
        test_config.server.base_url = Some(format!("http://127.0.0.1:{}", port));
        test_config.javascript.execution_timeout_ms = 5000;

        // Not development mode: that tier hands anonymous callers an
        // administrator's capabilities, which is the opposite of what a test
        // about authentication should be running under. The keys it stands in
        // for are supplied instead.
        test_config.security.development_mode = false;
        test_config.security.csrf_key = Some(TEST_ENCRYPTION_KEY.to_string());
        test_config.security.session_encryption_key = Some(TEST_ENCRYPTION_KEY.to_string());
        test_config.security.secret_encryption_key = Some(TEST_ENCRYPTION_KEY.to_string());

        test_config.auth = Some(AuthConfig {
            jwt_secret: "test-jwt-secret-of-at-least-32-characters".to_string(),
            // Served over plain HTTP here, so a `Secure` cookie would never be
            // sent back and every signed-in step would look signed out.
            cookie: CookieConfig {
                secure: false,
                ..CookieConfig::default()
            },
            internal: InternalAuthConfig {
                enabled: true,
                allow_registration: true,
                allow_guests: false,
                ..InternalAuthConfig::default()
            },
            ..AuthConfig::default()
        });

        let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
        let port = start_server_with_config(test_config, shutdown_rx).await?;

        Ok(Self {
            port,
            shutdown_tx: Some(shutdown_tx),
            _permit: permit,
        })
    }

    /// Get the port the server is running on
    #[allow(dead_code)]
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Shutdown the server gracefully
    #[allow(dead_code)]
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
            // Give server time to shut down
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
    }
}

/// Test context with server pool for better performance
pub struct TestContext {
    servers: Arc<Mutex<Vec<TestServer>>>,
}

impl TestContext {
    pub fn new() -> Self {
        init_tracing();
        init_test_db();
        Self {
            servers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Start a new server and add it to the context
    #[allow(dead_code)]
    pub async fn start_server(&self) -> anyhow::Result<u16> {
        let server = TestServer::start().await?;
        let port = server.port();
        self.servers.lock().await.push(server);
        Ok(port)
    }

    /// Cleanup all servers
    #[allow(dead_code)]
    pub async fn cleanup(&self) -> anyhow::Result<()> {
        let mut servers = self.servers.lock().await;
        for server in servers.drain(..) {
            server.shutdown().await;
        }
        Ok(())
    }
}

impl Default for TestContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Wait for server to be ready with retries
#[allow(dead_code)]
pub async fn wait_for_server(port: u16, max_attempts: u32) -> anyhow::Result<()> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_millis(500))
        .build()?;

    for attempt in 1..=max_attempts {
        // Try to connect to the health endpoint or root
        if let Ok(response) = client
            .get(format!("http://127.0.0.1:{}/health", port))
            .send()
            .await
            && (response.status().is_success() || response.status().is_client_error())
        {
            return Ok(());
        }

        if attempt < max_attempts {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    Err(anyhow::anyhow!(
        "Server not ready after {} attempts",
        max_attempts
    ))
}

/// Macro for running tests with automatic server management
#[macro_export]
macro_rules! with_test_server {
    ($test_body:expr) => {{
        let context = $crate::common::TestContext::new();
        let port = context
            .start_server()
            .await
            .expect("Failed to start test server");

        // Wait for server to be ready
        $crate::common::wait_for_server(port, 20)
            .await
            .expect("Server not ready");

        let result = $test_body(port).await;

        // Cleanup
        context
            .cleanup()
            .await
            .expect("Failed to cleanup test server");

        result
    }};
}
