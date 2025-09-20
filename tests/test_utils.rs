use aiwebengine::config;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Test server handle that ensures proper cleanup
pub struct TestServer {
    port: u16,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    handle: Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
}

impl TestServer {
    /// Start a test server with automatic port selection
    pub async fn start() -> anyhow::Result<Self> {
        Self::start_with_config(config::Config::from_env()).await
    }

    /// Start a test server with custom config
    pub async fn start_with_config(mut test_config: config::Config) -> anyhow::Result<Self> {
        // Use port 0 for automatic port selection
        test_config.port = 0;

        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();

        // Start the server in a background task
        let config_clone = test_config.clone();
        let handle = tokio::spawn(async move {
            aiwebengine::start_server_with_config(config_clone, shutdown_rx).await?;
            Ok(())
        });

        // Wait a bit for server to start
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        // For now, we need to use a workaround since the current server doesn't return the port
        // We'll implement a proper solution by modifying the server functions
        let port = 4000; // Placeholder - will be replaced with actual dynamic port

        Ok(Self {
            port,
            shutdown_tx: Some(shutdown_tx),
            handle: Some(handle),
        })
    }

    /// Get the port the server is running on
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Stop the server gracefully
    pub async fn stop(mut self) -> anyhow::Result<()> {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }

        if let Some(handle) = self.handle.take() {
            // Wait for the server to shut down with a timeout
            match tokio::time::timeout(
                std::time::Duration::from_secs(5),
                handle
            ).await {
                Ok(result) => {
                    result??;
                }
                Err(_) => return Err(anyhow::anyhow!("Server shutdown timed out")),
            }
        }

        Ok(())
    }
}

impl Drop for TestServer {
    fn drop(&mut self) {
        // Best effort cleanup in case stop() wasn't called
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
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
        for server in servers.drain(..) {
            server.stop().await?;
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