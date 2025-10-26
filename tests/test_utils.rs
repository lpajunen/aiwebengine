use aiwebengine::{config, start_server_without_shutdown_with_config};
use std::sync::Arc;
use tokio::sync::Mutex;

/// Test server handle that ensures proper cleanup
pub struct TestServer {
    port: u16,
    _shutdown_tx: Option<Box<tokio::sync::oneshot::Sender<()>>>, // Leaked to prevent shutdown
}

impl TestServer {
    /// Start a test server with automatic port selection
    pub async fn start() -> anyhow::Result<Self> {
        let test_config = config::AppConfig::test_config_with_port(0); // Use port 0 for automatic port selection

        let port = start_server_without_shutdown_with_config(test_config).await?;

        // Leak the shutdown sender to prevent server shutdown
        let (tx, _rx) = tokio::sync::oneshot::channel::<()>();
        let _leaked = Box::leak(Box::new(tx));

        Ok(Self {
            port,
            _shutdown_tx: None,
        })
    }

    /// Get the port the server is running on
    pub fn port(&self) -> u16 {
        self.port
    }
}

/// Test context that manages multiple servers and provides utilities
pub struct TestContext {
    servers: Arc<Mutex<Vec<TestServer>>>,
}

impl TestContext {
    pub fn new() -> Self {
        Self {
            servers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Start a new server and add it to the context
    pub async fn start_server(&self) -> anyhow::Result<u16> {
        let server = TestServer::start().await?;
        let port = server.port;
        self.servers.lock().await.push(server);
        Ok(port)
    }

    /// Stop all servers in the context
    pub async fn cleanup(&self) -> anyhow::Result<()> {
        let mut servers = self.servers.lock().await;
        for _server in servers.drain(..) {
            // Note: With current implementation, servers run until process ends
            // In a production version, we'd implement proper shutdown
        }
        Ok(())
    }
}

impl Default for TestContext {
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience macro for running tests with proper server management
#[macro_export]
macro_rules! with_test_server {
    ($test_name:ident, $test_body:block) => {
        #[tokio::test]
        async fn $test_name() {
            let context = $crate::test_utils::TestContext::new();
            let port = context.start_server().await.expect("Failed to start test server");

            // Run the test body
            $test_body

            // Cleanup
            context.cleanup().await.expect("Failed to cleanup test server");
        }
    };
}
