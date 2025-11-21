use aiwebengine::{AppResult, config::AppConfig, start_server_with_config};
use clap::{Arg, Command};
use tokio::sync::oneshot;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

#[tokio::main]
async fn main() -> AppResult<()> {
    // Parse command line arguments
    let matches = Command::new("aiwebengine-server")
        .version("0.1.0")
        .about("AIWebEngine Server - JavaScript execution engine with web API")
        .arg(
            Arg::new("config")
                .short('c')
                .long("config")
                .value_name("FILE")
                .help("Configuration file path")
                .action(clap::ArgAction::Set),
        )
        .arg(
            Arg::new("validate")
                .long("validate-config")
                .help("Validate configuration and exit")
                .action(clap::ArgAction::SetTrue),
        )
        .get_matches();

    // Load configuration first to get logging preferences
    let config = if let Some(config_path) = matches.get_one::<String>("config") {
        AppConfig::load_from_file(config_path).map_err(|e| {
            aiwebengine::AppError::config(format!("Failed to load configuration from file: {}", e))
        })?
    } else {
        AppConfig::load().map_err(|e| {
            aiwebengine::AppError::config(format!("Failed to load configuration: {}", e))
        })?
    };

    // Initialize logging based on configuration, but allow RUST_LOG to override
    let log_level = match config.logging.level.as_str() {
        "trace" => "trace",
        "debug" => "debug",
        "info" => "info",
        "warn" => "warn",
        "error" => "error",
        _ => "info", // Default fallback
    };

    // Create filter that respects both config and environment
    // Environment variable RUST_LOG takes precedence if set
    let filter = if std::env::var("RUST_LOG").is_ok() {
        // Use environment variable if set
        tracing_subscriber::EnvFilter::from_default_env()
    } else {
        // Use configuration file setting with fallback for other crates
        tracing_subscriber::EnvFilter::new(format!("aiwebengine={},warn", log_level))
    };

    // Initialize logging based on configuration format
    match config.logging.format.as_str() {
        "json" => {
            tracing_subscriber::registry()
                .with(filter)
                .with(tracing_subscriber::fmt::layer().json())
                .init();
        }
        "compact" => {
            tracing_subscriber::registry()
                .with(filter)
                .with(tracing_subscriber::fmt::layer().compact())
                .init();
        }
        _ => {
            // "pretty" or default
            tracing_subscriber::registry()
                .with(filter)
                .with(tracing_subscriber::fmt::layer().pretty())
                .init();
        }
    }

    // Now we can log the configuration loading
    tracing::info!(
        "Loading configuration from {}",
        if matches.get_one::<String>("config").is_some() {
            "specified file"
        } else {
            "environment and default sources"
        }
    );
    tracing::info!(
        "Logging configured: level={}, format={}",
        config.logging.level,
        config.logging.format
    );
    if std::env::var("RUST_LOG").is_ok() {
        tracing::info!("RUST_LOG environment variable detected, overriding config file log level");
    }

    // Validate configuration if requested
    if matches.get_flag("validate") {
        match config.validate() {
            Ok(()) => {
                println!("✓ Configuration is valid");
                println!("Server would start on: {}", config.server_addr());
                println!("Log level: {}", config.logging.level);
                println!(
                    "JavaScript timeout: {}ms",
                    config.javascript.execution_timeout_ms
                );
                println!("Storage type: {}", config.repository.storage_type);
                return Ok(());
            }
            Err(e) => {
                eprintln!("✗ Configuration validation failed: {}", e);
                std::process::exit(1);
            }
        }
    }

    // Validate configuration during startup
    if let Err(e) = config.validate() {
        eprintln!("Configuration error: {}", e);
        return Err(aiwebengine::AppError::ConfigValidation {
            field: "configuration".to_string(),
            reason: e.to_string(),
        });
    }

    tracing::debug!("Configuration validation completed successfully");

    tracing::info!("Starting AIWebEngine Server");
    tracing::info!("Configuration loaded successfully");
    tracing::info!("Server address: {}", config.server_addr());
    tracing::info!(
        "JavaScript timeout: {}ms",
        config.javascript.execution_timeout_ms
    );
    tracing::info!(
        "Max memory per script: {} bytes",
        config.javascript.max_memory_bytes
    );
    tracing::info!("Storage type: {}", config.repository.storage_type);
    tracing::info!("CORS enabled: {}", config.security.enable_cors);
    tracing::info!(
        "Rate limiting: {} requests/minute",
        config.security.rate_limit_per_minute
    );
    tracing::info!("Auth configuration present: {}", config.auth.is_some());
    if let Some(ref auth_cfg) = config.auth {
        tracing::info!("Auth enabled: {}", auth_cfg.enabled);
        tracing::info!("Auth JWT secret length: {}", auth_cfg.jwt_secret.len());
    }

    // Create a one-shot channel for graceful shutdown signaling
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    // Clone needed values before moving config
    let graceful_shutdown = config.server.graceful_shutdown;
    let shutdown_timeout_secs = config.server.shutdown_timeout_secs;

    // Spawn the server task that listens until shutdown_rx receives a value
    let server_task = tokio::spawn(async move {
        match start_server_with_config(config, shutdown_rx).await {
            Ok(port) => tracing::info!("Server started successfully on port {}", port),
            Err(e) => tracing::error!("Server error: {}", e),
        }
    });

    // Wait for Ctrl-C in the main task
    tokio::signal::ctrl_c().await?;
    tracing::info!("Shutdown signal received, stopping server...");

    // Signal the server to start graceful shutdown. Ignore send errors if the
    // server already exited.
    let _ = shutdown_tx.send(());

    // Wait for server task to finish with timeout if graceful shutdown is enabled
    if graceful_shutdown {
        let timeout = tokio::time::Duration::from_secs(shutdown_timeout_secs);
        match tokio::time::timeout(timeout, server_task).await {
            Ok(_) => tracing::info!("Server stopped gracefully"),
            Err(_) => tracing::warn!("Server shutdown timed out after {}s", shutdown_timeout_secs),
        }
    } else {
        let _ = server_task.await;
        tracing::info!("Server stopped");
    }

    Ok(())
}
