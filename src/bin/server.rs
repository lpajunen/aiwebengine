use aiwebengine::start_server;
use tokio::sync::oneshot;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize tracing subscriber for structured logging
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::from_default_env())
        .with(tracing_subscriber::fmt::layer().json())
        .init();

    // Create a one-shot channel for graceful shutdown signaling
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    // Spawn the server task that listens until shutdown_rx receives a value
    let server_task = tokio::spawn(async move {
        if let Err(e) = start_server(shutdown_rx).await {
            tracing::error!("server error: {}", e);
        }
    });

    // Wait for Ctrl-C in the main task
    tokio::signal::ctrl_c().await?;
    tracing::info!("shutdown requested, stopping server...");

    // Signal the server to start graceful shutdown. Ignore send errors if the
    // server already exited.
    let _ = shutdown_tx.send(());

    // Wait for server task to finish; give it a short timeout if you want to
    // bound shutdown time. Here we wait until it finishes naturally.
    let _ = server_task.await;

    tracing::info!("server stopped");
    Ok(())
}
