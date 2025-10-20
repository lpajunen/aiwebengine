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

pub mod config;
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

// Authentication module (Phase 1 - Core Infrastructure)
pub mod auth;

use security::UserContext;

/// Parses a query string into a HashMap of key-value pairs
fn parse_query_string(query: &str) -> HashMap<String, String> {
    serde_urlencoded::from_str(query).unwrap_or_default()
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
async fn handle_stream_request(path: String) -> Response {
    info!("Handling stream request for path: {}", path);

    // Create a connection with the stream manager
    let connection = match stream_manager::StreamConnectionManager::new()
        .create_connection(&path, None)
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

    // Clone the timeout value to avoid borrow checker issues in async closures
    let script_timeout_ms = config.script_timeout_ms();

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

    // Create GraphQL schema after scripts have been executed
    let schema = match graphql::build_schema() {
        Ok(schema) => schema,
        Err(e) => {
            error!("Failed to build GraphQL schema: {:?}", e);
            // Return a minimal dynamic schema if building fails
            async_graphql::dynamic::Schema::build("Query", None, None)
                .register(async_graphql::dynamic::Object::new("Query").field(
                    async_graphql::dynamic::Field::new(
                        "error",
                        async_graphql::dynamic::TypeRef::named(
                            async_graphql::dynamic::TypeRef::STRING,
                        ),
                        |_| {
                            async_graphql::dynamic::FieldFuture::new(async {
                                Ok(Some(async_graphql::Value::String(
                                    "Schema build failed".to_string(),
                                )))
                            })
                        },
                    ),
                ))
                .finish()
                .unwrap_or_else(|_| panic!("Failed to build fallback schema"))
        }
    };

    // Initialize authentication if configured
    let auth_manager: Option<Arc<auth::AuthManager>> = if let Some(auth_config) =
        config.auth.clone()
    {
        info!("Authentication is enabled, initializing AuthManager...");
        debug!(
            "Auth config: enabled={}, providers={:?}",
            auth_config.enabled,
            auth_config.providers.enabled_providers()
        );
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
            "No authentication configuration found (config.auth is None), running without authentication"
        );
        None
    };

    // GraphQL GET handler - serves GraphiQL
    async fn graphql_get() -> impl IntoResponse {
        axum::response::Html(
            async_graphql::http::GraphiQLSource::build()
                .endpoint("/graphql")
                .finish(),
        )
    }

    // GraphQL POST handler - executes queries
    async fn graphql_post(
        schema: async_graphql::dynamic::Schema,
        req: axum::http::Request<axum::body::Body>,
    ) -> impl IntoResponse {
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

        let response = schema.execute(request).await;
        axum::response::Json(serde_json::to_value(response).unwrap_or(serde_json::Value::Null))
    }

    // GraphQL SSE handler - handles subscriptions over Server-Sent Events using execute_stream
    async fn graphql_sse(
        schema: async_graphql::dynamic::Schema,
        req: axum::http::Request<axum::body::Body>,
    ) -> impl IntoResponse {
        let (_parts, body) = req.into_parts();
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

        // Check if this is a subscription operation
        let is_subscription = request.query.trim_start().starts_with("subscription");

        if is_subscription {
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
    let schema_for_post = schema.clone();
    let schema_for_sse = schema.clone();

    // Shared request handler function for both / and /{*path} routes
    async fn handle_dynamic_request(
        req: Request<Body>,
        script_timeout_ms: u64,
    ) -> impl IntoResponse {
        let path = req.uri().path().to_string();
        let request_method = req.method().to_string();

        // Check for assets first if it's a GET request
        if request_method == "GET"
            && let Some(asset) = repository::fetch_asset(&path)
        {
            let mut response = asset.content.into_response();
            response.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                axum::http::HeaderValue::from_str(&asset.mimetype).unwrap_or(
                    axum::http::HeaderValue::from_static("application/octet-stream"),
                ),
            );
            return response;
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
            return handle_stream_request(path).await;
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

        info!(
            "[{}] Executing handler '{}' from script '{}' for {} {}",
            request_id, handler_name, owner_uri, request_method, path
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
        let worker = move || -> Result<(u16, String, Option<String>), String> {
            js_engine::execute_script_for_request(
                &owner_uri_cl,
                &handler_cl,
                &path_clone,
                &request_method,
                Some(&query_params),
                Some(&form_data),
                raw_body,
            )
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
            Ok(Ok((status, body, content_type))) => {
                info!(
                    "[{}] ✅ Successfully executed handler '{}' - status: {}, body_length: {} bytes",
                    request_id,
                    handler_name,
                    status,
                    body.len()
                );
                let mut response = (
                    StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                    body,
                )
                    .into_response();

                if let Some(ct) = content_type {
                    response.headers_mut().insert(
                        axum::http::header::CONTENT_TYPE,
                        axum::http::HeaderValue::from_str(&ct)
                            .unwrap_or_else(|_| axum::http::HeaderValue::from_static("text/plain")),
                    );
                }

                response
            }
            Ok(Err(e)) => {
                error!(
                    "[{}] ❌ Script execution error for {} {}: {} (handler: {}, script: {})",
                    request_id, method_log, path_log, e, handler_name, owner_uri
                );
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
    let mut app = Router::new()
        // GraphQL endpoints
        .route("/graphql", axum::routing::get(graphql_get))
        .route(
            "/graphql",
            axum::routing::post(move |req| graphql_post(schema_for_post, req)),
        )
        .route(
            "/graphql/sse",
            axum::routing::post(move |req| graphql_sse(schema_for_sse, req)),
        );

    // Mount authentication routes if auth is enabled
    if let Some(ref auth_mgr) = auth_manager {
        info!("Mounting authentication routes at /auth");
        let auth_router = auth::create_auth_router(Arc::clone(auth_mgr));
        app = app.nest("/auth", auth_router);

        // Add optional auth middleware to all routes
        let auth_mgr_for_middleware = Arc::clone(auth_mgr);
        app = app.layer(axum::middleware::from_fn_with_state(
            auth_mgr_for_middleware,
            auth::optional_auth_middleware,
        ));
    } else {
        info!("Authentication disabled - no auth routes mounted");
    }

    // Add catch-all dynamic routes
    app = app
        .route(
            "/",
            any(move |req: Request<Body>| async move {
                handle_dynamic_request(req, script_timeout_ms).await
            }),
        )
        .route(
            "/{*path}",
            any(move |req: Request<Body>| async move {
                handle_dynamic_request(req, script_timeout_ms).await
            }),
        )
        .layer(axum::middleware::from_fn(middleware::request_id_middleware));

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
                    repository::insert_log_message("server", "server started");
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
                repository::insert_log_message("server", "server started");
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
