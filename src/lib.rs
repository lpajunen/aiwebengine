use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response, Sse, sse::Event};
use axum::{Router, routing::any};
use axum_server::Server;
use futures::StreamExt as FuturesStreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use tokio_stream::wrappers::BroadcastStream;
use tracing::{debug, error, info, warn};

pub mod asset_registry;
pub mod config;
pub mod database;
pub mod error;
pub mod graphql;
pub mod http_client;
pub mod js_engine;
pub mod middleware;
pub mod repository;
pub mod safe_helpers;
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

use security::UserContext;

/// Parses a query string into a HashMap of key-value pairs
fn parse_query_string(query: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();
    if query.is_empty() {
        return params;
    }

    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or("");
        let value = parts.next().unwrap_or("");

        // URL decode the key and value
        let decoded_key = urlencoding::decode(key).unwrap_or(std::borrow::Cow::Borrowed(key));
        let decoded_value = urlencoding::decode(value).unwrap_or(std::borrow::Cow::Borrowed(value));

        // Convert + to spaces (though urlencoding should handle this, being explicit)
        let final_key = decoded_key.replace('+', " ");
        let final_value = decoded_value.replace('+', " ");

        params.insert(final_key, final_value);
    }

    params
}

fn extract_subscription_name(query: &str) -> String {
    // Simple extraction of subscription name from GraphQL query
    // This is a basic implementation - in production you might want more sophisticated parsing

    // Remove leading/trailing whitespace and normalize
    let query = query.trim();

    // Look for "subscription" keyword followed by the subscription name
    if let Some(subscription_pos) = query.find("subscription") {
        let after_subscription = &query[subscription_pos + "subscription".len()..];

        // Skip whitespace after "subscription"
        let after_subscription = after_subscription.trim_start();

        // Check if it starts with '{', meaning no subscription name
        if after_subscription.starts_with('{') {
            // Extract the first field name from the subscription
            if let Some(end_pos) = after_subscription.find('}') {
                let subscription_body = &after_subscription[1..end_pos]; // Remove opening brace
                // Find the first field name (skip whitespace and extract until whitespace or end)
                let field_name = subscription_body.trim_start();
                if let Some(space_pos) = field_name.find(char::is_whitespace) {
                    field_name[..space_pos].trim().to_string()
                } else if let Some(brace_pos) = field_name.find('{') {
                    field_name[..brace_pos].trim().to_string()
                } else {
                    field_name.trim().to_string()
                }
            } else {
                "anonymous_subscription".to_string()
            }
        } else {
            // Look for the opening brace or whitespace that indicates the end of the subscription name
            if let Some(brace_pos) = after_subscription.find('{') {
                let subscription_name = after_subscription[..brace_pos].trim();
                if !subscription_name.is_empty() {
                    return subscription_name.to_string();
                }
            }

            // Fallback: look for whitespace after subscription name
            if let Some(space_pos) = after_subscription.find(char::is_whitespace) {
                let subscription_name = after_subscription[..space_pos].trim();
                if !subscription_name.is_empty() {
                    return subscription_name.to_string();
                }
            }

            // If we can't find a name, use the first field
            if let Some(brace_pos) = after_subscription.find('{') {
                let subscription_body = &after_subscription[brace_pos + 1..];
                if let Some(end_pos) = subscription_body.find('}') {
                    let field_name = subscription_body[..end_pos].trim_start();
                    if let Some(space_pos) = field_name.find(char::is_whitespace) {
                        field_name[..space_pos].trim().to_string()
                    } else if let Some(brace_pos) = field_name.find('{') {
                        field_name[..brace_pos].trim().to_string()
                    } else {
                        field_name.trim().to_string()
                    }
                } else {
                    "anonymous_subscription".to_string()
                }
            } else {
                "anonymous_subscription".to_string()
            }
        }
    } else {
        // Fallback to a generic name if we can't parse it
        "unknown_subscription".to_string()
    }
}

/// Parses form data from request body based on content type
async fn parse_form_data(
    content_type: Option<&str>,
    body: Body,
) -> Option<HashMap<String, String>> {
    if let Some(ct) = content_type {
        if ct.starts_with("application/x-www-form-urlencoded") {
            // Convert body to bytes and parse as URL-encoded form data
            let bytes = match to_bytes(body, usize::MAX).await {
                Ok(b) => b,
                Err(_) => return None,
            };
            let body_str = String::from_utf8(bytes.to_vec()).ok()?;
            Some(serde_urlencoded::from_str(&body_str).unwrap_or_default())
        } else if ct.starts_with("multipart/form-data") {
            // For multipart, we'd need to parse the boundary from content-type
            // This is more complex and would require additional implementation
            // For now, return empty map
            Some(HashMap::new())
        } else {
            None
        }
    } else {
        None
    }
}

/// Handle Server-Sent Events stream requests
async fn handle_stream_request(req: Request<Body>) -> Response {
    let path = req.uri().path().to_string();
    let query_string = req.uri().query().map(|s| s.to_string()).unwrap_or_default();
    let query_params = parse_query_string(&query_string);

    info!(
        "Handling stream request for path: {} with query params: {:?}",
        path, query_params
    );

    // Use query parameters as client metadata for selective broadcasting
    let client_metadata = if query_params.is_empty() {
        None
    } else {
        Some(query_params)
    };

    // Create a connection with the stream manager
    let connection = match stream_manager::StreamConnectionManager::new()
        .create_connection(&path, client_metadata)
        .await
    {
        Ok(conn) => conn,
        Err(e) => {
            error!("Failed to create stream connection for '{}': {}", path, e);
            return Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .header("content-type", "text/plain")
                .body(Body::from(format!(
                    "Failed to create stream connection: {}",
                    e
                )))
                .unwrap_or_else(|err| {
                    error!("Failed to build error response: {}", err);
                    Response::new(Body::from("Internal Server Error"))
                });
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

/// Dynamically find a route handler by checking cached registrations from init()
fn find_route_handler(path: &str, method: &str) -> Option<(String, String)> {
    // Fetch all script metadata which includes cached registrations from init()
    let all_metadata = match repository::get_all_script_metadata() {
        Ok(metadata) => metadata,
        Err(e) => {
            error!("Failed to fetch script metadata: {}", e);
            return None;
        }
    };

    for metadata in all_metadata {
        // Use cached registrations from init() function
        if metadata.initialized && !metadata.registrations.is_empty() {
            // Check for exact match
            if let Some(handler) = metadata
                .registrations
                .get(&(path.to_string(), method.to_string()))
            {
                return Some((metadata.uri.clone(), handler.clone()));
            }

            // Check for wildcard matches
            for ((pattern, reg_method), handler) in &metadata.registrations {
                if reg_method == method && pattern.ends_with("/*") {
                    let prefix = &pattern[..pattern.len() - 1]; // Remove the *
                    if path.starts_with(prefix) {
                        return Some((metadata.uri.clone(), handler.clone()));
                    }
                }
            }
        }
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
) -> Result<Arc<auth::AuthManager>, auth::AuthError> {
    use auth::{
        AuthManager, AuthManagerConfig, AuthSecurityContext, AuthSessionManager, CookieSameSite,
    };
    use security::{
        CsrfProtection, DataEncryption, RateLimiter, SecureSessionManager, SecurityAuditor,
    };

    // Create security infrastructure
    let auditor = Arc::new(SecurityAuditor::new());

    // Create rate limiter
    let rate_limiter = Arc::new(RateLimiter::new());

    // Create CSRF protection with random key
    let csrf_key: [u8; 32] = rand::random();
    let csrf = Arc::new(CsrfProtection::new(csrf_key, 3600)); // 1 hour lifetime

    // Create encryption with random key for sessions
    let encryption_key: [u8; 32] = rand::random();
    let encryption = Arc::new(DataEncryption::new(&encryption_key));

    // Create secure session manager
    let session_manager = Arc::new(SecureSessionManager::new(
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

    // Create AuthManager config from auth config
    let manager_config = AuthManagerConfig {
        base_url: "http://localhost:8080".to_string(), // TODO: Get from app config
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
    let mut auth_manager = AuthManager::new(manager_config, auth_session_manager, security_context);

    // Register OAuth2 providers if configured
    if let Some(google_config) = auth_config.providers.google {
        info!("Registering Google OAuth2 provider");
        let oauth_config = auth::OAuth2ProviderConfig {
            client_id: google_config.client_id,
            client_secret: google_config.client_secret,
            redirect_uri: google_config.redirect_uri,
            scopes: if !google_config.scopes.is_empty() {
                google_config.scopes
            } else {
                vec![
                    "openid".to_string(),
                    "profile".to_string(),
                    "email".to_string(),
                ]
            },
            auth_url: None, // Use default Google URLs
            token_url: None,
            userinfo_url: None,
            extra_params: HashMap::new(),
        };
        auth_manager.register_provider("google", oauth_config)?;
    }

    if let Some(microsoft_config) = auth_config.providers.microsoft {
        info!("Registering Microsoft OAuth2 provider");
        let mut extra_params = HashMap::new();
        if let Some(tenant_id) = microsoft_config.tenant_id {
            extra_params.insert("tenant_id".to_string(), tenant_id);
        }
        let oauth_config = auth::OAuth2ProviderConfig {
            client_id: microsoft_config.client_id,
            client_secret: microsoft_config.client_secret,
            redirect_uri: microsoft_config.redirect_uri,
            scopes: if !microsoft_config.scopes.is_empty() {
                microsoft_config.scopes
            } else {
                vec![
                    "openid".to_string(),
                    "profile".to_string(),
                    "email".to_string(),
                ]
            },
            auth_url: None, // Use default Microsoft URLs
            token_url: None,
            userinfo_url: None,
            extra_params,
        };
        auth_manager.register_provider("microsoft", oauth_config)?;
    }

    if let Some(apple_config) = auth_config.providers.apple {
        info!("Registering Apple OAuth2 provider");
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
        let oauth_config = auth::OAuth2ProviderConfig {
            client_id: apple_config.client_id,
            client_secret: apple_config.client_secret,
            redirect_uri: apple_config.redirect_uri,
            scopes: if !apple_config.scopes.is_empty() {
                apple_config.scopes
            } else {
                vec!["name".to_string(), "email".to_string()]
            },
            auth_url: None, // Use default Apple URLs
            token_url: None,
            userinfo_url: None,
            extra_params,
        };
        auth_manager.register_provider("apple", oauth_config)?;
    }

    Ok(Arc::new(auth_manager))
}

/// Starts the web server with the given shutdown receiver.
///
/// This function:
/// 1. Sets up the Axum router with dynamic route handling
/// 2. Starts the server on the configured address
/// 3. Listens for shutdown signal
pub async fn start_server(shutdown_rx: tokio::sync::oneshot::Receiver<()>) -> anyhow::Result<u16> {
    start_server_with_config(config::Config::from_env(), shutdown_rx).await
}

/// Starts the web server with custom configuration
pub async fn start_server_with_config(
    config: config::Config,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) -> anyhow::Result<u16> {
    // Initialize secrets manager
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

    let secrets_manager = std::sync::Arc::new(secrets_manager);

    // Set as global secrets manager for access from js_engine
    if secrets::initialize_global_secrets_manager(secrets_manager.clone()) {
        info!("Global secrets manager initialized successfully");
    } else {
        warn!("Global secrets manager was already initialized");
    }

    // Initialize database connection
    info!("Initializing database connection...");
    info!(
        "Repository config - storage_type: {}",
        config.repository.storage_type
    );
    if let Some(ref conn_str) = config.repository.connection_string {
        // Log sanitized connection string (hide password)
        let safe_conn_str = if let Some(at_pos) = conn_str.find('@') {
            let before_at = &conn_str[..at_pos];
            let after_at = &conn_str[at_pos..];
            if let Some(colon_pos) = before_at.rfind(':') {
                format!("{}:****{}", &before_at[..colon_pos], after_at)
            } else {
                conn_str.clone()
            }
        } else {
            conn_str.clone()
        };
        info!("Repository config - connection_string: {}", safe_conn_str);
    } else {
        warn!("Repository config - connection_string: None (not set!)");
    }

    match database::init_database(
        &config.repository,
        config.repository.storage_type == "postgresql",
    )
    .await
    {
        Ok(db) => {
            let db_arc = std::sync::Arc::new(db);
            if database::initialize_global_database(db_arc) {
                info!("Global database initialized successfully");
            } else {
                warn!("Global database was already initialized");
            }
        }
        Err(e) => {
            // Only fail if we're trying to use PostgreSQL storage
            if config.repository.storage_type == "postgresql" {
                return Err(anyhow::anyhow!(
                    "Database initialization failed: {}. Cannot continue with PostgreSQL storage.",
                    e
                ));
            } else {
                warn!(
                    "Database initialization failed: {}. Continuing without database (using {} storage).",
                    e, config.repository.storage_type
                );
                warn!("Health checks will report database as unavailable");
            }
        }
    }

    // Clone the timeout value to avoid borrow checker issues in async closures
    let script_timeout_ms = config.script_timeout_ms();

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
    if let Err(e) = repository::bootstrap_assets() {
        warn!(
            "Failed to bootstrap assets: {}. Continuing with static assets.",
            e
        );
    }

    // Execute all scripts at startup to populate GraphQL registry
    info!("Executing all scripts at startup to populate GraphQL registry...");
    let scripts = repository::fetch_scripts();
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
            repository::insert_log_message(uri, &error_msg, "FATAL");
        } else {
            info!("Successfully executed script: {}", uri);
        }
    }

    // Initialize all scripts by calling their init() functions if they exist
    if config.javascript.enable_init_functions {
        info!("Initializing all scripts...");
        let init_timeout = config
            .javascript
            .init_timeout_ms
            .unwrap_or(config.javascript.execution_timeout_ms);
        let initializer = script_init::ScriptInitializer::new(init_timeout);
        match initializer.initialize_all_scripts().await {
            Ok(results) => {
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
                        warn!(
                            "Script '{}' initialization failed: {}",
                            result.script_uri, error
                        );
                    }
                }

                // Fail startup if configured and any script failed
                if config.javascript.fail_startup_on_init_error && failed > 0 {
                    anyhow::bail!(
                        "Server startup aborted: {} script(s) failed initialization",
                        failed
                    );
                }
            }
            Err(e) => {
                error!("Failed to initialize scripts: {}", e);
                if config.javascript.fail_startup_on_init_error {
                    anyhow::bail!(
                        "Server startup aborted: script initialization failed: {}",
                        e
                    );
                }
            }
        }
    } else {
        info!("Script init() functions are disabled in configuration");
    }

    // Initialize GraphQL schema (will be rebuilt dynamically as needed)
    if let Err(e) = graphql::rebuild_schema() {
        error!("Failed to initialize GraphQL schema: {:?}", e);
        // Don't fail startup, just log the error
    }

    // Initialize authentication if configured and enabled
    let auth_manager: Option<Arc<auth::AuthManager>> = if let Some(auth_config) =
        config.auth.clone()
        && auth_config.enabled
    {
        info!("Authentication is enabled, initializing AuthManager...");
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

        match initialize_auth_manager(auth_config).await {
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
    };

    // Determine if auth is enabled
    let auth_enabled = auth_manager.is_some();

    // GraphQL GET handler - serves GraphiQL (requires authentication when auth is enabled)
    let graphql_get_handler = move |req: axum::http::Request<axum::body::Body>| async move {
        // Check for authentication when auth is enabled
        if auth_enabled && req.extensions().get::<auth::AuthUser>().is_none() {
            return axum::response::Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .header("content-type", "application/json")
                .body(axum::body::Body::from(
                    serde_json::json!({"error": "Authentication required"}).to_string(),
                ))
                .unwrap_or_else(|err| {
                    error!("Failed to build unauthorized response: {}", err);
                    axum::response::Response::new(axum::body::Body::from("Unauthorized"))
                })
                .into_response();
        }

        let graphiql_html = async_graphql::http::GraphiQLSource::build()
            .endpoint("/graphql")
            .subscription_endpoint("/graphql/sse")
            .title("aiwebengine GraphQL Editor")
            .finish();

        // Create a custom HTML response with navigation
        let navigation_script = r#"<script>
document.addEventListener('DOMContentLoaded', function() {
    setTimeout(function() {
        const nav = document.createElement('div');
        nav.id = 'aiwebengine-nav';
        nav.style.cssText = 'position: absolute; top: 10px; right: 10px; z-index: 1000; display: flex; gap: 8px;';
        
        const editorLink = document.createElement('a');
        editorLink.href = '/editor';
        editorLink.textContent = 'Editor';
        editorLink.style.cssText = 'background: #f6f7f9; border: 1px solid #d1d5db; border-radius: 4px; padding: 6px 12px; font-size: 12px; color: #374151; text-decoration: none;';
        
        const docsLink = document.createElement('a');
        docsLink.href = '/engine/docs';
        docsLink.textContent = 'Docs';
        docsLink.style.cssText = 'background: #f6f7f9; border: 1px solid #d1d5db; border-radius: 4px; padding: 6px 12px; font-size: 12px; color: #374151; text-decoration: none;';
        
        nav.appendChild(editorLink);
        nav.appendChild(docsLink);
        document.body.appendChild(nav);
        
        // Add hover effects
        const style = document.createElement('style');
        style.textContent = '#aiwebengine-nav a:hover { background: #e5e7eb !important; border-color: #9ca3af !important; }';
        document.head.appendChild(style);
    }, 2000);
});
</script>"#;

        let mut full_html = graphiql_html;
        full_html.push_str(navigation_script);

        axum::response::Html(full_html).into_response()
    };

    // GraphQL POST handler - executes queries
    async fn graphql_post(req: axum::http::Request<axum::body::Body>) -> impl IntoResponse {
        let (_parts, body) = req.into_parts();
        let body_bytes = match axum::body::to_bytes(body, usize::MAX).await {
            Ok(bytes) => bytes,
            Err(_) => {
                return axum::response::Json(
                    serde_json::json!({"error": "Failed to read request body"}),
                );
            }
        };

        let request: async_graphql::Request = match serde_json::from_slice(&body_bytes) {
            Ok(req) => req,
            Err(e) => {
                return axum::response::Json(
                    serde_json::json!({"error": format!("Invalid JSON: {}", e)}),
                );
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

        let response = schema.execute(request).await;
        axum::response::Json(serde_json::to_value(response).unwrap_or(serde_json::Value::Null))
    }

    // GraphQL SSE handler - handles subscriptions over Server-Sent Events using execute_stream
    async fn graphql_sse(req: axum::http::Request<axum::body::Body>) -> impl IntoResponse {
        let (parts, body) = req.into_parts();
        let query_string = parts.uri.query().map(|s| s.to_string()).unwrap_or_default();
        let query_params = parse_query_string(&query_string);

        info!("GraphQL SSE request with query params: {:?}", query_params);

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

        let request: async_graphql::Request = match serde_json::from_slice(&body_bytes) {
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

        // Check if this is a subscription operation
        let is_subscription = request.query.trim_start().starts_with("subscription");

        if is_subscription {
            // For subscriptions, create a connection with metadata to enable selective broadcasting
            // Extract subscription name from the query to determine the stream path
            let subscription_name = extract_subscription_name(&request.query);
            let stream_path = format!("/graphql/subscription/{}", subscription_name);

            // Use query parameters as client metadata for selective broadcasting
            let client_metadata = if query_params.is_empty() {
                None
            } else {
                Some(query_params)
            };

            // Create a connection with the stream manager for this subscription
            let connection = match stream_manager::StreamConnectionManager::new()
                .create_connection(&stream_path, client_metadata)
                .await
            {
                Ok(conn) => conn,
                Err(e) => {
                    error!(
                        "Failed to create GraphQL subscription connection for '{}': {}",
                        stream_path, e
                    );
                    return axum::response::Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .header("content-type", "text/plain")
                        .body(Body::from(format!(
                            "Failed to create subscription connection: {}",
                            e
                        )))
                        .unwrap_or_else(|err| {
                            error!("Failed to build error response: {}", err);
                            Response::new(Body::from("Internal Server Error"))
                        });
                }
            };

            let connection_id = connection.connection_id.clone();
            info!(
                "Created GraphQL subscription connection {} for path '{}' with metadata: {:?}",
                connection_id, stream_path, connection.client_metadata
            );

            // Use execute_stream for subscriptions - this provides native GraphQL subscription lifecycle
            // We need to move the schema into a spawn to handle the lifetime requirements
            let (tx, rx) = tokio::sync::mpsc::channel(100);

            tokio::spawn(async move {
                let stream = schema.execute_stream(request);

                // Convert each GraphQL response to SSE event and send via channel
                let mut stream = std::pin::pin!(stream);
                while let Some(response) = FuturesStreamExt::next(&mut stream).await {
                    let event = if response.data != async_graphql::Value::Null {
                        // Send data as SSE event
                        let json_data = serde_json::to_string(&response.data)
                            .unwrap_or_else(|_| "{}".to_string());
                        Ok::<Event, std::convert::Infallible>(Event::default().data(json_data))
                    } else if !response.errors.is_empty() {
                        // Send errors as SSE event
                        let error_json = serde_json::json!({
                            "errors": response.errors.iter().map(|e| e.message.clone()).collect::<Vec<_>>()
                        });
                        Ok::<Event, std::convert::Infallible>(
                            Event::default().data(error_json.to_string()),
                        )
                    } else {
                        // Send empty data
                        Ok::<Event, std::convert::Infallible>(Event::default().data("{}"))
                    };

                    if tx.send(event).await.is_err() {
                        // Connection closed, clean up the subscription connection
                        if let Err(cleanup_err) = stream_registry::GLOBAL_STREAM_REGISTRY
                            .remove_connection(&stream_path, &connection_id)
                        {
                            error!(
                                "Failed to cleanup GraphQL subscription connection {}: {}",
                                connection_id, cleanup_err
                            );
                        } else {
                            debug!(
                                "Cleaned up GraphQL subscription connection {} from stream {}",
                                connection_id, stream_path
                            );
                        }
                        break; // Receiver dropped, stop streaming
                    }
                }
            });

            let receiver_stream = tokio_stream::wrappers::ReceiverStream::new(rx);
            Sse::new(receiver_stream)
                .keep_alive(axum::response::sse::KeepAlive::default())
                .into_response()
        } else {
            // Handle regular queries/mutations as single response
            let response = schema.execute(request).await;
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
    }

    // Clone schema for handlers
    // (No longer needed - handlers get schema dynamically)

    // Shared request handler function for both / and /{*path} routes
    async fn handle_dynamic_request(
        req: Request<Body>,
        script_timeout_ms: u64,
        _auth_enabled: bool,
    ) -> impl IntoResponse {
        let path = req.uri().path().to_string();
        let request_method = req.method().to_string();

        // Check for registered asset paths first if it's a GET request
        if request_method == "GET" {
            // Check if this path is registered in the asset registry
            if let Some(asset_name) = asset_registry::get_global_registry().get_asset_name(&path) {
                // Fetch the asset by name from the repository
                if let Some(asset) = repository::fetch_asset(&asset_name) {
                    let mut response = asset.content.into_response();
                    response.headers_mut().insert(
                        axum::http::header::CONTENT_TYPE,
                        axum::http::HeaderValue::from_str(&asset.mimetype).unwrap_or(
                            axum::http::HeaderValue::from_static("application/octet-stream"),
                        ),
                    );
                    return response;
                } else {
                    warn!(
                        "Asset '{}' registered for path '{}' but not found in repository",
                        asset_name, path
                    );
                    // Fall through to route handling
                }
            }
        }

        // Check if this is a request to a registered stream path
        let is_get = request_method == "GET";
        let is_stream_registered =
            stream_registry::GLOBAL_STREAM_REGISTRY.is_stream_registered(&path);
        info!(
            "Stream check - method: {}, is_get: {}, path: '{}', is_registered: {}",
            request_method, is_get, path, is_stream_registered
        );

        if is_get && is_stream_registered {
            info!("Routing to stream handler for path: {}", path);
            return handle_stream_request(req).await;
        }

        // Check if any route exists for this path (including wildcards)
        let path_exists = path_has_any_route(&path);

        let reg = find_route_handler(&path, &request_method);
        let (owner_uri, handler_name) = match reg {
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
                    let error_response =
                        error::errors::method_not_allowed(&path, &request_method, &request_id);
                    let status = StatusCode::from_u16(error_response.status)
                        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                    let body = serde_json::to_string(&error_response)
                        .unwrap_or_else(|_| r#"{"error":"Serialization failed"}"#.to_string());
                    return (status, body).into_response();
                } else {
                    warn!(
                        "[{}] ⚠️  Route not found: {} {} (no handler registered for this path)",
                        request_id, request_method, path
                    );
                    let error_response = error::errors::not_found(&path, &request_id);
                    let status = StatusCode::from_u16(error_response.status)
                        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                    let body = serde_json::to_string(&error_response)
                        .unwrap_or_else(|_| r#"{"error":"Serialization failed"}"#.to_string());
                    return (status, body).into_response();
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

        // Make raw body available for all POST/PUT/PATCH requests
        let raw_body = if !body_bytes.is_empty()
            && (request_method == "POST" || request_method == "PUT" || request_method == "PATCH")
        {
            Some(String::from_utf8(body_bytes.to_vec()).unwrap_or_default())
        } else {
            None
        };

        // Parse form data if content type indicates form submission
        let is_form_data = content_type
            .as_ref()
            .map(|ct| {
                ct.contains("application/x-www-form-urlencoded")
                    || ct.contains("multipart/form-data")
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

            // Use the secure execution path with authentication context
            let params = js_engine::RequestExecutionParams {
                script_uri: owner_uri_cl.clone(),
                handler_name: handler_cl.clone(),
                path: path_clone.clone(),
                method: request_method.clone(),
                query_params: Some(query_params.clone()),
                form_data: Some(form_data.clone()),
                raw_body: raw_body.clone(),
                user_context: security::UserContext::anonymous(), // TODO: Extract from auth
                auth_context: Some(auth_context),
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
                    let error_response = error::errors::script_timeout(&path, &request_id);
                    let status = StatusCode::from_u16(error_response.status)
                        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                    let body = serde_json::to_string(&error_response)
                        .unwrap_or_else(|_| r#"{"error":"Serialization failed"}"#.to_string());
                    return (status, body).into_response();
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
                let mut response = (
                    StatusCode::from_u16(js_response.status)
                        .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
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

                let error_response = error::errors::script_execution_failed(&path, &e, &request_id);
                let status = StatusCode::from_u16(error_response.status)
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                let body = serde_json::to_string(&error_response)
                    .unwrap_or_else(|_| r#"{"error":"Serialization failed"}"#.to_string());
                (status, body).into_response()
            }
            Err(e) => {
                error!(
                    "[{}] ❌ Task/runtime error for {} {}: {} (handler: {}, script: {})",
                    request_id, method_log, path_log, e, handler_name, owner_uri
                );
                let error_response = error::errors::internal_server_error(&path, &e, &request_id);
                let status = StatusCode::from_u16(error_response.status)
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                let body = serde_json::to_string(&error_response)
                    .unwrap_or_else(|_| r#"{"error":"Serialization failed"}"#.to_string());
                (status, body).into_response()
            }
        }
    }

    // Build the router
    let mut app = Router::new();

    // Add GraphQL and editor routes with authentication requirement if auth is enabled
    if let Some(ref auth_mgr) = auth_manager {
        info!("✅ Authentication ENABLED - mounting auth routes and middleware");

        // GraphQL endpoints with require_editor_or_admin_middleware
        let auth_mgr_for_graphql = Arc::clone(auth_mgr);
        let graphql_router = Router::new()
            .route("/graphql", axum::routing::get(graphql_get_handler))
            .route("/graphql", axum::routing::post(graphql_post))
            .route("/graphql/sse", axum::routing::post(graphql_sse))
            .layer(axum::middleware::from_fn_with_state(
                auth_mgr_for_graphql,
                auth::require_editor_or_admin_middleware,
            ));

        app = app.merge(graphql_router);

        // Mount authentication routes
        let auth_router = auth::create_auth_router(Arc::clone(auth_mgr));
        app = app.nest("/auth", auth_router);
    } else {
        warn!("⚠️  Authentication DISABLED - no auth routes or middleware");

        // GraphQL endpoints without authentication
        app = app
            .route("/graphql", axum::routing::get(graphql_get_handler))
            .route("/graphql", axum::routing::post(graphql_post))
            .route("/graphql/sse", axum::routing::post(graphql_sse));
    }

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
    if let Some(ref auth_mgr) = auth_manager {
        let auth_mgr_for_middleware = Arc::clone(auth_mgr);
        info!("✅ Adding optional_auth_middleware layer to all routes");
        app = app.layer(axum::middleware::from_fn_with_state(
            auth_mgr_for_middleware,
            auth::optional_auth_middleware,
        ));
    }

    app = app.layer(axum::middleware::from_fn(middleware::request_id_middleware));

    let addr: std::net::SocketAddr = config
        .server_addr()
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid server address: {}", e))?;

    // Try to find an available port starting from the configured port
    let mut current_port = config.port();
    let mut actual_addr = addr;
    let mut attempts = 0;
    const MAX_PORT_ATTEMPTS: u16 = 100; // Try up to 100 ports

    loop {
        // Handle automatic port assignment (port 0)
        if config.port() == 0 {
            // Bind to port 0 to let OS assign a free port
            let test_bind = std::net::TcpListener::bind(actual_addr);
            match test_bind {
                Ok(listener) => {
                    // Get the actual port assigned by the OS
                    let actual_port = listener
                        .local_addr()
                        .map_err(|e| anyhow::anyhow!("Failed to get local address: {}", e))?
                        .port();
                    drop(listener);

                    let actual_addr = format!("{}:{}", config.host(), actual_port)
                        .parse()
                        .map_err(|e| anyhow::anyhow!("Invalid server address: {}", e))?;

                    info!("Auto-assigned port: {}", actual_port);

                    // record startup in logs so tests can observe server start
                    repository::insert_log_message("server", "server started", "INFO");
                    debug!(
                        "Server configuration - host: {}, requested port: {}, actual port: {}",
                        config.host(),
                        config.port(),
                        actual_port
                    );

                    let svc = app.into_make_service();
                    let server = Server::bind(actual_addr).serve(svc);

                    // Spawn the server in a background task so we can return immediately
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

                    return Ok(actual_port);
                }
                Err(e) => {
                    return Err(anyhow::anyhow!(
                        "Failed to bind to auto-assigned port: {}",
                        e
                    ));
                }
            }
        }

        // First check if the port is available using TcpListener
        let test_bind = std::net::TcpListener::bind(actual_addr);
        match test_bind {
            Ok(_) => {
                // Port is available, close the test listener and proceed with axum-server
                drop(test_bind);

                // Successfully found an available port
                if current_port != config.port() {
                    info!(
                        "Requested port {} was in use, using port {} instead",
                        config.port(),
                        current_port
                    );
                } else {
                    info!("listening on {}", actual_addr);
                }

                // record startup in logs so tests can observe server start
                repository::insert_log_message("server", "server started", "INFO");
                debug!(
                    "Server configuration - host: {}, requested port: {}, actual port: {}",
                    config.host(),
                    config.port(),
                    current_port
                );

                let svc = app.into_make_service();
                let server = Server::bind(actual_addr).serve(svc);

                // Spawn the server in a background task so we can return immediately
                tokio::spawn(async move {
                    tokio::select! {
                        res = server => {
                            if let Err(e) = res {
                                eprintln!("Server error: {:?}", e);
                            }
                        },
                        _ = &mut shutdown_rx => { /* graceful shutdown: stop accepting new connections */ }
                    }
                });

                return Ok(current_port);
            }
            Err(e) => {
                // Check if it's an "Address already in use" error
                let error_msg = e.to_string().to_lowercase();
                if error_msg.contains("address already in use")
                    || error_msg.contains("address in use")
                    || error_msg.contains("eaddrinuse")
                    || e.kind() == std::io::ErrorKind::AddrInUse
                {
                    attempts += 1;
                    if attempts >= MAX_PORT_ATTEMPTS {
                        return Err(anyhow::anyhow!(
                            "Could not find an available port after trying {} ports starting from {}",
                            MAX_PORT_ATTEMPTS,
                            config.port()
                        ));
                    }

                    // Try the next port
                    current_port += 1;
                    actual_addr = format!("{}:{}", config.host(), current_port)
                        .parse()
                        .map_err(|e| anyhow::anyhow!("Invalid server address: {}", e))?;

                    debug!(
                        "Port {} in use, trying port {}",
                        current_port - 1,
                        current_port
                    );
                } else {
                    // Some other error, return it
                    return Err(anyhow::anyhow!(
                        "Failed to bind to address {}: {}",
                        actual_addr,
                        e
                    ));
                }
            }
        }
    }
}

pub async fn start_server_without_shutdown() -> anyhow::Result<u16> {
    let mut config = config::Config::from_env();
    config.server.port = 0; // Use port 0 for automatic port assignment
    // Create a channel that will never receive a shutdown signal
    let (tx, rx) = tokio::sync::oneshot::channel::<()>();
    // Leak the sender so it never gets dropped and the channel never closes
    Box::leak(Box::new(tx));
    start_server_with_config(config, rx).await
}

pub async fn start_server_without_shutdown_with_config(
    config: config::Config,
) -> anyhow::Result<u16> {
    let (_tx, rx) = tokio::sync::oneshot::channel::<()>();
    start_server_with_config(config, rx).await
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn test_editor_script_execution() {
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
}
