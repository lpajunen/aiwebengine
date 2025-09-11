use aiwebengine::start_server;
use tokio::sync::oneshot;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Create a one-shot channel for graceful shutdown signaling
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    // Spawn the server task that listens until shutdown_rx receives a value
    let server_task = tokio::spawn(async move {
        if let Err(e) = start_server(shutdown_rx).await {
            eprintln!("server error: {}", e);
        }
    });

    // Wait for Ctrl-C in the main task
    tokio::signal::ctrl_c().await?;
    println!("shutdown requested, stopping server...");

    // Signal the server to start graceful shutdown. Ignore send errors if the
    // server already exited.
    let _ = shutdown_tx.send(());

    // Wait for server task to finish; give it a short timeout if you want to
    // bound shutdown time. Here we wait until it finishes naturally.
    let _ = server_task.await;

    println!("server stopped");
    Ok(())
}
