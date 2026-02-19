use aiwebengine::{config, start_server_with_config};
use std::sync::Arc;
use std::sync::Once;
use std::time::Duration;
use tokio::sync::{Mutex, oneshot};

static INIT: Once = Once::new();

pub fn init_tracing() {
    INIT.call_once(|| {
        tracing_subscriber::fmt().with_env_filter("debug").init();
    });
}

/// Check if database is available for integration tests
#[allow(dead_code)]
pub fn should_skip_integration_tests() -> bool {
    std::env::var("DATABASE_URL").is_err()
}

/// Improved test server with proper shutdown support
pub struct TestServer {
    port: u16,
    shutdown_tx: Option<oneshot::Sender<()>>,
}

impl TestServer {
    /// Start a test server with automatic port selection and shutdown support
    #[allow(dead_code)]
    pub async fn start() -> anyhow::Result<Self> {
        Self::start_with_storage("postgresql").await
    }

    /// Start a test server with specific storage type (only PostgreSQL supported now)
    pub async fn start_with_storage(_storage_type: &str) -> anyhow::Result<Self> {
        // Only PostgreSQL is supported now
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
        Self {
            servers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Start a new server and add it to the context
    #[allow(dead_code)]
    pub async fn start_server(&self) -> anyhow::Result<u16> {
        self.start_server_with_storage("postgresql").await
    }

    /// Start a new server with specific storage and add it to the context (only PostgreSQL supported now)
    #[allow(dead_code)]
    pub async fn start_server_with_storage(&self, _storage_type: &str) -> anyhow::Result<u16> {
        let server = TestServer::start_with_storage("postgresql").await?;
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
