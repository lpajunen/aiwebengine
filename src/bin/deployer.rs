use clap::Parser;
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use reqwest::blocking::Client;
use std::fs;
use std::path::Path;
use std::sync::mpsc::channel;
use std::thread;
use std::time::Duration;

#[derive(Parser)]
#[command(name = "deployer")]
#[command(about = "Deploy and watch JavaScript files for aiwebengine")]
struct Args {
    /// URI for the script (e.g., https://example.com/my-script)
    #[arg(short, long)]
    uri: String,

    /// Path to the JavaScript file to deploy
    #[arg(short, long)]
    file: String,

    /// Server URL (default: http://localhost:4000)
    #[arg(short, long, default_value = "http://localhost:4000")]
    server: String,

    /// Watch for file changes (default: true)
    #[arg(short, long, default_value = "true")]
    watch: bool,
}

fn deploy_script(
    client: &Client,
    server_url: &str,
    uri: &str,
    file_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // Read the file content
    let content = fs::read_to_string(file_path)?;

    // Construct the API URL
    let api_url = format!("{}/api/scripts/{}", server_url, uri);

    println!("🚀 Deploying {} to {}", file_path, api_url);

    // Send the POST request with the file content as body
    let response = client.post(&api_url).body(content).send()?;

    if response.status().is_success() {
        println!("✅ Successfully deployed script: {}", uri);
    } else {
        println!(
            "❌ Failed to deploy script: {} (Status: {})",
            uri,
            response.status()
        );
        if let Ok(error_text) = response.text() {
            println!("Error details: {}", error_text);
        }
    }

    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    // Validate that the file exists
    if !Path::new(&args.file).exists() {
        eprintln!("❌ Error: File '{}' does not exist", args.file);
        std::process::exit(1);
    }

    // Create HTTP client
    let client = Client::builder().timeout(Duration::from_secs(30)).build()?;

    // Initial deployment
    if let Err(e) = deploy_script(&client, &args.server, &args.uri, &args.file) {
        eprintln!("❌ Initial deployment failed: {}", e);
        std::process::exit(1);
    }

    if !args.watch {
        println!("📋 One-time deployment completed. Exiting.");
        return Ok(());
    }

    println!("👀 Watching for file changes... (Press Ctrl+C to stop)");

    // Create a channel for file system events
    let (tx, rx) = channel();

    // Create a file watcher
    let mut watcher = RecommendedWatcher::new(tx, Config::default())?;

    // Watch the file
    watcher.watch(Path::new(&args.file), RecursiveMode::NonRecursive)?;

    // Main event loop
    loop {
        match rx.recv() {
            Ok(event) => {
                match event {
                    Ok(event) => {
                        // Check if it's a write event (file modified)
                        if event.kind.is_modify() || event.kind.is_create() {
                            println!("📝 File changed, redeploying...");

                            // Small delay to ensure file is fully written
                            thread::sleep(Duration::from_millis(100));

                            if let Err(e) =
                                deploy_script(&client, &args.server, &args.uri, &args.file)
                            {
                                eprintln!("❌ Redeployment failed: {}", e);
                            }
                        }
                    }
                    Err(e) => eprintln!("❌ Watch error: {:?}", e),
                }
            }
            Err(e) => {
                eprintln!("❌ Channel error: {:?}", e);
                break;
            }
        }
    }

    Ok(())
}
