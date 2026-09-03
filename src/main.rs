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
        .arg(
            Arg::new("grant-role")
                .long("grant-role")
                .value_names(["ACCOUNT", "ROLE"])
                .num_args(2)
                .help(
                    "Grant a role to an account and exit, e.g. --grant-role alice administrator. \
                     ACCOUNT is a local username or an email address; ROLE is administrator or \
                     editor. Needs the database but no running server, which is what makes it a \
                     way back into an engine with no administrator left.",
                )
                .action(clap::ArgAction::Set),
        )
        .arg(
            Arg::new("set-password")
                .long("set-password")
                .value_name("ACCOUNT")
                .num_args(1)
                .help(
                    "Reset an account's password and exit, e.g. --set-password alice. ACCOUNT is \
                     a local username or an email address; the new password is read from standard \
                     input, so it is never an argument other processes can see. Needs the \
                     database but no running server, which is what makes it the way back into an \
                     account whose password nobody remembers.",
                )
                .action(clap::ArgAction::Set),
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
                println!(
                    "Server would start on: {}",
                    config
                        .server_address()
                        .map_err(|e| aiwebengine::AppError::Config {
                            message: e.to_string(),
                            source: None,
                        })?
                );
                println!("Log level: {}", config.logging.level);
                println!(
                    "JavaScript timeout: {}ms",
                    config.javascript.execution_timeout_ms
                );
                println!("Storage: PostgreSQL");
                return Ok(());
            }
            Err(e) => {
                eprintln!("✗ Configuration validation failed: {}", e);
                std::process::exit(1);
            }
        }
    }

    // Grant a role and exit. Before the startup validation below, so an engine
    // whose configuration is broken can still have its owner appointed.
    if let Some(mut arguments) = matches.get_many::<String>("grant-role") {
        let account = arguments.next().cloned().unwrap_or_default();
        let role = arguments.next().cloned().unwrap_or_default();

        match aiwebengine::grant_role_command(&config, &account, &role).await {
            Ok(message) => {
                println!("{}", message);
                return Ok(());
            }
            Err(e) => {
                eprintln!("✗ {}", e);
                std::process::exit(1);
            }
        }
    }

    // Reset a password and exit. Beside `--grant-role`, and before the startup
    // validation for the same reason: an engine whose configuration is broken
    // is exactly when someone needs to get back into an account.
    if let Some(account) = matches.get_one::<String>("set-password") {
        let password = match read_new_password(account) {
            Ok(password) => password,
            Err(e) => {
                eprintln!("✗ {}", e);
                std::process::exit(1);
            }
        };

        match aiwebengine::set_password_command(&config, account, &password).await {
            Ok(message) => {
                println!("{}", message);
                return Ok(());
            }
            Err(e) => {
                eprintln!("✗ {}", e);
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
    tracing::info!(
        "Server address: {}",
        config
            .server_address()
            .map_err(|e| aiwebengine::AppError::Config {
                message: e.to_string(),
                source: None,
            })?
    );
    tracing::info!(
        "JavaScript timeout: {}ms",
        config.javascript.execution_timeout_ms
    );
    tracing::info!(
        "Max memory per script: {} bytes",
        config.javascript.max_memory_bytes
    );
    tracing::info!("Storage: PostgreSQL");
    // Neither CORS nor rate limiting was ever reported from what the engine
    // does — both lines printed a configured value that nothing enforced, so
    // an operator reading the startup log was told the opposite of the truth
    // about two protections. Rate limiting is on, with per-key budgets rather
    // than one number; CORS is in SECURITY-todo.md.
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

/// Read the password `--set-password` will store, from standard input.
///
/// Standard input rather than an argument: an argument is visible in `ps` and
/// in shell history, and this is the one command whose whole job is to write a
/// credential. A pipe (`printf '%s' "$PASSWORD" | aiwebengine --set-password
/// alice`) is the scripted form; typing it in is the interactive one.
///
/// Only the line ending is stripped. A password may legitimately begin or end
/// with a space, and trimming one off would store something other than what the
/// operator meant — which they would discover at the sign-in page, locked out.
///
/// At a terminal the password is echoed, so it is asked for twice: nothing here
/// can suppress the echo without a platform-specific dependency, and a typo in
/// a password being reset because nobody remembers the old one is the failure
/// this command exists to end.
fn read_new_password(account: &str) -> AppResult<String> {
    use std::io::{BufRead, IsTerminal, Write};

    let interactive = std::io::stdin().is_terminal();
    let mut stdin = std::io::stdin().lock();

    let mut read_line = |prompt: &str| -> AppResult<String> {
        if interactive {
            eprint!("{}", prompt);
            let _ = std::io::stderr().flush();
        }
        let mut line = String::new();
        let read = stdin.read_line(&mut line).map_err(|e| {
            aiwebengine::AppError::config(format!("Could not read a password: {}", e))
        })?;
        if read == 0 {
            return Err(aiwebengine::AppError::config(
                "No password on standard input. Pipe one in, or run this at a terminal."
                    .to_string(),
            ));
        }
        Ok(line
            .trim_end_matches('\n')
            .trim_end_matches('\r')
            .to_string())
    };

    if interactive {
        eprintln!(
            "Setting a new password for {}. It will be visible as you type.",
            account
        );
    }

    let password = read_line("New password: ")?;
    if password.is_empty() {
        return Err(aiwebengine::AppError::config(
            "An empty password is not one anybody can sign in with.".to_string(),
        ));
    }

    if interactive {
        let again = read_line("Again: ")?;
        if again != password {
            return Err(aiwebengine::AppError::config(
                "Those two passwords are not the same. Nothing was changed.".to_string(),
            ));
        }
    }

    Ok(password)
}
