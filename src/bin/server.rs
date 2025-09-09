use aiwebengine::start_server_with_script;
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Start the server in a background task so we can listen for Ctrl-C in the main task
    let server_task = tokio::spawn(async move {
        if let Err(e) = start_server_with_script("scripts/example.js").await {
            eprintln!("server error: {}", e);
        }
    });

    // Wait for Ctrl-C
    tokio::signal::ctrl_c().await?;
    println!("shutdown requested, stopping server...");

    // Ask server task to stop by aborting it; give a short grace period for cleanup
    server_task.abort();
    tokio::time::sleep(Duration::from_millis(200)).await;

    println!("server stopped");
    Ok(())
}
