use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Redirect, Response, Sse, sse::Event};
use axum::{Router, routing::any};
use axum_server::Server;
use futures::StreamExt as FuturesStreamExt;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::Arc;
use tokio_stream::wrappers::BroadcastStream;
use tracing::{debug, error, info, warn};

pub mod asset_registry;
pub mod config;
pub mod conversion;
pub mod database;
pub mod db_schema_utils;
pub mod dispatcher;
pub mod error;
pub mod graphql;
pub mod graphql_schema_gen;
pub mod graphql_ws;
pub mod http_client;
pub mod js_engine;
pub mod mcp;
pub mod middleware;
pub mod notifications;
pub mod parsers;
pub mod repository;
pub mod safe_helpers;
pub mod scheduler;
pub mod script_init;
pub mod secrets;
pub mod security;
pub mod stream_manager;
pub mod stream_registry;
pub mod user_repository;

// Authentication module (Phase 1 - Core Infrastructure)
pub mod auth;

// Documentation module
pub mod docs;

use repository::Repository;
use security::UserContext;

// Re-export the unified error type
pub use error::{AppError, AppResult};

/// Parses a query string into a HashMap of key-value pairs
fn parse_query_string(query: &str) -> HashMap<String, String> {
    // Use url::form_urlencoded to handle percent-encoding and plus->space semantics.
    url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .fold(HashMap::new(), |mut acc, (k, v)| {
            // Insert will overwrite duplicates so last wins (same behavior as previous impl)
            acc.insert(k, v);
            acc
        })
}

/// Parses form data from request body based on content type
use crate::parsers::parse_form_data;

/// Helper: Sanitize connection string for logging (hide password)
fn sanitize_connection_string(conn_str: &str) -> String {
    if let Some(at_pos) = conn_str.find('@') {
        let before_at = &conn_str[..at_pos];
        let after_at = &conn_str[at_pos..];
        if let Some(colon_pos) = before_at.rfind(':') {
            format!("{}:****{}", &before_at[..colon_pos], after_at)
        } else {
            conn_str.to_string()
        }
    } else {
        conn_str.to_string()
    }
}

/// Helper: Create JsAuthContext from optional AuthUser
fn create_js_auth_context(auth_user: Option<&auth::AuthUser>) -> auth::JsAuthContext {
    match auth_user {
        Some(user) => auth::JsAuthContext::authenticated(
            user.user_id.clone(),
            user.email.clone(),
            user.name.clone(),
            user.provider.clone(),
            user.is_admin,
            user.is_editor,
        ),
        None => auth::JsAuthContext::anonymous(),
    }
}

/// OAuth provider registration configuration
struct OAuthProviderConfig {
    client_id: String,
    client_secret: String,
    redirect_uri: String,
    scopes: Vec<String>,
    default_scopes: Vec<&'static str>,
    extra_params: HashMap<String, String>,
}

/// Helper: Register an OAuth2 provider with common configuration pattern
fn register_oauth_provider(
    auth_manager: &mut auth::AuthManager,
    provider_name: &str,
    config: OAuthProviderConfig,
) -> Result<(), auth::AuthError> {
    info!("Registering {} OAuth2 provider", provider_name);
    let oauth_config = auth::OAuth2ProviderConfig {
        client_id: config.client_id,
        client_secret: config.client_secret,
        redirect_uri: config.redirect_uri,
        scopes: if config.scopes.is_empty() {
            config
                .default_scopes
                .iter()
                .map(|s| s.to_string())
                .collect()
        } else {
            config.scopes
        },
        auth_url: None,
        token_url: None,
        userinfo_url: None,
        extra_params: config.extra_params,
    };
    auth_manager.register_provider(provider_name, oauth_config)
}

/// Helper: Initialize secrets manager from environment and config
fn initialize_secrets(config: &config::Config) -> Arc<secrets::SecretsManager> {
    info!("Initializing secrets manager...");
    let secrets_manager = secrets::SecretsManager::new();

    // Load secrets from environment variables (SECRET_* prefix)
    secrets_manager.load_from_env();
    let env_secrets_count = secrets_manager.list_identifiers().len();
    if env_secrets_count > 0 {
        info!(
            "Loaded {} secret(s) from environment variables",
            env_secrets_count
        );
        debug!(
            "Available secrets: {:?}",
            secrets_manager.list_identifiers()
        );
    } else {
        info!("No secrets loaded from environment variables");
    }

    // Load secrets from configuration file if available
    if !config.secrets.values.is_empty() {
        secrets_manager.load_from_map(config.secrets.values.clone());
        let total_secrets = secrets_manager.list_identifiers().len();
        let config_secrets = total_secrets - env_secrets_count;
        if config_secrets > 0 {
            info!(
                "Loaded {} additional secret(s) from configuration file",
                config_secrets
            );
        }
        info!("Total secrets configured: {}", total_secrets);
    } else if env_secrets_count == 0 {
        debug!("No secrets configured from environment or config file");
    }

    let secrets_manager = Arc::new(secrets_manager);

    // Set as global secrets manager for access from js_engine
    if secrets::initialize_global_secrets_manager(secrets_manager.clone()) {
        info!("Global secrets manager initialized successfully");
    } else {
        warn!("Global secrets manager was already initialized");
    }

    secrets_manager
}

/// Helper: Initialize database and repository
async fn initialize_database_and_repository(config: &config::Config) -> AppResult<()> {
    info!("Initializing database connection...");
    info!(
        "Repository config - storage_type: {}",
        config.repository.storage_type
    );

    // Handle memory storage explicitly - no database connection needed
    if config.repository.storage_type == "memory" {
        info!("Initializing in-memory repository (no database connection required)");
        let repo = repository::UnifiedRepository::new_memory();
        if repository::initialize_repository(repo) {
            info!("Global repository initialized with Memory");
        } else {
            warn!("Global repository was already initialized");
        }
        return Ok(());
    }

    // For non-memory storage, we require a connection string
    if let Some(ref conn_str) = config.repository.connection_string {
        let safe_conn_str = sanitize_connection_string(conn_str);
        info!("Repository config - connection_string: {}", safe_conn_str);
    } else {
        return Err(AppError::ConfigValidation {
            field: "repository.connection_string".to_string(),
            reason: "Connection string is required for non-memory storage".to_string(),
        });
    }

    // Attempt to initialize database
    match database::init_database(
        &config.repository,
        config.repository.storage_type == "postgresql",
    )
    .await
    {
        Ok(db) => {
            let db_arc = Arc::new(db);
            if database::initialize_global_database(db_arc.clone()) {
                info!("Global database initialized successfully");
            } else {
                warn!("Global database was already initialized");
            }

            // Generate unique server ID for this instance (once)
            let server_id = notifications::generate_server_id();
            info!("Generated server ID: {}", server_id);

            // Store server ID globally for use by other components
            if !notifications::initialize_server_id(server_id.clone()) {
                warn!("Server ID was already initialized");
            }

            // Initialize UnifiedRepository with Postgres and server_id
            let repo =
                repository::UnifiedRepository::new_postgres(db_arc.pool().clone(), server_id);
            if repository::initialize_repository(repo) {
                info!("Global repository initialized with Postgres");
            } else {
                warn!("Global repository was already initialized");
            }
        }
        Err(e) => {
            // Strict failure: Do not fallback to memory if database fails
            return Err(AppError::Database {
                message: format!(
                    "Database initialization failed: {}. Fatal error for storage_type '{}'.",
                    e, config.repository.storage_type
                ),
                source: None,
            });
        }
    }

    Ok(())
}

/// Helper: Convert ErrorResponse to HTTP response
fn error_to_response(error_response: error::ErrorResponse) -> Response {
    let status =
        StatusCode::from_u16(error_response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    let body = serde_json::to_string(&error_response)
        .unwrap_or_else(|_| r#"{"error":"Serialization failed"}"#.to_string());
    (status, body).into_response()
}

/// Helper: Get client metadata for stream connection from customization function or query params
fn get_stream_client_metadata(
    path: &str,
    query_params: &HashMap<String, String>,
    auth_user: Option<&auth::AuthUser>,
) -> Result<Option<HashMap<String, String>>, String> {
    let stream_info = stream_registry::GLOBAL_STREAM_REGISTRY.get_stream_info(path);

    if let Some((script_uri, Some(func_name))) = stream_info {
        // Execute customization function to get filter criteria
        let auth_context = auth_user.map(|user| create_js_auth_context(Some(user)));

        let filter_criteria = js_engine::execute_stream_customization_function(
            &script_uri,
            &func_name,
            path,
            query_params,
            auth_context,
        )?;

        info!(
            "Customization function '{}' returned filter criteria: {:?}",
            func_name, filter_criteria
        );
        return Ok(if filter_criteria.is_empty() {
            None
        } else {
            Some(filter_criteria)
        });
    }

    // No customization function, use query params as fallback
    Ok(if query_params.is_empty() {
        None
    } else {
        Some(query_params.clone())
    })
}

/// Helper: Build error response for stream errors
fn build_stream_error_response(message: &str) -> Response {
    Response::builder()
        .status(StatusCode::INTERNAL_SERVER_ERROR)
        .header("content-type", "text/plain")
        .body(Body::from(message.to_string()))
        .unwrap_or_else(|err| {
            error!("Failed to build error response: {}", err);
            Response::new(Body::from("Internal Server Error"))
        })
}

/// Handle Server-Sent Events stream requests
async fn handle_stream_request(req: Request<Body>) -> Response {
    let path = req.uri().path().to_string();
    let query_string = req.uri().query().map(|s| s.to_string()).unwrap_or_default();
    let query_params = parse_query_string(&query_string);

    // Extract auth context before consuming the request
    let auth_user = req.extensions().get::<auth::AuthUser>().cloned();

    info!(
        "Handling stream request for path: {} with query params: {:?}",
        path, query_params
    );

    // Get client metadata from customization function or query params
    let client_metadata = match get_stream_client_metadata(&path, &query_params, auth_user.as_ref())
    {
        Ok(metadata) => metadata,
        Err(e) => {
            error!("Customization function failed for stream '{}': {}", path, e);
            return build_stream_error_response(&format!(
                "Stream customization function failed: {}",
                e
            ));
        }
    };

    // Create a connection with the stream manager
    let connection = match stream_manager::StreamConnectionManager::new()
        .create_connection(&path, client_metadata)
        .await
    {
        Ok(conn) => conn,
        Err(e) => {
            error!("Failed to create stream connection for '{}': {}", path, e);
            return build_stream_error_response(&format!(
                "Failed to create stream connection: {}",
                e
            ));
        }
    };

    let connection_id = connection.connection_id.clone();
    info!(
        "Created stream connection {} for path '{}'",
        connection_id, path
    );

    // Convert broadcast receiver to tokio stream
    let receiver_stream = BroadcastStream::new(connection.receiver);

    // Clone connection_id for use in the closure
    let connection_id_for_stream = connection_id.clone();

    // Convert to SSE events, handling both messages and errors
    let path_for_cleanup = path.clone();
    let sse_stream = tokio_stream::StreamExt::map(receiver_stream, move |result| {
        match result {
            Ok(msg) => {
                debug!(
                    "Sending SSE message to connection {}: {}",
                    connection_id_for_stream, msg
                );
                Ok::<Event, std::convert::Infallible>(Event::default().data(msg))
            }
            Err(e) => {
                error!(
                    "Broadcast receiver error for connection {}: {}",
                    connection_id_for_stream, e
                );
                // This indicates the connection has failed, we should clean it up
                if let Err(cleanup_err) = stream_registry::GLOBAL_STREAM_REGISTRY
                    .remove_connection(&path_for_cleanup, &connection_id_for_stream)
                {
                    error!(
                        "Failed to cleanup failed connection {}: {}",
                        connection_id_for_stream, cleanup_err
                    );
                } else {
                    debug!(
                        "Cleaned up failed connection {} from stream {}",
                        connection_id_for_stream, path_for_cleanup
                    );
                }
                Ok::<Event, std::convert::Infallible>(
                    Event::default().data(format!("{{\"error\": \"Stream error: {}\"}}", e)),
                )
            }
        }
    });

    // Create SSE response
    let sse = Sse::new(sse_stream).keep_alive(axum::response::sse::KeepAlive::default());

    // Return the SSE response
    sse.into_response()
}

/// Calculate specificity score for a route pattern
/// Higher score = more specific route
/// Score = (exact segments × 1000) + (param segments × 100) - (wildcard depth × 10)
fn calculate_route_specificity(pattern: &str) -> i32 {
    let parts: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let mut exact_count = 0i32;
    let mut param_count = 0i32;
    let mut wildcard_depth = 0i32;

    for (depth, part) in parts.iter().enumerate() {
        if part.starts_with(':') {
            param_count += 1;
        } else if *part == "*" {
            wildcard_depth = (parts.len() - depth) as i32;
        } else {
            exact_count += 1;
        }
    }

    (exact_count * 1000) + (param_count * 100) - (wildcard_depth * 10)
}

/// Match a route pattern with parameters against a path
/// Returns extracted parameters if the pattern matches
fn match_route_pattern(pattern: &str, path: &str) -> Option<HashMap<String, String>> {
    let pattern_parts: Vec<&str> = pattern.split('/').filter(|s| !s.is_empty()).collect();
    let path_parts: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    if pattern_parts.len() != path_parts.len() {
        return None;
    }

    let mut params = HashMap::new();

    for (pattern_part, path_part) in pattern_parts.iter().zip(path_parts.iter()) {
        if let Some(param_name) = pattern_part.strip_prefix(':') {
            // This is a parameter
            params.insert(param_name.to_string(), path_part.to_string());
        } else if *pattern_part != *path_part {
            // Literal parts must match exactly
            return None;
        }
    }

    Some(params)
}

/// Dynamically find a route handler by checking cached registrations from init()
/// Routes are matched by specificity: exact > params > wildcards
fn find_route_handler(
    path: &str,
    method: &str,
) -> Option<(String, String, HashMap<String, String>)> {
    // Fetch all script metadata which includes cached registrations from init()
    let all_metadata = match repository::get_all_script_metadata() {
        Ok(metadata) => metadata,
        Err(e) => {
            error!("Failed to fetch script metadata: {}", e);
            return None;
        }
    };

    // Collect all matching routes with their specificity scores
    let mut candidates: Vec<(i32, String, String, HashMap<String, String>)> = Vec::new();

    for metadata in all_metadata {
        // Use cached registrations from init() function
        if metadata.initialized && !metadata.registrations.is_empty() {
            // Check for exact match
            if let Some(route_meta) = metadata
                .registrations
                .get(&(path.to_string(), method.to_string()))
            {
                let specificity = calculate_route_specificity(path);
                candidates.push((
                    specificity,
                    metadata.uri.clone(),
                    route_meta.handler_name.clone(),
                    HashMap::new(),
                ));
            }

            // Check for parameterized matches (:variable)
            for ((pattern, reg_method), route_meta) in &metadata.registrations {
                if reg_method == method
                    && let Some(params) = match_route_pattern(pattern, path)
                {
                    let specificity = calculate_route_specificity(pattern);
                    candidates.push((
                        specificity,
                        metadata.uri.clone(),
                        route_meta.handler_name.clone(),
                        params,
                    ));
                }
            }

            // Check for wildcard matches
            for ((pattern, reg_method), route_meta) in &metadata.registrations {
                if reg_method == method && pattern.ends_with("/*") {
                    let prefix = &pattern[..pattern.len() - 1]; // Remove the *
                    if path.starts_with(prefix) {
                        let specificity = calculate_route_specificity(pattern);
                        candidates.push((
                            specificity,
                            metadata.uri.clone(),
                            route_meta.handler_name.clone(),
                            HashMap::new(),
                        ));
                    }
                }
            }
        }
    }

    // Sort by specificity (highest first) and return the most specific match
    if !candidates.is_empty() {
        candidates.sort_by(|a, b| b.0.cmp(&a.0)); // Descending order
        let (_, uri, handler, params) = candidates.into_iter().next().unwrap();
        return Some((uri, handler, params));
    }

    None
}

/// Check if any script registers a route for the given path (used for 405 responses)
fn path_has_any_route(path: &str) -> bool {
    let all_metadata = match repository::get_all_script_metadata() {
        Ok(metadata) => metadata,
        Err(_) => return false,
    };

    for metadata in all_metadata {
        if metadata.initialized && !metadata.registrations.is_empty() {
            // Check for exact match
            if metadata.registrations.keys().any(|(p, _)| p == path) {
                return true;
            }

            // Check for parameterized matches
            for (pattern, _) in metadata.registrations.keys() {
                if match_route_pattern(pattern, path).is_some() {
                    return true;
                }
            }

            // Check for wildcard matches
            for (pattern, _) in metadata.registrations.keys() {
                if pattern.ends_with("/*") {
                    let prefix = &pattern[..pattern.len() - 1]; // Remove the *
                    if path.starts_with(prefix) {
                        return true;
                    }
                }
            }
        }
    }

    false
}

/// Initialize authentication manager with all dependencies
async fn initialize_auth_manager(
    auth_config: auth::AuthConfig,
    server_config: &config::ServerConfig,
    security_config: &config::SecurityConfig,
    pool: sqlx::PgPool,
) -> Result<Arc<auth::AuthManager>, auth::AuthError> {
    use auth::{
        AuthManager, AuthManagerConfig, AuthSecurityContext, AuthSessionManager, CookieSameSite,
    };
    use security::{
        CsrfProtection, DataEncryption, RateLimiter, SecureSessionManager, SecurityAuditor,
    };

    // Create security infrastructure
    let auditor = Arc::new(SecurityAuditor::new(Some(pool.clone())));

    // Create rate limiter
    let rate_limiter = Arc::new(RateLimiter::new(pool.clone()));

    // Load CSRF key from configuration (base64 encoded 32 bytes)
    let csrf_key = match &security_config.csrf_key {
        Some(s) if !s.is_empty() => {
            let decoded = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, s)
                .map_err(|e| auth::AuthError::InvalidConfig {
                    key: "security.csrf_key".to_string(),
                    reason: format!("base64 decode failed: {}", e),
                })?;
            if decoded.len() != 32 {
                return Err(auth::AuthError::InvalidConfig {
                    key: "security.csrf_key".to_string(),
                    reason: "expected 32 bytes after base64 decoding".to_string(),
                });
            }
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&decoded);
            arr
        }
        _ => {
            warn!(
                "security.csrf_key not configured. Generating random key. CSRF tokens will be invalid after restart."
            );
            rand::random::<[u8; 32]>()
        }
    };

    let csrf = Arc::new(CsrfProtection::new(csrf_key, 3600)); // 1 hour lifetime

    // Load session encryption key from configuration (base64 encoded 32 bytes)
    let encryption_key = match &security_config.session_encryption_key {
        Some(s) if !s.is_empty() => {
            let decoded = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, s)
                .map_err(|e| auth::AuthError::InvalidConfig {
                    key: "security.session_encryption_key".to_string(),
                    reason: format!("base64 decode failed: {}", e),
                })?;
            if decoded.len() != 32 {
                return Err(auth::AuthError::InvalidConfig {
                    key: "security.session_encryption_key".to_string(),
                    reason: "expected 32 bytes after base64 decoding".to_string(),
                });
            }
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&decoded);
            arr
        }
        _ => {
            warn!(
                "security.session_encryption_key not configured. Generating random key. Sessions will be invalid after restart."
            );
            rand::random::<[u8; 32]>()
        }
    };

    let encryption = Arc::new(DataEncryption::new(&encryption_key));

    // Create secure session manager
    let session_manager = Arc::new(SecureSessionManager::new(
        pool.clone(),
        &encryption_key,
        3600, // 1 hour session lifetime (seconds)
        10,   // max 10 sessions per user
        Arc::clone(&auditor),
    )?);

    // Create auth-specific security context
    let security_context = Arc::new(AuthSecurityContext::new(
        Arc::clone(&auditor),
        rate_limiter,
        csrf,
        encryption,
    ));

    // Create auth session manager
    let auth_session_manager = Arc::new(AuthSessionManager::new(Arc::clone(&session_manager)));

    // Get base URL from server config
    let base_url = server_config.get_base_url();

    // Create AuthManager config from auth config
    let manager_config = AuthManagerConfig {
        base_url: base_url.clone(),
        session_cookie_name: auth_config.cookie.name.clone(),
        cookie_domain: auth_config.cookie.domain.clone(),
        cookie_secure: auth_config.cookie.secure,
        cookie_http_only: auth_config.cookie.http_only,
        cookie_same_site: match auth_config.cookie.same_site {
            auth::SameSitePolicy::Strict => CookieSameSite::Strict,
            auth::SameSitePolicy::Lax => CookieSameSite::Lax,
            auth::SameSitePolicy::None => CookieSameSite::None,
        },
        session_timeout: auth_config.session_timeout,
    };

    // Create auth manager
    let mut auth_manager = AuthManager::new(
        manager_config,
        auth_session_manager,
        security_context,
        security_config.api_key.clone(),
    );

    // Register OAuth2 providers if configured
    if let Some(google_config) = auth_config.providers.google {
        register_oauth_provider(
            &mut auth_manager,
            "google",
            OAuthProviderConfig {
                client_id: google_config.client_id,
                client_secret: google_config.client_secret,
                redirect_uri: google_config.redirect_uri,
                scopes: google_config.scopes,
                default_scopes: vec!["openid", "profile", "email"],
                extra_params: HashMap::new(),
            },
        )?;
    }

    if let Some(microsoft_config) = auth_config.providers.microsoft {
        let mut extra_params = HashMap::new();
        if let Some(tenant_id) = microsoft_config.tenant_id {
            extra_params.insert("tenant_id".to_string(), tenant_id);
        }
        register_oauth_provider(
            &mut auth_manager,
            "microsoft",
            OAuthProviderConfig {
                client_id: microsoft_config.client_id,
                client_secret: microsoft_config.client_secret,
                redirect_uri: microsoft_config.redirect_uri,
                scopes: microsoft_config.scopes,
                default_scopes: vec!["openid", "profile", "email"],
                extra_params,
            },
        )?;
    }

    if let Some(apple_config) = auth_config.providers.apple {
        let mut extra_params = HashMap::new();
        if let Some(team_id) = apple_config.team_id {
            extra_params.insert("team_id".to_string(), team_id);
        }
        if let Some(key_id) = apple_config.key_id {
            extra_params.insert("key_id".to_string(), key_id);
        }
        if let Some(private_key) = apple_config.private_key {
            extra_params.insert("private_key".to_string(), private_key);
        }
        register_oauth_provider(
            &mut auth_manager,
            "apple",
            OAuthProviderConfig {
                client_id: apple_config.client_id,
                client_secret: apple_config.client_secret,
                redirect_uri: apple_config.redirect_uri,
                scopes: apple_config.scopes,
                default_scopes: vec!["name", "email"],
                extra_params,
            },
        )?;
    }

    Ok(Arc::new(auth_manager))
}

/// Initialize all core components (secrets, database, scripts, assets)
async fn initialize_components(config: &config::Config) -> AppResult<()> {
    // Initialize secrets manager
    let _secrets_manager = initialize_secrets(config);

    // Initialize database connection and repository
    initialize_database_and_repository(config).await?;

    // Start PostgreSQL notification listener if using PostgreSQL storage
    if config.repository.storage_type == "postgresql"
        && let Some(db) = database::get_global_database()
    {
        info!("Starting PostgreSQL notification listener for script synchronization...");

        // Get the server ID that was generated during repository initialization
        let server_id = notifications::get_server_id()
            .expect("Server ID should be initialized before notification listener");

        let listener = Arc::new(notifications::NotificationListener::new(
            server_id.clone(),
            db.pool().clone(),
        ));

        if let Err(e) = listener.start().await {
            error!("Failed to start notification listener: {}", e);
            // Don't fail startup, just log the error
        } else {
            info!(
                "PostgreSQL notification listener started with server_id: {}",
                server_id
            );
            // Store listener globally for cleanup
            notifications::initialize_global_listener(listener);
        }
    }

    // Ensure scheduler state exists before scripts start registering jobs
    scheduler::initialize_global_scheduler();

    // Bootstrap hardcoded scripts into database if configured
    info!("Bootstrapping hardcoded scripts into database...");
    if let Err(e) = repository::bootstrap_scripts() {
        warn!(
            "Failed to bootstrap scripts: {}. Continuing with static scripts.",
            e
        );
    }

    // Bootstrap hardcoded assets into database if configured
    info!("Bootstrapping hardcoded assets into database...");
    if let Err(e) = repository::bootstrap_assets_async().await {
        warn!(
            "Failed to bootstrap assets: {}. Continuing with static assets.",
            e
        );
    }

    // Execute all scripts at startup to populate GraphQL registry
    execute_startup_scripts().await?;

    // Initialize all scripts by calling their init() functions if they exist
    if config.javascript.enable_init_functions {
        initialize_script_functions(config).await?;
    } else {
        info!("Script init() functions are disabled in configuration");
    }

    // Initialize GraphQL schema (will be rebuilt dynamically as needed)
    if let Err(e) = graphql::rebuild_schema() {
        error!("Failed to initialize GraphQL schema: {:?}", e);
        // Don't fail startup, just log the error
    }

    Ok(())
}

/// Execute all scripts at startup to populate GraphQL registry
async fn execute_startup_scripts() -> AppResult<()> {
    info!("Executing all scripts at startup to populate GraphQL registry...");
    let scripts = repository::get_repository()
        .list_scripts()
        .await
        .unwrap_or_default();
    info!("Found {} scripts to execute", scripts.len());

    for (uri, content) in scripts.iter() {
        info!("Executing script: {}", uri);
        // Use secure execution with admin user context for startup script execution
        let result = js_engine::execute_script_secure(
            uri,
            content,
            UserContext::admin("system".to_string()),
        );

        if !result.success {
            error!("Failed to execute script {}: {:?}", uri, result.error);
            // Log FATAL error to database
            let error_msg = result
                .error
                .as_ref()
                .map(|e| format!("Script execution failed: {}", e))
                .unwrap_or_else(|| "Script execution failed".to_string());
            if let Err(e) = repository::get_repository()
                .insert_log(uri, &error_msg, "FATAL")
                .await
            {
                warn!("Failed to log error to database: {}", e);
            }
        } else {
            info!("Successfully executed script: {}", uri);
        }
    }

    Ok(())
}

/// Initialize script functions by calling their init() functions
async fn initialize_script_functions(config: &config::Config) -> AppResult<()> {
    info!("Initializing all scripts...");
    let init_timeout = config
        .javascript
        .init_timeout_ms
        .unwrap_or(config.javascript.execution_timeout_ms);

    let initializer = script_init::ScriptInitializer::new(init_timeout);
    info!("Calling initialize_all_scripts...");

    match initializer.initialize_all_scripts().await {
        Ok(results) => {
            info!("initialize_all_scripts returned");
            let successful = results.iter().filter(|r| r.success).count();
            let failed = results
                .iter()
                .filter(|r| !r.success && r.error.is_some())
                .count();
            let skipped = results
                .iter()
                .filter(|r| r.success && r.duration_ms == 0)
                .count();

            info!(
                "Script initialization complete: {} successful, {} failed, {} skipped (no init function)",
                successful, failed, skipped
            );

            // Log any failures for visibility
            for result in results.iter().filter(|r| !r.success) {
                if let Some(ref error) = result.error {
                    error!(
                        "Script '{}' initialization failed: {}",
                        result.script_uri, error
                    );
                }
            }

            // Log warning but don't fail startup - scripts can be fixed and reloaded
            if failed > 0 {
                warn!(
                    "Server startup: {} script(s) failed initialization but continuing",
                    failed
                );
            }
        }
        Err(e) => {
            error!("Failed to initialize scripts: {}", e);
            warn!("Server continuing despite script initialization error");
        }
    }

    Ok(())
}

/// Starts the web server with custom configuration
pub async fn start_server_with_config(
    config: config::Config,
    shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) -> AppResult<u16> {
    // Initialize all core components
    initialize_components(&config).await?;

    // Fan out shutdown notifications so both the HTTP server and scheduler worker can stop cleanly
    let (server_shutdown_tx, server_shutdown_rx) = tokio::sync::oneshot::channel();
    let (scheduler_shutdown_tx, scheduler_shutdown_rx) = tokio::sync::oneshot::channel();

    scheduler::spawn_worker(scheduler_shutdown_rx);

    tokio::spawn(async move {
        let _ = shutdown_rx.await;
        let _ = scheduler_shutdown_tx.send(());
        let _ = server_shutdown_tx.send(());
    });

    // Clone the timeout value to avoid borrow checker issues in async closures
    let script_timeout_ms = config.javascript.execution_timeout_ms;

    // Get database pool if available
    let pool = database::get_global_database().map(|db| db.pool().clone());

    // Initialize authentication if configured and enabled
    let auth_manager = initialize_auth_if_enabled(&config, pool.clone()).await;

    // Determine if auth is enabled
    let auth_enabled = auth_manager.is_some();

    // Build the router with all routes and middleware
    let app = setup_routes(
        &config,
        script_timeout_ms,
        auth_enabled,
        auth_manager.as_ref(),
        pool,
    )
    .await;

    let (actual_port, actual_addr) = find_available_port(&config)?;

    // Record startup in logs so tests can observe server start
    repository::insert_log_message("server", "server started", "INFO");
    debug!(
        "Server configuration - host: {}, requested port: {}, actual port: {}",
        config.server.host, config.server.port, actual_port
    );

    start_server_instance(app, actual_addr, server_shutdown_rx);

    Ok(actual_port)
}

/// Health check endpoint - returns basic instance status
async fn health_handler() -> impl IntoResponse {
    let server_id = notifications::get_server_id().unwrap_or_else(|| "unknown".to_string());

    axum::response::Json(serde_json::json!({
        "status": "healthy",
        "instance_id": server_id,
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "checks": {}
    }))
}

/// Cluster health endpoint - returns detailed cluster status
async fn health_cluster_handler() -> impl IntoResponse {
    let server_id = notifications::get_server_id().unwrap_or_else(|| "unknown".to_string());

    // Get database pool stats if available
    let pool_stats = if let Some(db) = database::get_global_database() {
        let pool = db.pool();
        let size = pool.size() as usize;
        let idle = pool.num_idle();
        serde_json::json!({
            "available": true,
            "active_connections": size.saturating_sub(idle),
            "idle_connections": idle,
            "max_connections": pool.options().get_max_connections(),
        })
    } else {
        serde_json::json!({
            "available": false,
            "message": "Database not initialized (memory mode)"
        })
    };

    // Get notification listener status
    let listener_status = if let Some(_listener) = notifications::get_global_listener() {
        serde_json::json!({
            "active": true,
            "server_id": server_id.clone(),
        })
    } else {
        serde_json::json!({
            "active": false,
            "message": "Notification listener not initialized"
        })
    };

    // Get scheduler job counts per script
    let scheduler = scheduler::get_scheduler();
    let job_counts = scheduler.get_job_counts();
    let total_jobs: usize = job_counts.values().sum();

    axum::response::Json(serde_json::json!({
        "status": "healthy",
        "instance_id": server_id,
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "database": pool_stats,
        "notification_listener": listener_status,
        "scheduler": {
            "total_jobs": total_jobs,
            "jobs_by_script": job_counts,
        }
    }))
}

/// Initialize authentication manager if configured and enabled
async fn initialize_auth_if_enabled(
    config: &config::Config,
    pool: Option<sqlx::PgPool>,
) -> Option<Arc<auth::AuthManager>> {
    if let Some(auth_config) = config.auth.clone()
        && auth_config.enabled
    {
        info!("Authentication is enabled, initializing AuthManager...");

        let pool = match pool {
            Some(p) => p,
            None => {
                error!(
                    "Authentication enabled but database not initialized. Disabling authentication."
                );
                return None;
            }
        };

        debug!(
            "Auth config: enabled={}, providers={:?}",
            auth_config.enabled,
            auth_config.providers.enabled_providers()
        );

        // Configure bootstrap admins for automatic admin role assignment
        if !auth_config.bootstrap_admins.is_empty() {
            info!(
                "Configuring {} bootstrap admin(s): {:?}",
                auth_config.bootstrap_admins.len(),
                auth_config.bootstrap_admins
            );
            user_repository::set_bootstrap_admins(auth_config.bootstrap_admins.clone());
        }

        match initialize_auth_manager(auth_config, &config.server, &config.security, pool).await {
            Ok(manager) => {
                info!("AuthManager initialized successfully");
                Some(manager)
            }
            Err(e) => {
                error!(
                    "Failed to initialize AuthManager: {}. Authentication will be disabled.",
                    e
                );
                None
            }
        }
    } else {
        info!(
            "Authentication disabled: config.auth.is_some()={}, config.auth.enabled={}",
            config.auth.is_some(),
            config.auth.as_ref().map(|c| c.enabled).unwrap_or(false)
        );
        None
    }
}

/// Setup all routes and middleware for the application
async fn setup_routes(
    config: &config::Config,
    script_timeout_ms: u64,
    auth_enabled: bool,
    auth_manager: Option<&Arc<auth::AuthManager>>,
    pool: Option<sqlx::PgPool>,
) -> Router {
    // GraphQL GET handler - serves GraphiQL (requires authentication when auth is enabled)
    let graphql_get_handler = move |req: axum::http::Request<axum::body::Body>| {
        async move {
            // Check for authentication when auth is enabled
            if auth_enabled && req.extensions().get::<auth::AuthUser>().is_none() {
                return safe_helpers::json_response(
                    StatusCode::UNAUTHORIZED,
                    &serde_json::json!({"error": "Authentication required"}),
                );
            }

            let graphiql_html = async_graphql::http::GraphiQLSource::build()
                .endpoint("/graphql")
                .subscription_endpoint("/graphql/ws")
                .title("aiwebengine GraphQL Editor")
                .finish();

            let mut full_html = graphiql_html;
            full_html.push_str(graphiql_navigation_script());

            axum::response::Html(full_html).into_response()
        }
    };

    // Swagger UI GET handler - serves Swagger UI for OpenAPI spec
    let swagger_ui_handler = move |req: axum::http::Request<axum::body::Body>| {
        async move {
            // Check for authentication when auth is enabled
            if auth_enabled && req.extensions().get::<auth::AuthUser>().is_none() {
                return safe_helpers::json_response(
                    StatusCode::UNAUTHORIZED,
                    &serde_json::json!({"error": "Authentication required"}),
                );
            }

            axum::response::Html(swagger_ui_base_html()).into_response()
        }
    };

    // GraphQL handler - executes queries (supports GET and POST)
    let graphql_post_handler = |req: axum::http::Request<axum::body::Body>| async move {
        // Extract authentication context before consuming the request
        let auth_user = req.extensions().get::<auth::AuthUser>().cloned();

        let (parts, body) = req.into_parts();
        let method = parts.method.clone();

        let body_bytes = match axum::body::to_bytes(body, usize::MAX).await {
            Ok(bytes) => bytes,
            Err(_) => {
                return axum::response::Json(
                    serde_json::json!({"error": "Failed to read request body"}),
                );
            }
        };

        let request: async_graphql::Request = if method == axum::http::Method::GET {
            // Parse from query params
            let query_string = parts.uri.query().unwrap_or("");
            let query_params = url::form_urlencoded::parse(query_string.as_bytes());

            let mut query = None;
            let mut variables = None;
            let mut operation_name = None;

            for (key, value) in query_params {
                match key.as_ref() {
                    "query" => query = Some(value.into_owned()),
                    "variables" => {
                        if !value.is_empty()
                            && let Ok(vars) = serde_json::from_str(&value)
                        {
                            variables = Some(vars);
                        }
                    }
                    "operationName" => operation_name = Some(value.into_owned()),
                    _ => {}
                }
            }

            if query.is_none() {
                return axum::response::Json(
                    serde_json::json!({"error": "Missing query parameter"}),
                );
            }

            let mut req = async_graphql::Request::new(query.unwrap());
            if let Some(vars) = variables {
                req = req.variables(vars);
            }
            if let Some(op) = operation_name {
                req = req.operation_name(op);
            }
            req
        } else {
            match serde_json::from_slice(&body_bytes) {
                Ok(req) => req,
                Err(e) => {
                    return axum::response::Json(
                        serde_json::json!({"error": format!("Invalid JSON: {}", e)}),
                    );
                }
            }
        };

        // Get the current schema (rebuilds if necessary)
        let schema = match graphql::get_schema() {
            Ok(schema) => schema,
            Err(e) => {
                return axum::response::Json(
                    serde_json::json!({"error": format!("Schema error: {:?}", e)}),
                );
            }
        };

        // Create authentication context for GraphQL execution
        let js_auth_context = create_js_auth_context(auth_user.as_ref());

        let response = schema.execute(request.data(js_auth_context)).await;
        axum::response::Json(serde_json::to_value(response).unwrap_or(serde_json::Value::Null))
    };

    // GraphQL WebSocket handler - handles subscriptions over WebSocket using graphql-transport-ws protocol
    let graphql_ws_handler =
        |ws: axum::extract::ws::WebSocketUpgrade, req: axum::http::Request<axum::body::Body>| async move {
            // Extract authentication context before upgrade
            let auth_user = req.extensions().get::<auth::AuthUser>().cloned();

            ws.on_upgrade(move |socket| graphql_ws::handle_websocket_connection(socket, auth_user))
        };

    // GraphQL SSE handler - handles subscriptions over Server-Sent Events using execute_stream
    let graphql_sse_handler = |req: axum::http::Request<axum::body::Body>| async move {
        // Extract authentication context before consuming the request
        let auth_user = req.extensions().get::<auth::AuthUser>().cloned();

        let (parts, body) = req.into_parts();
        info!("GraphQL SSE request for URI: {}", parts.uri);

        let body_bytes = match axum::body::to_bytes(body, usize::MAX).await {
            Ok(bytes) => bytes,
            Err(e) => {
                error!("GraphQL SSE: Failed to read request body: {}", e);
                return axum::response::Response::builder()
                    .status(400)
                    .header("content-type", "text/plain")
                    .body(axum::body::Body::from("Failed to read request body"))
                    .unwrap_or_else(|err| {
                        error!("Failed to build error response: {}", err);
                        axum::response::Response::new(axum::body::Body::from("Bad Request"))
                    });
            }
        };

        let method = parts.method.clone();
        let request: async_graphql::Request = if method == axum::http::Method::GET {
            // Parse from query params
            let query_string = parts.uri.query().unwrap_or("");
            let query_params = url::form_urlencoded::parse(query_string.as_bytes());

            let mut query = None;
            let mut variables = None;
            let mut operation_name = None;

            for (key, value) in query_params {
                match key.as_ref() {
                    "query" => query = Some(value.into_owned()),
                    "variables" => {
                        if !value.is_empty()
                            && let Ok(vars) = serde_json::from_str(&value)
                        {
                            variables = Some(vars);
                        }
                    }
                    "operationName" => operation_name = Some(value.into_owned()),
                    _ => {}
                }
            }

            if query.is_none() {
                error!("GraphQL SSE: Missing query parameter");
                return axum::response::Response::builder()
                    .status(400)
                    .header("content-type", "text/plain")
                    .body(axum::body::Body::from("Missing query parameter"))
                    .unwrap_or_else(|err| {
                        error!("Failed to build error response: {}", err);
                        axum::response::Response::new(axum::body::Body::from("Bad Request"))
                    });
            }

            let mut req = async_graphql::Request::new(query.unwrap());
            if let Some(vars) = variables {
                req = req.variables(vars);
            }
            if let Some(op) = operation_name {
                req = req.operation_name(op);
            }
            req
        } else {
            match serde_json::from_slice(&body_bytes) {
                Ok(req) => req,
                Err(e) => {
                    error!("GraphQL SSE: Invalid JSON in request body: {}", e);
                    return axum::response::Response::builder()
                        .status(400)
                        .header("content-type", "text/plain")
                        .body(axum::body::Body::from(format!("Invalid JSON: {}", e)))
                        .unwrap_or_else(|err| {
                            error!("Failed to build error response: {}", err);
                            axum::response::Response::new(axum::body::Body::from("Bad Request"))
                        });
                }
            }
        };

        // Get the current schema (rebuilds if necessary)
        let schema = match graphql::get_schema() {
            Ok(schema) => schema,
            Err(e) => {
                error!("GraphQL SSE: Failed to get schema: {:?}", e);
                return axum::response::Response::builder()
                    .status(500)
                    .header("content-type", "text/plain")
                    .body(axum::body::Body::from(format!("Schema error: {:?}", e)))
                    .unwrap_or_else(|err| {
                        error!("Failed to build error response: {}", err);
                        axum::response::Response::new(axum::body::Body::from(
                            "Internal Server Error",
                        ))
                    });
            }
        };

        // Create authentication context for GraphQL execution
        let js_auth_context = create_js_auth_context(auth_user.as_ref());

        // Check if this is a subscription operation
        let is_subscription = request.query.trim_start().starts_with("subscription");

        if is_subscription {
            // Execute subscription via async-graphql streaming and forward events
            let (tx, rx) = tokio::sync::mpsc::channel(100);

            tokio::spawn(async move {
                let stream = schema.execute_stream(request.data(js_auth_context));

                let mut stream = std::pin::pin!(stream);
                while let Some(response) = FuturesStreamExt::next(&mut stream).await {
                    let json_data =
                        serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string());
                    let event =
                        Ok::<Event, std::convert::Infallible>(Event::default().data(json_data));

                    if tx.send(event).await.is_err() {
                        break;
                    }
                }
            });

            let receiver_stream = tokio_stream::wrappers::ReceiverStream::new(rx);
            Sse::new(receiver_stream)
                .keep_alive(axum::response::sse::KeepAlive::default())
                .into_response()
        } else {
            // Handle regular queries/mutations as single response
            let response = schema.execute(request.data(js_auth_context)).await;
            let json_data = serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string());

            // Return SSE formatted response for consistency
            let sse_data = format!("data: {}\n\n", json_data);
            axum::response::Response::builder()
                .header("content-type", "text/event-stream")
                .header("cache-control", "no-cache")
                .header("connection", "keep-alive")
                .header("access-control-allow-origin", "*")
                .header("access-control-allow-headers", "content-type")
                .body(axum::body::Body::from(sse_data))
                .unwrap_or_else(|err| {
                    error!("Failed to build SSE response: {}", err);
                    axum::response::Response::new(axum::body::Body::from("Internal Server Error"))
                })
        }
    };

    // MCP JSON-RPC handler - supports tools/list and tools/call methods
    let mcp_handler = |req: axum::http::Request<axum::body::Body>| async move {
        let body_bytes = match axum::body::to_bytes(req.into_body(), usize::MAX).await {
            Ok(bytes) => bytes,
            Err(e) => {
                error!("MCP: Failed to read request body: {}", e);
                return axum::response::Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "error": {
                        "code": -32700,
                        "message": "Parse error: Failed to read request body"
                    },
                    "id": null
                }));
            }
        };

        #[derive(Deserialize)]
        struct JsonRpcRequest {
            jsonrpc: String,
            id: Option<serde_json::Value>,
            method: String,
            params: Option<serde_json::Value>,
        }

        let rpc_request: JsonRpcRequest = match serde_json::from_slice(&body_bytes) {
            Ok(req) => req,
            Err(e) => {
                error!("MCP: Invalid JSON-RPC request: {}", e);
                return axum::response::Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "error": {
                        "code": -32700,
                        "message": format!("Parse error: {}", e)
                    },
                    "id": null
                }));
            }
        };

        // Validate JSON-RPC version
        if rpc_request.jsonrpc != "2.0" {
            return axum::response::Json(serde_json::json!({
                "jsonrpc": "2.0",
                "error": {
                    "code": -32600,
                    "message": "Invalid Request: jsonrpc must be 2.0"
                },
                "id": rpc_request.id
            }));
        }

        match rpc_request.method.as_str() {
            "initialize" => {
                // MCP initialization - negotiate protocol version and capabilities
                info!("MCP: Initialize request received");

                // Extract protocol version from params
                let params = rpc_request.params.unwrap_or(serde_json::json!({}));
                let _client_version = params
                    .get("protocolVersion")
                    .and_then(|v| v.as_str())
                    .unwrap_or("2024-11-05");

                // We support 2024-11-05 as our primary version
                let supported_version = "2024-11-05";

                axum::response::Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": rpc_request.id,
                    "result": {
                        "protocolVersion": supported_version,
                        "capabilities": {
                            "tools": {
                                "listChanged": true
                            },
                            "prompts": {
                                "listChanged": true
                            },
                            "completions": {}
                        },
                        "serverInfo": {
                            "name": "aiwebengine",
                            "version": env!("CARGO_PKG_VERSION")
                        }
                    }
                }))
            }
            "notifications/initialized" => {
                // Client signals it's ready after initialization
                info!("MCP: Client initialized notification received");

                // This is a notification, no response needed
                // But we return empty success for compatibility
                axum::response::Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": rpc_request.id
                }))
            }
            "tools/list" => {
                let tools = mcp::list_tools();

                let tools_list: Vec<serde_json::Value> = tools
                    .iter()
                    .map(|tool| {
                        serde_json::json!({
                            "name": tool.name,
                            "description": tool.description,
                            "inputSchema": tool.input_schema
                        })
                    })
                    .collect();

                axum::response::Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": rpc_request.id,
                    "result": {
                        "tools": tools_list
                    }
                }))
            }
            "tools/call" => {
                #[derive(Deserialize)]
                struct ToolCallParams {
                    name: String,
                    arguments: Option<serde_json::Value>,
                }

                let params: ToolCallParams = match rpc_request.params {
                    Some(p) => match serde_json::from_value(p) {
                        Ok(params) => params,
                        Err(e) => {
                            error!("MCP tools/call: Invalid params: {}", e);
                            return axum::response::Json(serde_json::json!({
                                "jsonrpc": "2.0",
                                "error": {
                                    "code": -32602,
                                    "message": format!("Invalid params: {}", e)
                                },
                                "id": rpc_request.id
                            }));
                        }
                    },
                    None => {
                        return axum::response::Json(serde_json::json!({
                            "jsonrpc": "2.0",
                            "error": {
                                "code": -32602,
                                "message": "Invalid params: missing required params"
                            },
                            "id": rpc_request.id
                        }));
                    }
                };

                let arguments = params.arguments.unwrap_or(serde_json::json!({}));

                match mcp::execute_mcp_tool(&params.name, arguments) {
                    Ok(result) => {
                        debug!("MCP tool '{}' executed successfully", params.name);

                        // Parse the result to determine if it's structured or just text
                        let content = vec![serde_json::json!({
                            "type": "text",
                            "text": serde_json::to_string(&result).unwrap_or_else(|_| "{}".to_string())
                        })];

                        axum::response::Json(serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": rpc_request.id,
                            "result": {
                                "content": content,
                                "isError": false
                            }
                        }))
                    }
                    Err(e) => {
                        error!("MCP tool '{}' execution failed: {}", params.name, e);

                        // Check if it's a "tool not found" error
                        if e.contains("not found") {
                            axum::response::Json(serde_json::json!({
                                "jsonrpc": "2.0",
                                "error": {
                                    "code": -32602,
                                    "message": format!("Unknown tool: {}", params.name)
                                },
                                "id": rpc_request.id
                            }))
                        } else {
                            // Return as tool execution error (not protocol error)
                            let content = vec![serde_json::json!({
                                "type": "text",
                                "text": format!("Tool execution failed: {}", e)
                            })];

                            axum::response::Json(serde_json::json!({
                                "jsonrpc": "2.0",
                                "id": rpc_request.id,
                                "result": {
                                    "content": content,
                                    "isError": true
                                }
                            }))
                        }
                    }
                }
            }
            "prompts/list" => {
                let prompts = mcp::list_prompts();

                let prompts_list: Vec<serde_json::Value> = prompts
                    .iter()
                    .map(|prompt| {
                        serde_json::json!({
                            "name": prompt.name,
                            "description": prompt.description,
                            "arguments": prompt.arguments
                        })
                    })
                    .collect();

                axum::response::Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": rpc_request.id,
                    "result": {
                        "prompts": prompts_list
                    }
                }))
            }
            "prompts/get" => {
                #[derive(Deserialize)]
                struct PromptGetParams {
                    name: String,
                    arguments: Option<serde_json::Value>,
                }

                let params: PromptGetParams = match rpc_request.params {
                    Some(p) => match serde_json::from_value(p) {
                        Ok(params) => params,
                        Err(e) => {
                            error!("MCP prompts/get: Invalid params: {}", e);
                            return axum::response::Json(serde_json::json!({
                                "jsonrpc": "2.0",
                                "error": {
                                    "code": -32602,
                                    "message": format!("Invalid params: {}", e)
                                },
                                "id": rpc_request.id
                            }));
                        }
                    },
                    None => {
                        return axum::response::Json(serde_json::json!({
                            "jsonrpc": "2.0",
                            "error": {
                                "code": -32602,
                                "message": "Invalid params: missing required params"
                            },
                            "id": rpc_request.id
                        }));
                    }
                };

                let arguments = params.arguments.unwrap_or(serde_json::json!({}));

                match mcp::execute_mcp_prompt(&params.name, arguments) {
                    Ok(result) => {
                        // The handler should return an object with a "messages" array
                        axum::response::Json(serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": rpc_request.id,
                            "result": result
                        }))
                    }
                    Err(e) => {
                        error!("MCP prompts/get error: {}", e);
                        axum::response::Json(serde_json::json!({
                            "jsonrpc": "2.0",
                            "error": {
                                "code": -32602,
                                "message": e
                            },
                            "id": rpc_request.id
                        }))
                    }
                }
            }
            "completion/complete" => {
                #[derive(Deserialize)]
                struct CompletionRef {
                    #[serde(rename = "type")]
                    ref_type: String,
                    name: Option<String>,
                }

                #[derive(Deserialize)]
                struct CompletionArgument {
                    name: String,
                    value: String,
                }

                #[derive(Deserialize)]
                struct CompletionContext {
                    arguments: Option<serde_json::Value>,
                }

                #[derive(Deserialize)]
                struct CompletionParams {
                    #[serde(rename = "ref")]
                    reference: CompletionRef,
                    argument: CompletionArgument,
                    context: Option<CompletionContext>,
                }

                let params: CompletionParams = match rpc_request.params {
                    Some(p) => match serde_json::from_value(p) {
                        Ok(params) => params,
                        Err(e) => {
                            error!("MCP completion/complete: Invalid params: {}", e);
                            return axum::response::Json(serde_json::json!({
                                "jsonrpc": "2.0",
                                "error": {
                                    "code": -32602,
                                    "message": format!("Invalid params: {}", e)
                                },
                                "id": rpc_request.id
                            }));
                        }
                    },
                    None => {
                        return axum::response::Json(serde_json::json!({
                            "jsonrpc": "2.0",
                            "error": {
                                "code": -32602,
                                "message": "Invalid params: missing required params"
                            },
                            "id": rpc_request.id
                        }));
                    }
                };

                // Only support ref/prompt type for now
                if params.reference.ref_type != "ref/prompt" {
                    return axum::response::Json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "error": {
                            "code": -32602,
                            "message": format!("Unsupported reference type: {}", params.reference.ref_type)
                        },
                        "id": rpc_request.id
                    }));
                }

                let prompt_name = match params.reference.name {
                    Some(name) => name,
                    None => {
                        return axum::response::Json(serde_json::json!({
                            "jsonrpc": "2.0",
                            "error": {
                                "code": -32602,
                                "message": "Missing prompt name in reference"
                            },
                            "id": rpc_request.id
                        }));
                    }
                };

                let context_arguments = params.context.and_then(|c| c.arguments);

                match mcp::execute_mcp_completion(
                    &prompt_name,
                    &params.argument.name,
                    &params.argument.value,
                    context_arguments,
                ) {
                    Ok(result) => {
                        // The handler should return an object with "values", optional "total", and optional "hasMore"
                        axum::response::Json(serde_json::json!({
                            "jsonrpc": "2.0",
                            "id": rpc_request.id,
                            "result": {
                                "completion": result
                            }
                        }))
                    }
                    Err(e) => {
                        error!("MCP completion/complete error: {}", e);
                        axum::response::Json(serde_json::json!({
                            "jsonrpc": "2.0",
                            "error": {
                                "code": -32602,
                                "message": e
                            },
                            "id": rpc_request.id
                        }))
                    }
                }
            }
            _ => axum::response::Json(serde_json::json!({
                "jsonrpc": "2.0",
                "error": {
                    "code": -32601,
                    "message": format!("Method not found: {}", rpc_request.method)
                },
                "id": rpc_request.id
            })),
        }
    };

    // Build the router
    let mut app = Router::new();

    // Add GraphQL and editor routes with authentication requirement if auth is enabled
    if let Some(auth_mgr) = auth_manager {
        info!("✅ Authentication ENABLED - mounting auth routes and middleware");

        // GraphQL endpoints with require_editor_or_admin_middleware
        let auth_mgr_for_graphql = Arc::clone(auth_mgr);
        let graphql_router = Router::new()
            .route("/engine/graphql", axum::routing::get(graphql_get_handler))
            .route("/engine/swagger", axum::routing::get(swagger_ui_handler))
            .layer(axum::middleware::from_fn_with_state(
                auth_mgr_for_graphql,
                auth::require_editor_or_admin_middleware,
            ));

        app = app.merge(graphql_router);

        // GraphQL API endpoints (queries, mutations, subscriptions) - REQUIRES authentication
        let auth_mgr_for_graphql_api = Arc::clone(auth_mgr);
        let graphql_api_router = Router::new()
            .route(
                "/graphql",
                axum::routing::get(graphql_post_handler).post(graphql_post_handler),
            )
            .route("/graphql/ws", axum::routing::get(graphql_ws_handler))
            .route("/graphql/sse", axum::routing::get(graphql_sse_handler))
            .layer(axum::middleware::from_fn_with_state(
                auth_mgr_for_graphql_api,
                auth::required_auth_middleware,
            ));

        app = app.merge(graphql_api_router);

        // MCP endpoint - REQUIRES Bearer token authentication
        // Supports JSON-RPC 2.0 protocol with tools/list and tools/call methods
        let auth_mgr_for_mcp = Arc::clone(auth_mgr);
        let mcp_router = Router::new()
            .route("/mcp", axum::routing::post(mcp_handler))
            .layer(axum::middleware::from_fn_with_state(
                auth_mgr_for_mcp,
                auth::mcp_auth_middleware,
            ));

        app = app.merge(mcp_router);

        // Mount authentication routes
        let auth_router = auth::create_auth_router(Arc::clone(auth_mgr));
        app = app.nest("/auth", auth_router);

        // Mount OAuth2 metadata and dynamic client registration endpoints
        // These provide RFC 8414 authorization server metadata and RFC 7591 client registration
        let metadata_config = Arc::new(auth::MetadataConfig {
            issuer: config.server.get_base_url(),
            enable_registration: true,
            require_pkce: true,
            resource_indicators_supported: true,
        });

        let registration_manager = Arc::new(auth::ClientRegistrationManager::new(90)); // 90 day secret expiry

        let pool = pool.expect("Database pool required when auth is enabled");

        let oauth2_router = auth::create_oauth2_router(
            metadata_config,
            Some(registration_manager),
            Arc::clone(auth_mgr),
            pool,
        );
        app = app.merge(oauth2_router);
    } else {
        warn!("⚠️  Authentication DISABLED - no auth routes or middleware");

        // GraphQL endpoints without authentication
        app = app
            .route("/engine/graphql", axum::routing::get(graphql_get_handler))
            .route("/engine/swagger", axum::routing::get(swagger_ui_handler))
            .route(
                "/graphql",
                axum::routing::get(graphql_post_handler).post(graphql_post_handler),
            )
            .route("/graphql/ws", axum::routing::get(graphql_ws_handler))
            .route("/graphql/sse", axum::routing::get(graphql_sse_handler));

        // MCP endpoint without authentication (auth is disabled globally)
        app = app.route("/mcp", axum::routing::post(mcp_handler));
    }

    // Add health check endpoints (no authentication required)
    app = app
        .route("/health", axum::routing::get(health_handler))
        .route(
            "/health/cluster",
            axum::routing::get(health_cluster_handler),
        );

    // Add documentation routes
    app = app.route(
        "/engine/docs",
        axum::routing::get(|axum::extract::Path(()): axum::extract::Path<()>| async {
            axum::response::Redirect::permanent("/engine/docs/").into_response()
        }),
    );
    app = app.route(
        "/engine/docs/",
        axum::routing::get(docs::handle_docs_request),
    );
    app = app.route(
        "/engine/docs/{*path}",
        axum::routing::get(docs::handle_docs_request),
    );

    // Add catch-all dynamic routes
    let auth_enabled_for_home = auth_enabled;
    let auth_enabled_for_path = auth_enabled;
    let script_timeout_for_home = script_timeout_ms;
    let script_timeout_for_path = script_timeout_ms;

    app = app
        .route(
            "/",
            any(move |req: Request<Body>| async move {
                handle_dynamic_request(req, script_timeout_for_home, auth_enabled_for_home).await
            }),
        )
        .route(
            "/{*path}",
            any(move |req: Request<Body>| async move {
                handle_dynamic_request(req, script_timeout_for_path, auth_enabled_for_path).await
            }),
        );

    // Add middleware layers (applied in reverse order to how they're added)
    // So request_id runs first, then auth middleware
    if let Some(auth_mgr) = auth_manager {
        let auth_mgr_for_middleware = Arc::clone(auth_mgr);
        info!("✅ Adding optional_auth_middleware layer to all routes");
        app = app.layer(axum::middleware::from_fn_with_state(
            auth_mgr_for_middleware,
            auth::optional_auth_middleware,
        ));
    }

    app = app.layer(axum::middleware::from_fn(middleware::request_id_middleware));

    app
}

/// Handle dynamic requests by routing to registered JavaScript handlers
async fn handle_dynamic_request(
    req: Request<Body>,
    script_timeout_ms: u64,
    _auth_enabled: bool,
) -> impl IntoResponse {
    let path = req.uri().path().to_string();
    let request_method = req.method().to_string();

    // Check for registered asset paths first if it's a GET request
    if let Some(asset_response) = try_serve_asset(&path, &request_method) {
        return asset_response;
    }

    // Check if this is a request to a registered stream path
    if should_route_to_stream(&path, &request_method) {
        return handle_stream_request(req).await;
    }

    // Check if any route exists for this path (including wildcards)
    let path_exists = path_has_any_route(&path);

    let reg = find_route_handler(&path, &request_method);
    let (owner_uri, handler_name, route_params) = match reg {
        Some(t) => t,
        None => {
            // Extract request ID from extensions
            let request_id = req
                .extensions()
                .get::<middleware::RequestId>()
                .map(|rid| rid.0.clone())
                .unwrap_or_else(|| "unknown".to_string());

            if path_exists {
                warn!(
                    "[{}] ⚠️  Method not allowed: {} {} (path exists but method not registered)",
                    request_id, request_method, path
                );
                return error_to_response(error::errors::method_not_allowed(
                    &path,
                    &request_method,
                    &request_id,
                ));
            } else if path == "/" && request_method == "GET" {
                info!(
                    "[{}] 🔄 Redirecting root path to /engine/docs for bootstrapping",
                    request_id
                );
                return Redirect::temporary("/engine/docs").into_response();
            } else {
                warn!(
                    "[{}] ⚠️  Route not found: {} {} (no handler registered for this path)",
                    request_id, request_method, path
                );
                return error_to_response(error::errors::not_found(&path, &request_id));
            }
        }
    };

    let owner_uri_cl = owner_uri.clone();
    let handler_cl = handler_name.clone();
    let path_log = path.to_string();
    let method_log = request_method.clone();
    let query_string = req.uri().query().map(|s| s.to_string()).unwrap_or_default();
    let query_params = parse_query_string(&query_string);

    // Extract request ID from extensions before consuming the request
    let request_id = req
        .extensions()
        .get::<middleware::RequestId>()
        .map(|rid| rid.0.clone())
        .unwrap_or_else(|| "unknown".to_string());

    // Extract authentication context from middleware
    let auth_user = req.extensions().get::<auth::AuthUser>().cloned();

    if let Some(ref user) = auth_user {
        info!(
            "[{}] Authentication context found: user_id={}, provider={}",
            request_id, user.user_id, user.provider
        );
    } else {
        info!("[{}] No authentication context in request", request_id);
    }

    info!(
        "[{}] Executing handler '{}' from script '{}' for {} {} (authenticated: {})",
        request_id,
        handler_name,
        owner_uri,
        request_method,
        path,
        auth_user.is_some()
    );

    // Snapshot headers before consuming the request body
    let mut header_map = HashMap::new();
    for (name, value) in req.headers().iter() {
        if let Ok(value_str) = value.to_str() {
            header_map.insert(name.as_str().to_string(), value_str.to_string());
        }
    }

    // Extract content type before consuming the request
    let content_type = req
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let body = req.into_body();

    // Always read the body as bytes first
    let body_bytes = match to_bytes(body, usize::MAX).await {
        Ok(bytes) => bytes,
        Err(_) => axum::body::Bytes::new(),
    };

    // Make raw body available for all requests that might have a body
    // Note: While RFC 7231 doesn't explicitly forbid request bodies for DELETE,
    // some HTTP clients and proxies may not support it. However, we support it
    // for maximum flexibility in API design.
    let raw_body = if !body_bytes.is_empty() {
        Some(String::from_utf8(body_bytes.to_vec()).unwrap_or_default())
    } else {
        None
    };

    // Parse form data if content type indicates form submission
    let is_form_data = content_type
        .as_ref()
        .map(|ct| {
            ct.contains("application/x-www-form-urlencoded") || ct.contains("multipart/form-data")
        })
        .unwrap_or(false);

    let form_data = if is_form_data {
        // Parse form data from the bytes
        let body = Body::from(body_bytes.clone());
        if let Some(ct) = content_type.as_ref() {
            parse_form_data(Some(ct), body).await.unwrap_or_default()
        } else {
            parse_form_data(None, body).await.unwrap_or_default()
        }
    } else {
        HashMap::new()
    };

    let path_clone = path.clone();
    let headers_for_worker = header_map;
    let worker = move || -> Result<js_engine::JsHttpResponse, String> {
        // Create authentication context for JavaScript
        let auth_context = if let Some(ref auth_user) = auth_user {
            auth::JsAuthContext::authenticated(
                auth_user.user_id.clone(),
                auth_user.email.clone(),
                auth_user.name.clone(),
                auth_user.provider.clone(),
                auth_user.is_admin,
                auth_user.is_editor,
            )
        } else {
            auth::JsAuthContext::anonymous()
        };

        // Create UserContext for secure globals based on authenticated user
        let user_context = if let Some(ref auth_user) = auth_user {
            if auth_user.is_admin {
                security::UserContext::admin(auth_user.user_id.clone())
            } else {
                security::UserContext::authenticated(auth_user.user_id.clone())
            }
        } else {
            security::UserContext::anonymous()
        };

        // Use the secure execution path with authentication context
        let params = js_engine::RequestExecutionParams {
            script_uri: owner_uri_cl.clone(),
            handler_name: handler_cl.clone(),
            path: path_clone.clone(),
            method: request_method.clone(),
            query_params: Some(query_params.clone()),
            form_data: Some(form_data.clone()),
            raw_body: raw_body.clone(),
            headers: headers_for_worker.clone(),
            user_context, // Use the properly constructed user_context
            auth_context: Some(auth_context),
            route_params: Some(route_params.clone()),
        };

        js_engine::execute_script_for_request_secure(params)
    };

    let join = tokio::task::spawn_blocking(worker)
        .await
        .map_err(|e| format!("join error: {}", e));

    let timed =
        match tokio::time::timeout(std::time::Duration::from_millis(script_timeout_ms), async {
            join
        })
        .await
        {
            Ok(r) => r,
            Err(_) => {
                return error_to_response(error::errors::script_timeout(&path, &request_id));
            }
        };

    match timed {
        Ok(Ok(js_response)) => {
            info!(
                "[{}] ✅ Successfully executed handler '{}' - status: {}, body_length: {} bytes, headers: {}",
                request_id,
                handler_name,
                js_response.status,
                js_response.body.len(),
                js_response.headers.len()
            );
            build_http_response_from_js(js_response)
        }
        Ok(Err(e)) => {
            error!(
                "[{}] ❌ Script execution error for {} {}: {} (handler: {}, script: {})",
                request_id, method_log, path_log, e, handler_name, owner_uri
            );
            // Log FATAL error to database
            let error_msg = format!(
                "Script execution failed for handler '{}': {}",
                handler_name, e
            );
            repository::insert_log_message(&owner_uri, &error_msg, "FATAL");

            error_to_response(error::errors::script_execution_failed(
                &path,
                &e,
                &request_id,
            ))
        }
        Err(e) => {
            error!(
                "[{}] ❌ Task/runtime error for {} {}: {} (handler: {}, script: {})",
                request_id, method_log, path_log, e, handler_name, owner_uri
            );
            error_to_response(error::errors::internal_server_error(&path, &e, &request_id))
        }
    }
}

/// Finds an available port starting from the given port.
/// Returns the available port and the socket address.
fn find_available_port(config: &config::Config) -> AppResult<(u16, std::net::SocketAddr)> {
    let base_addr: std::net::SocketAddr = config
        .server_address()
        .map_err(|e| AppError::config(format!("Invalid server address: {}", e)))?;

    // Handle automatic port assignment (port 0)
    if config.server.port == 0 {
        let listener = std::net::TcpListener::bind(base_addr).map_err(|e| {
            AppError::internal(format!("Failed to bind to auto-assigned port: {}", e))
        })?;

        let actual_port = listener
            .local_addr()
            .map_err(|e| AppError::internal(format!("Failed to get local address: {}", e)))?
            .port();

        let actual_addr = format!("{}:{}", config.server.host, actual_port)
            .parse()
            .map_err(|e| AppError::config(format!("Invalid server address: {}", e)))?;

        info!("Auto-assigned port: {}", actual_port);
        return Ok((actual_port, actual_addr));
    }

    // Try to find an available port starting from the configured port
    let mut current_port = config.server.port;
    const MAX_PORT_ATTEMPTS: u16 = 100;

    for _attempt in 0..MAX_PORT_ATTEMPTS {
        let addr = format!("{}:{}", config.server.host, current_port)
            .parse()
            .map_err(|e| AppError::config(format!("Invalid server address: {}", e)))?;

        match std::net::TcpListener::bind(addr) {
            Ok(_) => {
                if current_port != config.server.port {
                    info!(
                        "Requested port {} was in use, using port {} instead",
                        config.server.port, current_port
                    );
                } else {
                    info!("listening on {}", addr);
                }
                return Ok((current_port, addr));
            }
            Err(e) if is_address_in_use(&e) => {
                debug!(
                    "Port {} in use, trying port {}",
                    current_port,
                    current_port + 1
                );
                current_port += 1;
            }
            Err(e) => {
                return Err(AppError::internal(format!(
                    "Failed to bind to address {}: {}",
                    addr, e
                )));
            }
        }
    }

    Err(AppError::internal(format!(
        "Could not find an available port after trying {} ports starting from {}",
        MAX_PORT_ATTEMPTS, config.server.port
    )))
}

/// Checks if the error indicates the address is already in use.
fn is_address_in_use(error: &std::io::Error) -> bool {
    let error_msg = error.to_string().to_lowercase();
    error_msg.contains("address already in use")
        || error_msg.contains("address in use")
        || error_msg.contains("eaddrinuse")
        || error.kind() == std::io::ErrorKind::AddrInUse
}

/// Starts the server with the given app and address, handling shutdown.
fn start_server_instance(
    app: Router,
    addr: std::net::SocketAddr,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) {
    let svc = app.into_make_service();
    let server = Server::bind(addr).serve(svc);

    tokio::spawn(async move {
        tokio::select! {
            res = server => {
                if let Err(e) = res {
                    eprintln!("Server error: {:?}", e);
                }
            },
            _ = &mut shutdown_rx => {
                /* graceful shutdown: stop accepting new connections */
            }
        }
    });
}

pub async fn start_server_without_shutdown() -> AppResult<u16> {
    let mut config = config::Config::from_env();
    config.server.port = 0; // Use port 0 for automatic port assignment
    // Create a channel that will never receive a shutdown signal
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    // Leak the sender so it never gets dropped and the channel never closes
    Box::leak(Box::new(tx));
    start_server_with_config(config, rx).await
}

pub async fn start_server_without_shutdown_with_config(config: config::Config) -> AppResult<u16> {
    let (_tx, rx) = tokio::sync::oneshot::channel::<()>();
    start_server_with_config(config, rx).await
}

// ============================================================================
// Helper Functions for Refactored Route Setup and Request Handling
// ============================================================================

/// Returns the navigation script HTML for GraphiQL
fn graphiql_navigation_script() -> &'static str {
    r#"<script>
document.addEventListener('DOMContentLoaded', function() {
    setTimeout(function() {
        const nav = document.createElement('div');
        nav.id = 'aiwebengine-nav';
        nav.style.cssText = 'position: absolute; top: 10px; right: 10px; z-index: 1000; display: flex; gap: 8px;';
        
        const editorLink = document.createElement('a');
        editorLink.href = '/engine/editor';
        editorLink.textContent = 'Editor';
        editorLink.style.cssText = 'background: #f6f7f9; border: 1px solid #d1d5db; border-radius: 4px; padding: 6px 12px; font-size: 12px; color: #374151; text-decoration: none;';
        
        const docsLink = document.createElement('a');
        docsLink.href = '/engine/docs';
        docsLink.textContent = 'Docs';
        docsLink.style.cssText = 'background: #f6f7f9; border: 1px solid #d1d5db; border-radius: 4px; padding: 6px 12px; font-size: 12px; color: #374151; text-decoration: none;';
        
        const swaggerLink = document.createElement('a');
        swaggerLink.href = '/engine/swagger';
        swaggerLink.textContent = 'API Docs';
        swaggerLink.style.cssText = 'background: #f6f7f9; border: 1px solid #d1d5db; border-radius: 4px; padding: 6px 12px; font-size: 12px; color: #374151; text-decoration: none;';
        
        nav.appendChild(editorLink);
        nav.appendChild(docsLink);
        nav.appendChild(swaggerLink);
        document.body.appendChild(nav);
        
        const style = document.createElement('style');
        style.textContent = '#aiwebengine-nav a:hover { background: #e5e7eb !important; border-color: #9ca3af !important; }';
        document.head.appendChild(style);
    }, 2000);
});
</script>"#
}

/// Returns the base Swagger UI HTML (navigation added separately)
fn swagger_ui_base_html() -> &'static str {
    r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>aiwebengine API Documentation</title>
    <link rel="stylesheet" href="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui.css" />
    <style>
        body { margin: 0; padding: 0; }
        #swagger-ui { max-width: 1460px; margin: 0 auto; }
    </style>
</head>
<body>
    <div id="swagger-ui"></div>
    <script src="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui-bundle.js"></script>
    <script src="https://cdn.jsdelivr.net/npm/swagger-ui-dist@5/swagger-ui-standalone-preset.js"></script>
    <script>
        window.onload = function() {
            SwaggerUIBundle({
                url: '/engine/openapi.json',
                dom_id: '#swagger-ui',
                deepLinking: true,
                presets: [SwaggerUIBundle.presets.apis, SwaggerUIStandalonePreset],
                layout: "StandaloneLayout"
            });
            setTimeout(function() {
                const nav = document.createElement('div');
                nav.id = 'aiwebengine-nav';
                nav.style.cssText = 'position: fixed; top: 10px; right: 10px; z-index: 1000; display: flex; gap: 8px;';
                const editorLink = document.createElement('a');
                editorLink.href = '/engine/editor';
                editorLink.textContent = 'Editor';
                editorLink.style.cssText = 'background: #f6f7f9; border: 1px solid #d1d5db; border-radius: 4px; padding: 6px 12px; font-size: 12px; color: #374151; text-decoration: none;';
                const graphqlLink = document.createElement('a');
                graphqlLink.href = '/engine/graphql';
                graphqlLink.textContent = 'GraphQL';
                graphqlLink.style.cssText = 'background: #f6f7f9; border: 1px solid #d1d5db; border-radius: 4px; padding: 6px 12px; font-size: 12px; color: #374151; text-decoration: none;';
                const docsLink = document.createElement('a');
                docsLink.href = '/engine/docs';
                docsLink.textContent = 'Docs';
                docsLink.style.cssText = 'background: #f6f7f9; border: 1px solid #d1d5db; border-radius: 4px; padding: 6px 12px; font-size: 12px; color: #374151; text-decoration: none;';
                nav.appendChild(editorLink);
                nav.appendChild(graphqlLink);
                nav.appendChild(docsLink);
                document.body.appendChild(nav);
                const style = document.createElement('style');
                style.textContent = '#aiwebengine-nav a:hover { background: #e5e7eb !important; border-color: #9ca3af !important; }';
                document.head.appendChild(style);
            }, 1000);
        };
    </script>
</body>
</html>"#
}

/// Try to serve an asset if the path matches a registered asset
fn try_serve_asset(path: &str, method: &str) -> Option<Response> {
    if method != "GET" {
        return None;
    }

    let asset_name = asset_registry::get_global_registry().get_asset_name(path)?;

    if let Some(asset) = repository::fetch_asset(&asset_name) {
        let mut response = asset.content.into_response();
        response.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_str(&asset.mimetype).unwrap_or(
                axum::http::HeaderValue::from_static("application/octet-stream"),
            ),
        );
        return Some(response);
    }

    warn!(
        "Asset '{}' registered for path '{}' but not found in repository",
        asset_name, path
    );
    None
}

/// Check if request should be routed to a stream handler
fn should_route_to_stream(path: &str, method: &str) -> bool {
    let is_get = method == "GET";
    let is_stream_registered = stream_registry::GLOBAL_STREAM_REGISTRY.is_stream_registered(path);

    info!(
        "Stream check - method: {}, is_get: {}, path: '{}', is_registered: {}",
        method, is_get, path, is_stream_registered
    );

    if is_get && is_stream_registered {
        info!("Routing to stream handler for path: {}", path);
        return true;
    }

    false
}

/// Build an HTTP response from a JavaScript response object
fn build_http_response_from_js(js_response: js_engine::JsHttpResponse) -> Response {
    let mut response = (
        StatusCode::from_u16(js_response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        js_response.body,
    )
        .into_response();

    // Add content type if specified
    if let Some(ct) = js_response.content_type {
        response.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_str(&ct)
                .unwrap_or_else(|_| axum::http::HeaderValue::from_static("text/plain")),
        );
    }

    // Add custom headers from JavaScript response
    for (name, value) in js_response.headers {
        if let Ok(header_name) = axum::http::HeaderName::from_bytes(name.as_bytes())
            && let Ok(header_value) = axum::http::HeaderValue::from_str(&value)
        {
            response.headers_mut().insert(header_name, header_value);
        }
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Once;

    static INIT_DB: Once = Once::new();

    fn setup_db() {
        INIT_DB.call_once(|| {
            let pool = sqlx::PgPool::connect_lazy(
                "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine",
            )
            .unwrap();
            let db = Arc::new(crate::database::Database::from_pool(pool));
            crate::database::initialize_global_database(db);
        });
    }

    #[test]
    fn test_parse_query_string() {
        // Test basic functionality
        let result = parse_query_string("id=123&name=test");
        assert_eq!(result.get("id"), Some(&"123".to_string()));
        assert_eq!(result.get("name"), Some(&"test".to_string()));

        // Test URL decoding
        let result = parse_query_string("name=test%20with%20spaces");
        assert_eq!(result.get("name"), Some(&"test with spaces".to_string()));

        // Test plus to space conversion
        let result = parse_query_string("name=test+with+plus");
        assert_eq!(result.get("name"), Some(&"test with plus".to_string()));

        // Test empty query
        let result = parse_query_string("");
        assert!(result.is_empty());

        // Test empty value
        let result = parse_query_string("empty=");
        assert_eq!(result.get("empty"), Some(&"".to_string()));

        // Test duplicate keys (last one wins)
        let result = parse_query_string("key=first&key=second");
        assert_eq!(result.get("key"), Some(&"second".to_string()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_editor_script_execution() {
        setup_db();
        // Load test scripts dynamically using upsert_script
        let _ = repository::upsert_script(
            "https://example.com/test_editor",
            include_str!("../scripts/test_scripts/test_editor.js"),
        );
        let _ = repository::upsert_script(
            "https://example.com/test_editor_api",
            include_str!("../scripts/test_scripts/test_editor_api.js"),
        );

        // Test that the editor script can be executed without errors and has init()
        let result = js_engine::execute_script(
            "https://example.com/editor",
            include_str!("../scripts/feature_scripts/editor.js"),
        );
        assert!(
            result.success,
            "Editor script should execute successfully: {:?}",
            result.error
        );

        // Test that calling init() on editor script captures registrations
        let init_context =
            crate::script_init::InitContext::new("https://example.com/editor".to_string(), false);
        let editor_registrations = js_engine::call_init_if_exists(
            "https://example.com/editor",
            include_str!("../scripts/feature_scripts/editor.js"),
            init_context,
        )
        .expect("Editor script init() should succeed");
        assert!(
            editor_registrations.is_some(),
            "Editor script should have init() function"
        );
        assert!(
            !editor_registrations.unwrap().is_empty(),
            "Editor script should register routes in init()"
        );

        // Test that the test_editor script can be executed without errors
        let test_editor_result = js_engine::execute_script(
            "https://example.com/test_editor",
            include_str!("../scripts/test_scripts/test_editor.js"),
        );
        assert!(
            test_editor_result.success,
            "Test editor script should execute successfully: {:?}",
            test_editor_result.error
        );

        // Test that calling init() on test_editor script captures registrations
        let test_init_context = crate::script_init::InitContext::new(
            "https://example.com/test_editor".to_string(),
            false,
        );
        let test_editor_registrations = js_engine::call_init_if_exists(
            "https://example.com/test_editor",
            include_str!("../scripts/test_scripts/test_editor.js"),
            test_init_context,
        )
        .expect("Test editor script init() should succeed");
        assert!(
            test_editor_registrations.is_some(),
            "Test editor script should have init() function"
        );
        assert!(
            !test_editor_registrations.unwrap().is_empty(),
            "Test editor script should register routes in init()"
        );
    }

    #[test]
    fn test_script_crud_operations() {
        // Test script retrieval
        let script_content = repository::fetch_script("https://example.com/core");
        assert!(script_content.is_some(), "Core script should exist");
        assert!(
            script_content.unwrap().contains("function"),
            "Core script should contain functions"
        );

        // Test script upsert and retrieval
        let test_uri = "https://example.com/test_script";
        let test_content = "// Test script\nfunction test() { return 'hello'; }";
        let _ = repository::upsert_script(test_uri, test_content);

        let retrieved = repository::fetch_script(test_uri);
        assert_eq!(
            retrieved,
            Some(test_content.to_string()),
            "Script should be retrievable after upsert"
        );
    }

    #[test]
    fn test_route_specificity_calculation() {
        // Test exact path (highest specificity)
        assert_eq!(
            calculate_route_specificity("/api/users/profile"),
            3000, // 3 exact segments × 1000
            "Exact path should have highest specificity"
        );

        // Test path with parameters
        assert_eq!(
            calculate_route_specificity("/api/users/:id"),
            2100, // 2 exact × 1000 + 1 param × 100
            "Path with param should have medium specificity"
        );

        // Test wildcard path (lowest specificity)
        assert_eq!(
            calculate_route_specificity("/api/users/*"),
            2000 - 10, // 2 exact × 1000 - 1 wildcard depth × 10
            "Wildcard path should have lower specificity"
        );

        // Test that more specific wildcards rank higher
        assert!(
            calculate_route_specificity("/api/scripts/*/owners")
                > calculate_route_specificity("/api/scripts/*"),
            "More specific wildcard should rank higher"
        );

        // Verify exact > param > wildcard ordering
        let exact = calculate_route_specificity("/api/users/123");
        let param = calculate_route_specificity("/api/users/:id");
        let wildcard = calculate_route_specificity("/api/users/*");
        assert!(
            exact > param && param > wildcard,
            "Specificity should be: exact > param > wildcard"
        );
    }

    #[test]
    fn test_route_pattern_matching() {
        // Test exact match
        let params = match_route_pattern("/api/users", "/api/users");
        assert!(params.is_some(), "Exact paths should match");
        assert!(params.unwrap().is_empty(), "No params for exact match");

        // Test parameter extraction
        let params = match_route_pattern("/api/users/:id", "/api/users/123");
        assert!(params.is_some(), "Parameterized path should match");
        let extracted = params.unwrap();
        assert_eq!(extracted.get("id"), Some(&"123".to_string()));

        // Test multiple parameters
        let params =
            match_route_pattern("/api/users/:userId/posts/:postId", "/api/users/42/posts/99");
        assert!(params.is_some(), "Multiple params should match");
        let extracted = params.unwrap();
        assert_eq!(extracted.get("userId"), Some(&"42".to_string()));
        assert_eq!(extracted.get("postId"), Some(&"99".to_string()));

        // Test non-match due to different segment count
        let params = match_route_pattern("/api/users/:id", "/api/users/123/extra");
        assert!(params.is_none(), "Different segment counts shouldn't match");

        // Test non-match due to different literal segments
        let params = match_route_pattern("/api/users/:id", "/api/posts/123");
        assert!(params.is_none(), "Different literals shouldn't match");
    }
}
