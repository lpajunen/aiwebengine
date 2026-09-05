use aiwebengine::{config, start_server_with_config};
use std::sync::Arc;

pub mod testdb;
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

/// The administrator the engine-API tests act as. Named in the test server's
/// `bootstrap_admin_usernames`, so signing in as it is signing in as an
/// administrator.
#[allow(dead_code)]
pub const TEST_ADMIN_USERNAME: &str = "test-admin";

/// Its password. A test fixture, not a secret.
#[allow(dead_code)]
pub const TEST_ADMIN_PASSWORD: &str = "test-administrator-password";

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
    let config = test_config(0).await?;
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
/// The server id is registered here and handed to the repository, so that the
/// two agree. They are what tells an instance's own writes apart from a peer's:
/// the repository stamps every notification with the id it was built with, and
/// the listener drops the ones carrying its own. This used to register no id at
/// all and build the repository with a fixed `"test"`, which was harmless right
/// up until a test called `setup_env()` and *then* started a server — the
/// server generates an id, finds the repository already built, and leaves it
/// stamping notifications with `"test"`. Every write the instance made then
/// came back looking like a peer's, so a script was re-initialised a second
/// time concurrently with the initialisation its own write had already
/// spawned, and the two passes each cleared the script's listeners before
/// registering their own — leaving the script listening twice. Which of the
/// two dispatch tests saw it depended on which won the race.
async fn build_globals() {
    let Some(pool) = open_database().await else {
        return;
    };
    let server_id = aiwebengine::notifications::generate_server_id();
    let _ = aiwebengine::notifications::initialize_server_id(server_id.clone());
    aiwebengine::repository::initialize_repository(
        aiwebengine::repository::PostgresRepository::new(pool, server_id),
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

/// How many connections one test process may hold.
///
/// Sized for a suite rather than for a deployment, and the right number depends
/// on which runner is driving: what has to stay under the server's
/// `max_connections` is the total across every process running at once, and the
/// two runners spread the same tests over very different numbers of processes.
///
/// `cargo nextest` runs each test in its own process — several at a time, each
/// needing only what one test needs — so a small pool per process is both
/// enough and necessary; the deployment default of 20 across eight of them asks
/// for more connections than Postgres will hand out, and the process that loses
/// fails on the pool rather than on anything it was checking. `cargo test` runs
/// one binary at a time and its tests on threads inside it, so a single pool is
/// shared by as many tests as there are cores and has to be sized for all of
/// them.
fn pool_size() -> u32 {
    if std::env::var("NEXTEST").is_ok() {
        5
    } else {
        24
    }
}

/// How many ports to try before giving up on starting an authenticated server.
const PORT_ATTEMPTS: u32 = 5;

/// A port nothing is listening on, as of a moment ago.
///
/// Only for the servers that cannot bind port zero — an engine whose base URL
/// has to name the port it is reachable on has to know the port before it
/// starts. Everything else lets the operating system choose while binding,
/// which has no window to race in.
fn free_port() -> anyhow::Result<u16> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(listener.local_addr()?.port())
}

/// The configuration a test drives the engine with.
///
/// The one place the suite decides which database a test runs against, so that
/// the harness, the engine and any test building its own `RepositoryConfig`
/// all name the same one. Every call in this process answers with the same
/// per-process database — see [`testdb`] for why there is one.
///
/// `None` when the database server will not answer; `should_skip_integration_tests`
/// is what decides whether that should fail a test or skip it.
#[allow(dead_code)]
pub async fn test_config(port: u16) -> Option<config::Config> {
    let mut config = config::Config::test_config_postgres(port);
    config.repository.connection_string = testdb::connection_string().await?.to_string();
    config.repository.max_connections = pool_size();
    Some(config)
}

/// A pool on this process's test database.
///
/// For a test that drives a component — a session manager, a rate limiter —
/// directly rather than through a server. It replaced a connection string
/// written out longhand in each such test, which named the developer's own
/// database and so ignored both `DATABASE_URL` and the isolation in
/// [`testdb`]: those tests wrote their sessions, rate-limit buckets and audit
/// rows into whatever database the machine had, and left them there.
#[allow(dead_code)]
pub async fn test_pool() -> sqlx::PgPool {
    let url = testdb::connection_string()
        .await
        .expect("the test database should be reachable");
    sqlx::PgPool::connect_lazy(url).expect("the test database URL should parse")
}

/// The same, for a test that cannot proceed without one.
#[allow(dead_code)]
pub async fn require_test_config(port: u16) -> config::Config {
    test_config(port)
        .await
        .expect("the test database should be reachable")
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
        Self::start_customized(|_| {}).await
    }

    /// The same, with a chance to change the configuration first.
    ///
    /// For a test about a setting rather than about script behaviour. The test
    /// configuration is built in code rather than loaded through figment, so
    /// an environment variable would not reach it.
    #[allow(dead_code)]
    pub async fn start_customized(
        customize: impl FnOnce(&mut config::Config),
    ) -> anyhow::Result<Self> {
        // Serialize: wait until no other test server is running.
        let permit = get_test_semaphore()
            .acquire_owned()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to acquire test semaphore: {}", e))?;

        let mut test_config = require_test_config(0).await;

        // Disable auth for tests by default to avoid overhead
        test_config.auth = None;

        // Set faster timeout for tests
        test_config.javascript.execution_timeout_ms = 5000; // 5 second timeout for tests

        customize(&mut test_config);

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
        Self::start_with_auth_customized(|_| {}).await
    }

    /// The same, with a chance to change the configuration first.
    #[allow(dead_code)]
    pub async fn start_with_auth_customized(
        customize: impl FnOnce(&mut config::Config),
    ) -> anyhow::Result<Self> {
        use aiwebengine::auth::config::{AuthConfig, CookieConfig, InternalAuthConfig};

        let permit = get_test_semaphore()
            .acquire_owned()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to acquire test semaphore: {}", e))?;

        let port = free_port()?;

        let mut test_config = require_test_config(port).await;
        test_config.server.base_url = Some(format!("http://127.0.0.1:{}", port));
        test_config.javascript.execution_timeout_ms = 5000;

        // Authentication needs its keys; throwaway ones, per test process.
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
                // The fixed administrator. Engine APIs are guarded by
                // AdministerEngine, and this is how a test holds it: by being
                // an administrator, the way a person is, rather than by running
                // the engine in a mode that hands the capability to anonymous
                // callers.
                bootstrap_admin_usernames: vec![TEST_ADMIN_USERNAME.to_string()],
                // On, like every shipped template, so the suite exercises the
                // recovery endpoints the way a deployment has them. The tests
                // that are about the switch being off start their own server.
                allow_recovery_codes: true,
                ..InternalAuthConfig::default()
            },
            ..AuthConfig::default()
        });

        customize(&mut test_config);

        // Retried on a fresh port, because choosing one and binding it are two
        // steps: the listener above is closed before the server opens its own,
        // and with tests running in parallel another process can take the port
        // in between. Only the port this helper picked is moved — a test that
        // set its own base URL keeps it, since for those the port is part of
        // what is being tested.
        let chose_base_url =
            test_config.server.base_url == Some(format!("http://127.0.0.1:{}", port));

        let mut attempt = 0;
        loop {
            attempt += 1;
            let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
            match start_server_with_config(test_config.clone(), shutdown_rx).await {
                Ok(port) => {
                    return Ok(Self {
                        port,
                        shutdown_tx: Some(shutdown_tx),
                        _permit: permit,
                    });
                }
                Err(error) if attempt < PORT_ATTEMPTS && chose_base_url => {
                    let next = free_port()?;
                    test_config.server.port = next;
                    test_config.server.base_url = Some(format!("http://127.0.0.1:{}", next));
                    eprintln!("test server: port {port} would not bind ({error}); trying {next}");
                }
                Err(error) => return Err(error.into()),
            }
        }
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
        self.start_server_customized(|_| {}).await
    }

    /// The same, with a chance to change the configuration first.
    #[allow(dead_code)]
    pub async fn start_server_customized(
        &self,
        customize: impl FnOnce(&mut config::Config),
    ) -> anyhow::Result<u16> {
        let server = TestServer::start_customized(customize).await?;
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

/// A running engine and an administrator signed into it.
///
/// What the engine's own APIs need, and what replaced development mode: those
/// endpoints are guarded by `AdministerEngine`, and a test now holds that
/// capability by being an administrator rather than by running the engine in a
/// mode that hands it to anonymous callers. The requests a test makes are then
/// the requests a real administrator makes, through the middleware a real
/// deployment runs.
pub struct AdminServer {
    server: TestServer,
    http: reqwest::Client,
    cookie: String,
    base: String,
}

#[allow(dead_code)]
impl AdminServer {
    /// Start the server and sign in as [`TEST_ADMIN_USERNAME`].
    pub async fn start() -> anyhow::Result<Self> {
        Self::start_customized(|_| {}).await
    }

    /// The same, with a chance to change the configuration first — for a test
    /// about a switch this server does not have in its default position.
    pub async fn start_customized(
        customize: impl FnOnce(&mut config::Config),
    ) -> anyhow::Result<Self> {
        let server = TestServer::start_with_auth_customized(customize).await?;
        let port = server.port();
        wait_for_server(port, 30).await?;

        let http = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .build()?;
        let base = format!("http://127.0.0.1:{}", port);
        let cookie = admin_session(&http, &base).await?;

        Ok(Self {
            server,
            http,
            cookie,
            base,
        })
    }

    pub fn port(&self) -> u16 {
        self.server.port()
    }

    /// The engine's address, for a request this helper does not shape.
    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    /// A client that carries the administrator's session on every request.
    ///
    /// Handed out whole rather than wrapped, so a test that was written against
    /// a bare `reqwest::Client` keeps its requests exactly as they were and
    /// only changes where the client comes from. What changes is that the
    /// requests now arrive as somebody.
    pub fn client(&self) -> reqwest::Client {
        self.client_builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| self.http.clone())
    }

    /// The same session, for a test that wants its own redirect or timeout
    /// policy on top.
    pub fn client_builder(&self) -> reqwest::ClientBuilder {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Ok(value) = reqwest::header::HeaderValue::from_str(&self.cookie) {
            headers.insert(reqwest::header::COOKIE, value);
        }

        reqwest::Client::builder().default_headers(headers)
    }

    /// A request carrying the administrator's session.
    pub fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, self.url(path))
            .header(reqwest::header::COOKIE, &self.cookie)
    }

    pub fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.request(reqwest::Method::GET, path)
    }

    pub fn post(&self, path: &str) -> reqwest::RequestBuilder {
        self.request(reqwest::Method::POST, path)
    }

    pub fn delete(&self, path: &str) -> reqwest::RequestBuilder {
        self.request(reqwest::Method::DELETE, path)
    }

    pub fn patch(&self, path: &str) -> reqwest::RequestBuilder {
        self.request(reqwest::Method::PATCH, path)
    }

    /// A caller with no session at all, for the assertions that are about
    /// being refused.
    pub fn anonymous(&self) -> &reqwest::Client {
        &self.http
    }

    /// Store a script and initialise it, the way the engine's own write
    /// endpoint does.
    ///
    /// Writing the row is not deploying. `repository::upsert_script` stores the
    /// source and nothing else, so no route, resolver, stream or listener the
    /// script registers exists until something calls its `init()` — the engine
    /// does that for every script at startup, and for one script on each write
    /// through `/engine/upsert_script`. A test that wrote the row directly
    /// *after* starting its server did neither, and passed only because an
    /// earlier run had left the same script in the shared database for this
    /// run's startup to execute. On a database of its own, that test finds
    /// nothing registered.
    ///
    /// Awaited rather than spawned, unlike the write endpoint's own
    /// initialisation: a test that has to poll for its script to come up is a
    /// test with a timing assumption in it.
    pub async fn deploy_script(&self, uri: &str, content: &str) {
        aiwebengine::repository::upsert_script(uri, content)
            .unwrap_or_else(|e| panic!("storing '{uri}' should succeed: {e}"));
        self.initialize_script(uri).await;
    }

    /// The second half of [`deploy_script`], for a script whose assets have to
    /// be written between storing it and running it.
    ///
    /// An asset is keyed by the script that owns it, so the script row has to
    /// exist first; and a script importing a module cannot be built until that
    /// module is stored. Those two orderings leave one gap to write the assets
    /// into, which is why this half is separate.
    ///
    /// [`deploy_script`]: AdminServer::deploy_script
    pub async fn initialize_script(&self, uri: &str) {
        let result = aiwebengine::script_init::ScriptInitializer::with_configured_timeout()
            .initialize_script(uri, false)
            .await
            .unwrap_or_else(|e| panic!("initialising '{uri}' should be attempted: {e}"));

        assert!(
            result.success,
            "initialising '{uri}' should succeed: {:?}",
            result.error
        );
    }

    pub async fn shutdown(self) {
        self.server.shutdown().await;
    }
}

/// Give this test's sign-ins a fresh per-address budget.
///
/// The auth rate limiter is database-backed and keyed by address, and every
/// test in the suite arrives from 127.0.0.1 — so one bucket is shared by
/// hundreds of registrations and sign-ins that a real deployment would spread
/// over hundreds of clients. Left alone it empties partway through a run and
/// the rest of the suite gets 429s that say nothing about the code under test.
///
/// The same trick `tests/mcp_oauth_flow.rs` uses on the registration budget,
/// and for the same reason.
async fn clear_auth_rate_limit() {
    let Some(db) = aiwebengine::database::get_global_database() else {
        return;
    };
    let _ =
        sqlx::query("DELETE FROM rate_limits WHERE key LIKE 'ip:%' OR key LIKE 'login_failure:%'")
            .execute(db.pool())
            .await;
}

/// Sign the fixed administrator in, creating the account the first time.
///
/// Every test process shares one database, so the account usually exists
/// already; registration and sign-in both end in the same place, which is a
/// session cookie for an account the configuration names an administrator.
async fn admin_session(http: &reqwest::Client, base: &str) -> anyhow::Result<String> {
    clear_auth_rate_limit().await;

    let credentials = serde_json::json!({
        "username": TEST_ADMIN_USERNAME,
        "password": TEST_ADMIN_PASSWORD,
    });

    let registered = http
        .post(format!("{}/auth/local/register", base))
        .json(&credentials)
        .send()
        .await?;

    let response = if registered.status().is_success() {
        registered
    } else {
        http.post(format!("{}/auth/local/login", base))
            .json(&credentials)
            .send()
            .await?
    };

    let status = response.status();
    let cookie = response
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::to_string);

    cookie.ok_or_else(|| {
        anyhow::anyhow!(
            "signing the test administrator in issued no session: {}",
            status
        )
    })
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
