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
pub mod bytecode;
pub mod config;
pub mod conversion;
pub mod database;
pub mod db_schema_utils;
pub mod deployments;
pub mod dispatcher;
pub mod engine_api;
pub mod error;
pub mod execution_slots;
pub mod graphql;
pub mod graphql_schema_gen;
pub mod graphql_ws;
pub mod hosts;
pub mod http_client;
pub mod js_engine;
pub mod log_retention;
pub mod mcp;
pub mod mcp_client;
pub mod middleware;
pub mod module_loader;
pub mod notifications;
pub mod openapi_schemas;
pub mod parsers;
pub mod repository;
pub mod revisions;
pub mod route_index;
pub mod safe_helpers;
pub mod scheduler;
pub mod script_check;
pub mod script_eval;
pub mod script_init;
pub mod script_test;
pub mod security;
pub mod source_view;
pub mod sql_dialect;
pub mod stream_manager;
pub mod stream_registry;
pub mod transpiler;
pub mod user_repository;
pub mod worker_census;

// Authentication module (Phase 1 - Core Infrastructure)
pub mod auth;

use repository::Repository;
use security::UserContext;

// Re-export the unified error type
pub use error::{AppError, AppResult};

// OpenAPI documentation setup
use utoipa::Modify;
use utoipa::OpenApi;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, OAuth2, SecurityScheme};

/// OpenAPI documentation for all Rust-implemented endpoints
#[derive(OpenApi)]
#[openapi(
    paths(
        health_handler,
        engine_api::cluster_health_route,
        engine_api::upsert_script_route,
        engine_api::delete_script_route,
        engine_api::read_script_route,
        engine_api::script_logs_route,
        engine_api::script_logs_delete_route,
        engine_api::script_logs_stream_route,
        engine_api::routes_route,
        engine_api::revisions_route,
        engine_api::revert_route,
        engine_api::revision_label_route,
        engine_api::revision_diff_route,
        engine_api::deploy_route,
        engine_api::undeploy_route,
        engine_api::deployment_route,
        engine_api::run_tests_route,
        engine_api::check_route,
        engine_api::eval_route,
        engine_api::assets_get_route,
        engine_api::assets_post_route,
        engine_api::assets_patch_route,
        engine_api::assets_batch_route,
        engine_api::assets_delete_route,
        engine_api::list_scripts_route,
        engine_api::script_init_status_route,
        engine_api::script_hosts_get_route,
        engine_api::script_hosts_post_route,
        engine_api::script_hosts_delete_route,
        engine_api::script_owners_get_route,
        engine_api::script_owners_post_route,
        engine_api::script_owners_delete_route,
        engine_api::secrets_get_route,
        engine_api::secrets_post_route,
        engine_api::secrets_delete_route,
        engine_api::users_get_route,
        engine_api::user_roles_post_route,
        engine_api::user_roles_delete_route,
        engine_api::user_realm_post_route,
        engine_api::installed_page_route,
        engine_api::openapi_route,
        engine_api::unauthorized_page_route,
        engine_api::favicon_route,
        auth::routes::login_page,
        auth::routes::start_login,
        auth::routes::oauth_callback,
        auth::routes::logout,
        auth::routes::auth_status,
        auth::routes::refresh_session,
        auth::routes::oauth2_authorize,
        auth::routes::oauth2_token,
        auth::metadata::metadata_handler,
        auth::metadata::protected_resource_metadata_handler,
        auth::client_registration::register_client_handler,
    ),
    components(
        schemas(
            openapi_schemas::HealthResponse,
            openapi_schemas::ClusterHealthResponse,
            openapi_schemas::DatabaseStatus,
            openapi_schemas::ScriptStatus,
            openapi_schemas::SystemInfo,
            openapi_schemas::GraphQLRequest,
            openapi_schemas::GraphQLResponse,
            openapi_schemas::GraphQLError,
            openapi_schemas::McpRpcRequest,
            openapi_schemas::McpRpcResponse,
            openapi_schemas::McpRpcError,
            openapi_schemas::ToolDescriptor,
            openapi_schemas::McpToolsListResponse,
            openapi_schemas::McpToolsList,
            openapi_schemas::OAuth2TokenResponse,
            openapi_schemas::AuthStatusResponse,
            openapi_schemas::ErrorResponse,
            openapi_schemas::ValidationErrorResponse,
            openapi_schemas::UnauthorizedErrorResponse,
            auth::routes::AuthorizeParams,
            auth::routes::TokenParams,
            auth::metadata::AuthorizationServerMetadata,
            auth::metadata::ProtectedResourceMetadata,
            auth::client_registration::ClientRegistrationRequest,
            auth::client_registration::ClientRegistrationResponse,
            auth::client_registration::RegisteredClientMetadata,
        )
    ),
    modifiers(&SecurityAddon),
    tags(
        (name = "Health", description = "Health check and monitoring endpoints"),
        (name = "GraphQL", description = "GraphQL API endpoints for queries, mutations, and subscriptions"),
        (name = "MCP", description = "Model Context Protocol (JSON-RPC 2.0) endpoints for AI tool integration"),
        (name = "Authentication", description = "OAuth2 authentication and authorization endpoints"),
        (name = "Assets", description = "Static assets served from the asset registry"),
        (name = "Streams", description = "Server-Sent Events (SSE) streams registered by scripts"),
        (name = "Users", description = "User directory and role administration (administrators only)"),
    )
)]
struct ApiDoc;

/// Security scheme definitions for OAuth2 and Bearer token authentication
struct SecurityAddon;

impl Modify for SecurityAddon {
    fn modify(&self, openapi: &mut utoipa::openapi::OpenApi) {
        use utoipa::openapi::security::{AuthorizationCode, Flow, Scopes};

        if let Some(components) = openapi.components.as_mut() {
            // OAuth2 Authorization Code Flow with PKCE
            let auth_code_flow = AuthorizationCode::new(
                "/auth/oauth2/authorize",
                "/auth/oauth2/token",
                Scopes::new(),
            );

            let oauth2 = OAuth2::new([Flow::AuthorizationCode(auth_code_flow)]);

            components.add_security_scheme("oauth2", SecurityScheme::OAuth2(oauth2));

            // Bearer token authentication (for direct access token usage)
            components.add_security_scheme(
                "bearerAuth",
                SecurityScheme::Http(
                    HttpBuilder::new()
                        .scheme(HttpAuthScheme::Bearer)
                        .bearer_format("JWT")
                        .description(Some("OAuth2 access token"))
                        .build(),
                ),
            );
        }
    }
}

/// Returns the Rust-generated OpenAPI specification as JSON string
/// This will be merged with JavaScript-generated routes in the runtime
pub fn get_rust_openapi_spec() -> String {
    lazy_static::lazy_static! {
        static ref OPENAPI_SPEC: String = {
            let openapi = ApiDoc::openapi();

            // Serialize to JSON Value for easier manipulation
            let mut spec_value = serde_json::to_value(&openapi)
                .unwrap_or_else(|e| {
                    error!("Failed to serialize OpenAPI to JSON: {}", e);
                    serde_json::json!({})
                });

            // Add manual path definitions that can't be annotated (closures)
            if let Some(paths) = spec_value["paths"].as_object_mut() {
                // GraphQL POST endpoint
                paths.insert("/graphql".to_string(), serde_json::json!({
                    "get": {
                        "tags": ["GraphQL"],
                        "summary": "Execute GraphQL query via GET",
                        "description": "HTTP GET endpoint for executing read-only GraphQL queries using query parameters.",
                        "parameters": [
                            {
                                "name": "query",
                                "in": "query",
                                "required": true,
                                "schema": {"type": "string"},
                                "description": "GraphQL query string"
                            },
                            {
                                "name": "variables",
                                "in": "query",
                                "schema": {"type": "string"},
                                "description": "JSON-encoded variables"
                            },
                            {
                                "name": "operationName",
                                "in": "query",
                                "schema": {"type": "string"},
                                "description": "Operation name if query contains multiple operations"
                            }
                        ],
                        "responses": {
                            "200": {
                                "description": "GraphQL response",
                                "content": {
                                    "application/json": {
                                        "schema": {"$ref": "#/components/schemas/GraphQLResponse"}
                                    }
                                }
                            },
                            "401": {
                                "description": "Authentication required",
                                "content": {
                                    "application/json": {
                                        "schema": {"$ref": "#/components/schemas/UnauthorizedErrorResponse"}
                                    }
                                }
                            }
                        },
                        "security": [{"oauth2": []}]
                    },
                    "post": {
                        "tags": ["GraphQL"],
                        "summary": "Execute GraphQL query or mutation",
                        "description": "HTTP endpoint for executing GraphQL queries and mutations. Use /graphql/ws for subscriptions via WebSocket or /graphql/sse for subscriptions via Server-Sent Events.",
                        "requestBody": {
                            "required": true,
                            "content": {
                                "application/json": {
                                    "schema": {"$ref": "#/components/schemas/GraphQLRequest"}
                                }
                            }
                        },
                        "responses": {
                            "200": {
                                "description": "GraphQL response with data or errors",
                                "content": {
                                    "application/json": {
                                        "schema": {"$ref": "#/components/schemas/GraphQLResponse"}
                                    }
                                }
                            },
                            "400": {
                                "description": "Invalid request",
                                "content": {
                                    "application/json": {
                                        "schema": {"$ref": "#/components/schemas/ValidationErrorResponse"}
                                    }
                                }
                            },
                            "401": {
                                "description": "Authentication required",
                                "content": {
                                    "application/json": {
                                        "schema": {"$ref": "#/components/schemas/UnauthorizedErrorResponse"}
                                    }
                                }
                            }
                        },
                        "security": [{"oauth2": []}]
                    }
                }));

                // GraphQL WebSocket endpoint
                paths.insert("/graphql/ws".to_string(), serde_json::json!({
                    "get": {
                        "tags": ["GraphQL"],
                        "summary": "GraphQL WebSocket subscriptions",
                        "description": "WebSocket endpoint for GraphQL subscriptions using the graphql-ws protocol. Allows real-time data streaming.",
                        "x-protocol": "graphql-ws",
                        "x-transport": "websocket",
                        "responses": {
                            "101": {
                                "description": "Switching Protocols - WebSocket connection established"
                            },
                            "401": {
                                "description": "Authentication required"
                            }
                        },
                        "security": [{"oauth2": []}]
                    }
                }));

                // GraphQL SSE endpoint
                paths.insert("/graphql/sse".to_string(), serde_json::json!({
                    "get": {
                        "tags": ["GraphQL"],
                        "summary": "GraphQL Server-Sent Events subscriptions",
                        "description": "Server-Sent Events endpoint for GraphQL subscriptions. Allows real-time data streaming over HTTP.",
                        "x-protocol": "text/event-stream",
                        "x-transport": "sse",
                        "parameters": [
                            {
                                "name": "query",
                                "in": "query",
                                "required": true,
                                "schema": {"type": "string"},
                                "description": "GraphQL subscription query"
                            }
                        ],
                        "responses": {
                            "200": {
                                "description": "Event stream with GraphQL subscription data",
                                "headers": {
                                    "Content-Type": {
                                        "schema": {"type": "string"}
                                    }
                                }
                            },
                            "401": {
                                "description": "Authentication required"
                            }
                        },
                        "security": [{"oauth2": []}]
                    }
                }));

                // The log tail is annotated on its handler like every other
                // engine endpoint; what the annotation cannot say is that the
                // response is an event stream rather than a single document.
                if let Some(operation) = paths
                    .get_mut("/engine/script_logs/stream")
                    .and_then(|path| path.get_mut("get"))
                    .and_then(|get| get.as_object_mut())
                {
                    operation.insert("x-protocol".to_string(), "text/event-stream".into());
                    operation.insert("x-transport".to_string(), "sse".into());
                }

                // MCP endpoint
                paths.insert("/mcp".to_string(), serde_json::json!({
                    "post": {
                        "tags": ["MCP"],
                        "summary": "Model Context Protocol endpoint",
                        "description": "JSON-RPC 2.0 endpoint implementing the Model Context Protocol for AI tool integration. Supports methods: initialize, notifications/initialized, tools/list, tools/call, prompts/list, prompts/get, completion/complete.",
                        "requestBody": {
                            "required": true,
                            "content": {
                                "application/json": {
                                    "schema": {"$ref": "#/components/schemas/McpRpcRequest"}
                                }
                            }
                        },
                        "responses": {
                            "200": {
                                "description": "JSON-RPC response",
                                "content": {
                                    "application/json": {
                                        "schema": {"$ref": "#/components/schemas/McpRpcResponse"}
                                    }
                                }
                            },
                            "400": {
                                "description": "Invalid JSON-RPC request",
                                "content": {
                                    "application/json": {
                                        "schema": {"$ref": "#/components/schemas/ValidationErrorResponse"}
                                    }
                                }
                            },
                            "401": {
                                "description": "Authentication required (Bearer token)",
                                "content": {
                                    "application/json": {
                                        "schema": {"$ref": "#/components/schemas/UnauthorizedErrorResponse"}
                                    }
                                }
                            }
                        },
                        "security": [{"bearerAuth": []}]
                    }
                }));

                // TypeScript type definitions
                let version = env!("CARGO_PKG_VERSION");
                let type_defs_path = format!("/engine/types/v{}/aiwebengine.d.ts", version);
                paths.insert(type_defs_path.clone(), serde_json::json!({
                    "get": {
                        "tags": ["Documentation"],
                        "summary": "TypeScript type definitions",
                        "description": "TypeScript type definitions for the AIWebEngine public API",
                        "responses": {
                            "200": {
                                "description": "TypeScript type definitions file",
                                "content": {
                                    "text/plain": {}
                                }
                            },
                            "404": {
                                "description": "Type definitions not found"
                            }
                        }
                    }
                }));

                // Documentation routes are now handled by docs.js feature script
                // OpenAPI spec is automatically generated from route registrations
                // So these static entries are no longer needed:
                // - /engine/docs (redirect)
                // - /engine/docs/ (main page)
                // - /engine/docs/* (wildcard)
            }

            serde_json::to_string_pretty(&spec_value)
                .unwrap_or_else(|e| {
                    error!("Failed to serialize OpenAPI spec: {}", e);
                    "{}".to_string()
                })
        };
    }

    OPENAPI_SPEC.clone()
}

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

fn create_js_auth_context_from_session(session: Option<&auth::AuthSession>) -> auth::JsAuthContext {
    match session {
        Some(session) => auth::JsAuthContext::authenticated(
            session.user_id.clone(),
            session.email.clone(),
            session.name.clone(),
            session.provider.clone(),
            session.is_admin,
            session.is_editor,
        ),
        None => auth::JsAuthContext::anonymous(),
    }
}

fn create_user_context_from_session(session: Option<&auth::AuthSession>) -> security::UserContext {
    match session {
        Some(session) if session.is_admin => security::UserContext::admin(session.user_id.clone()),
        Some(session) if session.is_editor => {
            security::UserContext::editor(session.user_id.clone())
        }
        Some(session) => security::UserContext::authenticated(session.user_id.clone()),
        None => security::UserContext::anonymous(),
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

/// Read the host a request was addressed to, for host-scoped routing.
fn request_host(headers: &axum::http::HeaderMap) -> Option<String> {
    headers
        .get(axum::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(|host| host.trim().to_lowercase())
        .filter(|host| !host.is_empty())
}

/// Refuse management endpoints on hosts that are not allowed to serve them.
///
/// Answers 404 rather than 403: on a host where these endpoints are not
/// offered, "not found" is the truthful answer and it does not confirm to a
/// script's page that the management API exists elsewhere.
async fn management_host_guard(
    req: Request<Body>,
    next: axum::middleware::Next,
) -> axum::response::Response {
    let host = request_host(req.headers());
    if !engine_api::is_management_host(host.as_deref()) {
        let path = req.uri().path().to_string();
        let request_id = req
            .extensions()
            .get::<middleware::RequestId>()
            .map(|rid| rid.0.clone())
            .unwrap_or_else(|| "unknown".to_string());
        warn!(
            "[{}] Management endpoint {} refused on host {:?}: not in server.management_hosts",
            request_id,
            path,
            host.as_deref().unwrap_or("<no Host header>")
        );
        return error_to_response(error::errors::not_found(&path, &request_id));
    }
    next.run(req).await
}

/// Point a provider's configured redirect URI at a different base URL, keeping
/// its path and query so each host uses the same callback route.
///
/// Returns `None` when either URL fails to parse; the caller then leaves that
/// host without a dedicated provider instead of guessing a redirect URI.
fn redirect_uri_for_base(redirect_uri: &str, base_url: &str) -> Option<String> {
    let source = url::Url::parse(redirect_uri).ok()?;
    let mut target = url::Url::parse(base_url).ok()?;
    target.set_path(source.path());
    target.set_query(source.query());
    target.set_fragment(None);
    Some(target.to_string())
}

/// Helper: Register an OAuth2 provider with common configuration pattern
///
/// Registers the host-independent instance built from the configured redirect
/// URI, then one instance per entry in `additional_base_urls` so a login
/// started on those hosts returns there instead of to the base URL. Every
/// derived redirect URI has to be registered with the provider as well, so
/// each is logged at startup.
fn register_oauth_provider(
    auth_manager: &mut auth::AuthManager,
    provider_name: &str,
    config: OAuthProviderConfig,
    additional_base_urls: &[String],
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

    for base_url in additional_base_urls {
        let Some(host) = config::base_url_authority(base_url) else {
            warn!(
                "Skipping {} provider for '{}': not a URL with a host",
                provider_name, base_url
            );
            continue;
        };
        let Some(redirect_uri) = redirect_uri_for_base(&oauth_config.redirect_uri, base_url) else {
            warn!(
                "Skipping {} provider for '{}': could not derive a redirect URI from '{}'",
                provider_name, base_url, oauth_config.redirect_uri
            );
            continue;
        };
        info!(
            "Registering {} OAuth2 provider for host '{}' with redirect URI {} \
             (must be registered with the provider)",
            provider_name, host, redirect_uri
        );
        let host_config = auth::OAuth2ProviderConfig {
            redirect_uri,
            ..oauth_config.clone()
        };
        auth_manager.register_provider_for_host(&host, provider_name, host_config)?;
    }

    auth_manager.register_provider(provider_name, oauth_config)
}

/// Helper: Initialize database and repository
async fn initialize_database_and_repository(config: &config::Config) -> AppResult<()> {
    info!("Initializing database connection...");
    info!("Initializing PostgreSQL repository");

    // Log connection string (sanitized)
    let safe_conn_str = sanitize_connection_string(&config.repository.connection_string);
    info!("Repository config - database_url: {}", safe_conn_str);

    // Initialize database
    match database::init_database(&config.repository, true).await {
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

            // Initialize PostgresRepository with pool and server_id
            let repo = repository::PostgresRepository::new(db_arc.pool().clone(), server_id);
            if repository::initialize_repository(repo) {
                info!("Global repository initialized with PostgreSQL");
            } else {
                warn!("Global repository was already initialized");
            }
        }
        Err(e) => {
            // Strict failure: Do not fallback if database fails
            return Err(AppError::Database {
                message: format!("Database initialization failed: {}. Fatal error.", e),
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
                Ok::<Event, std::convert::Infallible>(Event::default().data(
                    serde_json::json!({ "error": format!("Stream error: {}", e) }).to_string(),
                ))
            }
        }
    });

    // Create SSE response
    let sse = Sse::new(sse_stream).keep_alive(axum::response::sse::KeepAlive::default());

    // Return the SSE response
    sse.into_response()
}

/// Initialize authentication manager with all dependencies
async fn initialize_auth_manager(
    auth_config: auth::AuthConfig,
    server_config: &config::ServerConfig,
    security_config: &config::SecurityConfig,
    pool: sqlx::PgPool,
) -> Result<Arc<auth::AuthManager>, auth::AuthError> {
    use auth::{AuthManager, AuthManagerConfig, AuthSecurityContext, AuthSessionManager};
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

    // Load secret encryption key from configuration (base64-encoded 32 bytes)
    // Used for encrypting secret values stored in the database at rest.
    match &security_config.secret_encryption_key {
        Some(s) if !s.is_empty() => {
            let decoded = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, s)
                .map_err(|e| auth::AuthError::InvalidConfig {
                    key: "security.secret_encryption_key".to_string(),
                    reason: format!("base64 decode failed: {}", e),
                })?;
            if decoded.len() != 32 {
                return Err(auth::AuthError::InvalidConfig {
                    key: "security.secret_encryption_key".to_string(),
                    reason: "expected 32 bytes after base64 decoding".to_string(),
                });
            }
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&decoded);
            repository::initialize_secret_encryption(Arc::new(DataEncryption::new(&arr)));
            info!("Secret encryption key loaded — secrets will be encrypted at rest.");
        }
        _ => {
            warn!(
                "security.secret_encryption_key not configured. Secrets will be stored as plaintext in the database."
            );
        }
    }

    // Create secure session manager
    let session_manager = Arc::new(
        SecureSessionManager::new(
            pool.clone(),
            &encryption_key,
            auth_config.session_timeout as i64,
            auth_config.max_session_age as i64,
            auth_config.max_concurrent_sessions,
            Arc::clone(&auditor),
        )?
        .with_strict_ip_validation(security_config.strict_ip_validation),
    );

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
        session_cookie_name: auth::host_scoped_cookie_name(
            &auth_config.cookie.name,
            auth_config.cookie.secure,
        ),
        cookie_secure: auth_config.cookie.secure,
        session_timeout: auth_config.session_timeout,
        max_session_age: auth_config.max_session_age,
        internal: auth_config.internal.clone(),
    };

    // Create auth manager
    let mut auth_manager = AuthManager::new(
        manager_config,
        auth_session_manager,
        security_context,
        security_config.api_key.clone(),
    );

    // Extra hostnames this engine answers to; each gets its own provider
    // instance so a login there completes there.
    let additional_base_urls = server_config.additional_base_urls.clone();

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
            &additional_base_urls,
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
            &additional_base_urls,
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
            &additional_base_urls,
        )?;
    }

    Ok(Arc::new(auth_manager))
}

/// Initialize all core components (database, scripts, assets)
async fn initialize_components(config: &config::Config) -> AppResult<()> {
    // Initialize database connection and repository
    initialize_database_and_repository(config).await?;

    // Start PostgreSQL notification listener for script synchronization
    if let Some(db) = database::get_global_database() {
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
    if let Err(e) = repository::bootstrap_scripts_async().await {
        warn!(
            "Failed to bootstrap scripts: {}. Continuing with static scripts.",
            e
        );
    }

    // Register engine-provided streams (e.g. /engine/script_updates) before any
    // script or connection can reference them
    engine_api::register_engine_streams();

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

    // Give scripts that predate their own history something to return to,
    // before init() runs so its outcome has a revision to attach itself to.
    // One pass over the scripts that have no revisions at all, rather than a
    // question asked once per script on every boot.
    revisions::backfill_missing(None).await;
    // Which revision each script is at, so lines logged before this boot's
    // first write to a script are still attributed to a version.
    revisions::load_current().await;

    // Which revision each script *serves*, before any of them is executed. The
    // source cache is filled from the `scripts` table, which is head — a
    // pinned script would otherwise run head's code until something happened
    // to refresh it, which is the whole of what pinning promises not to do.
    deployments::load_pins().await;
    for uri in scripts.keys() {
        if deployments::pinned(uri).is_some() {
            repository::refresh_served_source(uri).await;
        }
    }

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
                .insert_log(uri, &error_msg, "FATAL", &repository::LogContext::default())
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
async fn initialize_script_functions(_config: &config::Config) -> AppResult<()> {
    info!("Initializing all scripts...");
    let initializer = script_init::ScriptInitializer::with_configured_timeout();
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

/// Grant a role to an account from the command line, with no server running.
///
/// Break-glass, and the second half of giving a personal install an owner:
/// `auth.internal.bootstrap_admin_usernames` covers the ordinary case, and this
/// covers the ones it cannot — an account created under a different name than
/// the config expects, an administrator who has locked themselves out, a
/// federated deployment whose only administrator left. Every other road to the
/// administrator tier is guarded by an administrator, so an engine with none
/// has no way back in short of editing the database by hand.
///
/// Authorized by holding the configuration file and the database it points at,
/// which is the same authority `bootstrap_admins` already runs on.
pub async fn grant_role_command(
    config: &config::Config,
    account: &str,
    role: &str,
) -> AppResult<String> {
    let parsed = match role {
        "administrator" | "Administrator" => user_repository::UserRole::Administrator,
        "editor" | "Editor" => user_repository::UserRole::Editor,
        other => {
            return Err(AppError::config(format!(
                "Unknown role '{}'. Use 'administrator' or 'editor'.",
                other
            )));
        }
    };

    let db = database::init_database(&config.repository, true).await?;
    database::initialize_global_database(Arc::new(db));

    // A local username first, because that is what a personal install has; an
    // address second, for a deployment whose accounts came from a provider.
    let user = match user_repository::find_user_by_provider(
        auth::local::LOCAL_PROVIDER,
        &auth::local::normalize_username(account),
    )? {
        Some(user) => user,
        None => user_repository::find_user_by_email(account)
            .await?
            .ok_or_else(|| {
                AppError::config(format!(
                    "No account named '{}'. Sign in once to create it, then run this again.",
                    account
                ))
            })?,
    };

    user_repository::add_user_role(&user.id, parsed.clone())?;

    // An administrator scoped to one host cannot administer the engine: the
    // management host refuses the session, and the endpoint that would widen
    // the realm is served only on the host that can no longer be reached. This
    // command exists to be the way back in, so it makes the account a principal
    // everywhere — the same thing `bootstrap_admins` does, by the same
    // authority. An editor is an author of solutions and stays where they are.
    let realm_note = if matches!(parsed, user_repository::UserRole::Administrator)
        && user.realm != user_repository::GLOBAL_REALM
    {
        user_repository::set_user_realm(&user.id, user_repository::GLOBAL_REALM)?;
        " They are now a principal on every host this engine serves."
    } else {
        ""
    };

    Ok(format!(
        "{} ({}) now holds the {} role.{} Any session they held has been ended, \
         so the change takes effect at their next sign-in.",
        account, user.id, role, realm_note
    ))
}

/// Starts the web server with custom configuration
pub async fn start_server_with_config(
    config: config::Config,
    shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) -> AppResult<u16> {
    // Apply configured JavaScript limits to every execution path (memory limit,
    // stack size, script size, and the interrupt-handler deadline) before any
    // script runs.
    let js_limits = js_engine::ExecutionLimits {
        timeout_ms: config.javascript.execution_timeout_ms,
        max_memory_mb: (config.javascript.max_memory_bytes / (1024 * 1024)).max(1),
        // `ExecutionLimits` has carried this field and checked it at
        // `js_engine::validate_script_size` all along; what it never had was
        // the configured value, so `repository.max_script_size_bytes` set a
        // limit nothing consulted.
        max_script_size_bytes: config.repository.max_script_size_bytes,
        stack_size_bytes: config.javascript.stack_size_bytes,
    };
    if !js_engine::configure_execution_limits(js_limits) {
        debug!("JavaScript execution limits were already configured");
    }

    // How many of those runtimes may exist at once. Without this the ceiling
    // is Tokio's default blocking pool rather than anything configured.
    if !execution_slots::configure(config.javascript.max_concurrent_executions) {
        debug!("JavaScript execution slots were already configured");
    }

    // The init() budget every re-initialization path uses: startup, a local
    // upsert, and a peer instance's upsert notification.
    let init_timeout_ms = config
        .javascript
        .init_timeout_ms
        .unwrap_or(config.javascript.execution_timeout_ms);
    if !script_init::configure_init_timeout(init_timeout_ms) {
        debug!("Script init timeout was already configured");
    }

    // Recorded so the engine API reports the same retention the background
    // pruner enforces.
    if !log_retention::configure(log_retention::LogRetention::from_config(&config.logs)) {
        debug!("Log retention was already configured");
    }

    // Test budgets: one per test module, one for a whole run.
    let test_timeout_ms = config
        .javascript
        .test_timeout_ms
        .unwrap_or(config.javascript.execution_timeout_ms);
    let test_run_timeout_ms = config
        .javascript
        .test_run_timeout_ms
        .unwrap_or(script_test::DEFAULT_TEST_RUN_TIMEOUT_MS);
    if !script_test::configure_test_timeouts(test_timeout_ms, test_run_timeout_ms) {
        debug!("Script test timeouts were already configured");
    }

    // Resolve the hosts this engine serves before anything reads them: script
    // host bindings, route indexing and the management guard all depend on it.
    let host_config = hosts::HostConfig::new(
        &config.server.get_base_url(),
        &config.server.additional_base_urls,
    );
    info!(
        "Serving hosts: {:?} (scripts publish on {:?} unless bound otherwise)",
        host_config.all_hosts(),
        host_config.default_host()
    );
    hosts::init(host_config);

    // Scope the management APIs to their hosts before the server can serve a
    // request. Logged either way: which hosts answer /engine/* is exactly the
    // kind of thing that should be visible in the startup output.
    let management_hosts = config.server.normalized_management_hosts();
    if management_hosts.is_empty() {
        warn!(
            "server.management_hosts is not set — the /engine management APIs are served on \
             every host. Set it to restrict them once scripts serve content on a host that \
             should not reach them from an administrator's browser."
        );
    } else {
        info!(
            "Management APIs (/engine/*, except the static /engine/installed page) restricted \
             to hosts: {:?}",
            management_hosts
        );
    }
    engine_api::init_management_hosts(management_hosts);

    // Initialize all core components
    initialize_components(&config).await?;

    // Fan out shutdown notifications so both the HTTP server and scheduler worker can stop cleanly
    let (server_shutdown_tx, server_shutdown_rx) = tokio::sync::oneshot::channel();
    let (scheduler_shutdown_tx, scheduler_shutdown_rx) = tokio::sync::oneshot::channel();
    let (pruner_shutdown_tx, pruner_shutdown_rx) = tokio::sync::oneshot::channel();
    let (log_pruner_shutdown_tx, log_pruner_shutdown_rx) = tokio::sync::oneshot::channel();

    scheduler::spawn_worker(scheduler_shutdown_rx);
    revisions::spawn_pruner(config.revisions.clone(), pruner_shutdown_rx);
    log_retention::spawn_pruner(config.logs.clone(), log_pruner_shutdown_rx);

    tokio::spawn(async move {
        let _ = shutdown_rx.await;
        let _ = scheduler_shutdown_tx.send(());
        let _ = pruner_shutdown_tx.send(());
        let _ = log_pruner_shutdown_tx.send(());
        let _ = server_shutdown_tx.send(());
    });

    // Clone the timeout value to avoid borrow checker issues in async closures
    let script_timeout_ms = config.javascript.execution_timeout_ms;

    // Get database pool if available
    let pool = database::get_global_database().map(|db| db.pool().clone());

    // Initialize authentication if configured and enabled
    let auth_manager = initialize_auth_if_enabled(&config, pool.clone()).await?;

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
    repository::insert_log_message_async("server", "server started", "INFO").await;
    debug!(
        "Server configuration - host: {}, requested port: {}, actual port: {}",
        config.server.host, config.server.port, actual_port
    );

    start_server_instance(app, actual_addr, server_shutdown_rx);

    Ok(actual_port)
}

/// Health check endpoint - returns basic instance status
///
/// Verifies the database dependency with a lightweight `SELECT 1` round-trip so
/// that load balancers and container health checks are pulled from rotation when
/// Postgres is unreachable. Returns 503 when the database check fails.
#[utoipa::path(
    get,
    path = "/health",
    tags = ["Health"],
    responses(
        (status = 200, description = "Service is healthy", body = crate::openapi_schemas::HealthResponse),
        (status = 503, description = "Service is unhealthy (database unreachable)", body = crate::openapi_schemas::HealthResponse),
    )
)]
async fn health_handler() -> impl IntoResponse {
    let server_id = notifications::get_server_id().unwrap_or_else(|| "unknown".to_string());

    // Verify the database dependency with a real query, not just process liveness.
    let (healthy, database) = match database::get_global_database() {
        Some(db) => match db.health_check().await {
            Ok(()) => (true, "ok".to_string()),
            Err(e) => (false, format!("error: {}", e)),
        },
        None => (false, "not configured".to_string()),
    };

    let status_code = if healthy {
        axum::http::StatusCode::OK
    } else {
        axum::http::StatusCode::SERVICE_UNAVAILABLE
    };

    let body = axum::response::Json(serde_json::json!({
        "status": if healthy { "healthy" } else { "unhealthy" },
        "instance_id": server_id,
        "timestamp": chrono::Utc::now().to_rfc3339(),
        "version": {
            "cargo": env!("CARGO_PKG_VERSION"),
            "git_commit": option_env!("VERGEN_GIT_SHA").unwrap_or(""),
            "git_commit_timestamp": option_env!("VERGEN_GIT_COMMIT_TIMESTAMP").unwrap_or(""),
            "build_timestamp": option_env!("VERGEN_BUILD_TIMESTAMP").unwrap_or("")
        },
        "database": database
    }));

    (status_code, body)
}

/// Initialize authentication manager if configured and enabled.
///
/// Fails startup when authentication is explicitly enabled but cannot be
/// initialized: silently continuing without auth would leave every endpoint
/// open that the operator expects to be protected.
async fn initialize_auth_if_enabled(
    config: &config::Config,
    pool: Option<sqlx::PgPool>,
) -> AppResult<Option<Arc<auth::AuthManager>>> {
    if let Some(auth_config) = config.auth.clone()
        && auth_config.enabled
    {
        info!("Authentication is enabled, initializing AuthManager...");

        let pool = pool.ok_or_else(|| {
            AppError::config(
                "Authentication is enabled but the database is not initialized; \
                 refusing to start without authentication",
            )
        })?;

        debug!(
            "Auth config: enabled={}, providers={:?}",
            auth_config.enabled,
            auth_config.providers.enabled_providers()
        );

        // The same declaration for an engine whose accounts have no verified
        // address — a personal install's only way to a first administrator.
        if !auth_config.internal.bootstrap_admin_usernames.is_empty() {
            info!(
                "Configuring {} bootstrap admin username(s): {:?}",
                auth_config.internal.bootstrap_admin_usernames.len(),
                auth_config.internal.bootstrap_admin_usernames
            );
        }
        user_repository::set_bootstrap_admin_usernames(
            auth_config.internal.bootstrap_admin_usernames.clone(),
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
                Ok(Some(manager))
            }
            Err(e) => Err(AppError::config(format!(
                "Failed to initialize AuthManager: {}. Authentication is enabled in \
                 configuration, so the server refuses to start without it.",
                e
            ))),
        }
    } else {
        info!(
            "Authentication disabled: config.auth.is_some()={}, config.auth.enabled={}",
            config.auth.is_some(),
            config.auth.as_ref().map(|c| c.enabled).unwrap_or(false)
        );
        Ok(None)
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
    // Cap on request body reads; bodies beyond this are rejected instead of
    // being buffered into memory (usize is Copy, so each closure gets its own)
    let max_request_body = config.security.max_request_body_bytes;

    // GraphQL handler - executes queries (supports GET and POST)
    let graphql_post_handler = move |req: axum::http::Request<axum::body::Body>| async move {
        // Extract authentication context before consuming the request
        let auth_user = req.extensions().get::<auth::AuthUser>().cloned();

        let (parts, body) = req.into_parts();
        let method = parts.method.clone();

        let body_bytes = match axum::body::to_bytes(body, max_request_body).await {
            Ok(bytes) => bytes,
            Err(_) => {
                return axum::response::Json(
                    serde_json::json!({"error": "Request body too large or unreadable"}),
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

            let mut req = if let Some(q) = query {
                async_graphql::Request::new(q)
            } else {
                return axum::response::Json(
                    serde_json::json!({"error": "Missing query parameter"}),
                );
            };
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

        // Schema for this host: only operations from scripts published here
        let canonical_host = hosts::canonical_host(request_host(&parts.headers).as_deref());
        let schema = match graphql::get_schema_for_host(&canonical_host).await {
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
            let canonical_host = hosts::canonical_host(request_host(req.headers()).as_deref());

            ws.on_upgrade(move |socket| {
                graphql_ws::handle_websocket_connection(socket, auth_user, canonical_host)
            })
        };

    // GraphQL SSE handler - handles subscriptions over Server-Sent Events using execute_stream
    let graphql_sse_handler = move |req: axum::http::Request<axum::body::Body>| async move {
        // Extract authentication context before consuming the request
        let auth_user = req.extensions().get::<auth::AuthUser>().cloned();

        let (parts, body) = req.into_parts();
        info!("GraphQL SSE request for URI: {}", parts.uri);

        let body_bytes = match axum::body::to_bytes(body, max_request_body).await {
            Ok(bytes) => bytes,
            Err(e) => {
                error!("GraphQL SSE: Failed to read request body: {}", e);
                return axum::response::Response::builder()
                    .status(StatusCode::PAYLOAD_TOO_LARGE)
                    .header("content-type", "text/plain")
                    .body(axum::body::Body::from(
                        "Request body too large or unreadable",
                    ))
                    .unwrap_or_else(|err| {
                        error!("Failed to build error response: {}", err);
                        axum::response::Response::new(axum::body::Body::from("Payload Too Large"))
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

            let mut req = if let Some(q) = query {
                async_graphql::Request::new(q)
            } else {
                error!("GraphQL SSE: Missing query parameter");
                return axum::response::Response::builder()
                    .status(400)
                    .header("content-type", "text/plain")
                    .body(axum::body::Body::from("Missing query parameter"))
                    .unwrap_or_else(|err| {
                        error!("Failed to build error response: {}", err);
                        axum::response::Response::new(axum::body::Body::from("Bad Request"))
                    });
            };
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

        // Schema for this host: only operations from scripts published here
        let canonical_host = hosts::canonical_host(request_host(&parts.headers).as_deref());
        let schema = match graphql::get_schema_for_host(&canonical_host).await {
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
    let mcp_handler = move |req: axum::http::Request<axum::body::Body>| async move {
        let mcp_session = req
            .extensions()
            .get::<auth::McpAuthSession>()
            .map(|auth_session| auth_session.session.clone());
        // Which host's script registrations this client sees
        let raw_host = request_host(req.headers());
        let canonical_host = hosts::canonical_host(raw_host.as_deref());
        // The engine's own tools follow `server.management_hosts`, matched
        // against the Host header itself rather than the canonicalised host —
        // a management host need not be one of the hosts scripts publish on.
        let native_tools_allowed = engine_api::is_management_host(raw_host.as_deref());

        let body_bytes = match axum::body::to_bytes(req.into_body(), max_request_body).await {
            Ok(bytes) => bytes,
            Err(e) => {
                error!("MCP: Failed to read request body: {}", e);
                return axum::response::Json(serde_json::json!({
                    "jsonrpc": "2.0",
                    "error": {
                        "code": -32700,
                        "message": "Parse error: request body too large or unreadable"
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
                let tools = mcp::list_tools_for_host(&canonical_host, native_tools_allowed).await;

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
                let auth_context = mcp_session
                    .as_ref()
                    .map(|session| create_js_auth_context_from_session(Some(session)));
                let user_context = create_user_context_from_session(mcp_session.as_ref());

                // A tool runs JavaScript to completion — a script-registered
                // handler, or the engine's own test runner — which is CPU-bound
                // work with a budget measured in seconds. On the async thread
                // that blocks a worker for the whole run, so it goes to the
                // blocking pool like every other script execution path.
                let tool_name = params.name.clone();

                // Repeat the listing's host filter at dispatch, so naming a
                // tool that is not published here does not reach its script.
                if !mcp::tool_is_available_on_host(
                    &tool_name,
                    &canonical_host,
                    native_tools_allowed,
                )
                .await
                {
                    warn!(
                        "MCP tool '{}' is not published on host {}; refusing the call",
                        tool_name, canonical_host
                    );
                    return axum::response::Json(serde_json::json!({
                        "jsonrpc": "2.0",
                        "id": rpc_request.id,
                        "error": {
                            "code": -32601,
                            "message": format!("Tool not found: {}", tool_name)
                        }
                    }));
                }

                // A tool's own budget is enforced twice over — by the
                // JavaScript interrupt handler between bytecode instructions,
                // and by the host-call budget across a call that has left
                // JavaScript to wait on the database or the network. This
                // backstop covers what neither reaches: a wait inside the
                // engine itself. The blocking thread is still lost when it
                // fires; what it recovers is the response.
                let backstop =
                    std::time::Duration::from_millis(mcp::tool_call_backstop_ms(&tool_name));
                let tool_name_for_error = tool_name.clone();
                let (ticket, watch) = worker_census::watch(format!("mcp tool {}", tool_name));
                let execution = match tokio::time::timeout(backstop, async move {
                    let permit = execution_slots::acquire().await;
                    tokio::task::spawn_blocking(move || {
                        let _permit = permit;
                        let _ticket = ticket;
                        mcp::execute_mcp_tool(&tool_name, arguments, auth_context, user_context)
                    })
                    .await
                })
                .await
                {
                    Ok(joined) => joined.unwrap_or_else(|join_error| {
                        Err(format!("tool task failed: {}", join_error))
                    }),
                    Err(_) => {
                        watch.abandon();
                        error!(
                            "MCP tool '{}' did not answer within {:?}; abandoning the call",
                            tool_name_for_error, backstop
                        );
                        Err(format!(
                            "Tool '{}' did not answer within {}ms and was abandoned. It is \
                            blocked in a host call — a fetch, a database query, an MCP call — \
                            which the engine cannot interrupt, rather than in JavaScript. Look \
                            for an unbounded call: a request with no timeout, or a query without \
                            a limit.",
                            tool_name_for_error,
                            backstop.as_millis()
                        ))
                    }
                };

                match execution {
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
                let prompts = mcp::list_prompts_for_host(&canonical_host).await;

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

                // The caller's own identity, built the same way the tools
                // branch builds it. A prompt handler is script code answering
                // a request, so it runs as whoever made it.
                let auth_context = mcp_session
                    .as_ref()
                    .map(|session| create_js_auth_context_from_session(Some(session)));
                let user_context = create_user_context_from_session(mcp_session.as_ref());

                match mcp::execute_mcp_prompt(&params.name, arguments, auth_context, user_context) {
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

                let auth_context = mcp_session
                    .as_ref()
                    .map(|session| create_js_auth_context_from_session(Some(session)));
                let user_context = create_user_context_from_session(mcp_session.as_ref());

                match mcp::execute_mcp_completion(
                    &prompt_name,
                    &params.argument.name,
                    &params.argument.value,
                    context_arguments,
                    auth_context,
                    user_context,
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
        let metadata_config = Arc::new(auth::MetadataConfig::new(
            &config.server.all_base_urls(),
            true, // enable_registration
            true, // resource_indicators_supported
        ));

        let pool = pool.expect("Database pool required when auth is enabled");

        // Registered clients are stored, because the authorization endpoint
        // checks every `client_id` and `redirect_uri` against them.
        let registration_manager = Arc::new(auth::ClientRegistrationManager::new(
            90, // day secret expiry
            pool.clone(),
        ));

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
            .route(
                "/graphql",
                axum::routing::get(graphql_post_handler).post(graphql_post_handler),
            )
            .route("/graphql/ws", axum::routing::get(graphql_ws_handler))
            .route("/graphql/sse", axum::routing::get(graphql_sse_handler));

        // MCP endpoint without authentication (auth is disabled globally)
        app = app.route("/mcp", axum::routing::post(mcp_handler));
    }

    // Script and asset management endpoints (engine functionality,
    // previously provided by the built-in core.js/cli.js scripts).
    // The configured request body limit applies, same as dynamic routes.
    // These paths live under the reserved /engine prefix, and are served only
    // on the hosts in `server.management_hosts` (see `management_host_guard`).
    let management_router = Router::new()
        .route(
            "/engine/upsert_script",
            axum::routing::post(engine_api::upsert_script_route),
        )
        .route(
            "/engine/delete_script",
            axum::routing::post(engine_api::delete_script_route),
        )
        .route(
            "/engine/read_script",
            axum::routing::get(engine_api::read_script_route),
        )
        .route(
            "/engine/script_logs",
            axum::routing::get(engine_api::script_logs_route)
                .delete(engine_api::script_logs_delete_route),
        )
        .route(
            "/engine/script_logs/stream",
            axum::routing::get(engine_api::script_logs_stream_route),
        )
        .route(
            "/engine/routes",
            axum::routing::get(engine_api::routes_route),
        )
        .route(
            "/engine/revisions",
            axum::routing::get(engine_api::revisions_route),
        )
        .route(
            "/engine/revisions/revert",
            axum::routing::post(engine_api::revert_route),
        )
        .route(
            "/engine/deploy",
            axum::routing::post(engine_api::deploy_route)
                .delete(engine_api::undeploy_route)
                .get(engine_api::deployment_route),
        )
        .route(
            "/engine/revisions/label",
            axum::routing::post(engine_api::revision_label_route),
        )
        .route(
            "/engine/revisions/diff",
            axum::routing::get(engine_api::revision_diff_route),
        )
        .route(
            "/engine/run_tests",
            axum::routing::post(engine_api::run_tests_route),
        )
        .route(
            "/engine/check",
            axum::routing::post(engine_api::check_route),
        )
        .route("/engine/eval", axum::routing::post(engine_api::eval_route))
        .route(
            "/engine/assets",
            axum::routing::get(engine_api::assets_get_route)
                .post(engine_api::assets_post_route)
                .patch(engine_api::assets_patch_route)
                .delete(engine_api::assets_delete_route),
        )
        // A batch carries a script's whole module tree, which the management
        // router's `max_request_body_bytes` (1MB by default) is far too small
        // for. The inner layer wins, so this route gets the ceiling the batch
        // write actually enforces on its content.
        .route(
            "/engine/assets/batch",
            axum::routing::post(engine_api::assets_batch_route).layer(
                axum::extract::DefaultBodyLimit::max(engine_api::MAX_BATCH_BODY_BYTES),
            ),
        )
        .route(
            "/engine/scripts",
            axum::routing::get(engine_api::list_scripts_route),
        )
        .route(
            "/engine/script_init_status",
            axum::routing::get(engine_api::script_init_status_route),
        )
        .route(
            "/engine/script_hosts",
            axum::routing::get(engine_api::script_hosts_get_route)
                .post(engine_api::script_hosts_post_route)
                .delete(engine_api::script_hosts_delete_route),
        )
        .route(
            "/engine/script_owners",
            axum::routing::get(engine_api::script_owners_get_route)
                .post(engine_api::script_owners_post_route)
                .delete(engine_api::script_owners_delete_route),
        )
        .route(
            "/engine/secrets",
            axum::routing::get(engine_api::secrets_get_route)
                .post(engine_api::secrets_post_route)
                .delete(engine_api::secrets_delete_route),
        )
        .route(
            "/engine/users",
            axum::routing::get(engine_api::users_get_route),
        )
        .route(
            "/engine/user_roles",
            axum::routing::post(engine_api::user_roles_post_route)
                .delete(engine_api::user_roles_delete_route),
        )
        .route(
            "/engine/user_realm",
            axum::routing::post(engine_api::user_realm_post_route),
        )
        .route(
            "/engine/health/cluster",
            axum::routing::get(engine_api::cluster_health_route),
        )
        .route(
            "/engine/openapi.json",
            axum::routing::get(engine_api::openapi_route),
        )
        .layer(axum::middleware::from_fn(management_host_guard))
        .layer(axum::extract::DefaultBodyLimit::max(max_request_body));
    app = app.merge(management_router);

    // Served on every host, so they stay outside the guard above rather than
    // being carved back out of it by path: `/engine/installed` is a static
    // page with no data and the landing target for `/`, and
    // `/auth/unauthorized` is where a failed authorization lands wherever it
    // happened.
    app = app
        .route(
            "/engine/installed",
            axum::routing::get(engine_api::installed_page_route),
        )
        .route(
            "/auth/unauthorized",
            axum::routing::get(engine_api::unauthorized_page_route),
        );

    // Add health check endpoint (no authentication required). Detailed cluster
    // diagnostics live at /engine/health/cluster and require administrator rights.
    app = app
        .route("/health", axum::routing::get(health_handler))
        .route(
            "/.well-known/microsoft-identity-association.json",
            axum::routing::get(|| async {
                axum::Json(serde_json::json!({
                    "associatedApplications": [
                        {
                            "applicationId": "48edce84-c6b1-4559-be54-998dba4c8b4c"
                        }
                    ]
                }))
            }),
        );

    // Add TypeScript type definitions endpoints (no authentication required).
    async fn serve_type_defs(asset_name: &'static str) -> axum::response::Response {
        if let Some(asset) =
            repository::fetch_asset_async("https://example.com/core", asset_name).await
        {
            let mut response = asset.content.into_response();
            response.headers_mut().insert(
                axum::http::header::CONTENT_TYPE,
                axum::http::HeaderValue::from_static("text/plain; charset=utf-8"),
            );
            response.headers_mut().insert(
                axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN,
                axum::http::HeaderValue::from_static("*"),
            );
            response.headers_mut().insert(
                axum::http::header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("public, max-age=3600"),
            );
            response
        } else {
            (StatusCode::NOT_FOUND, "Type definitions not found").into_response()
        }
    }

    let version = env!("CARGO_PKG_VERSION");
    app = app.route(
        &format!("/engine/types/v{}/aiwebengine.d.ts", version),
        axum::routing::get(|| serve_type_defs("aiwebengine.d.ts")),
    );

    // Documentation routes are now handled by docs.js feature script
    // Commented out to avoid conflicts with JavaScript implementation
    // app = app.route(
    //     "/engine/docs",
    //     axum::routing::get(|axum::extract::Path(()): axum::extract::Path<()>| async {
    //         axum::response::Redirect::permanent("/engine/docs/").into_response()
    //     }),
    // );
    // app = app.route(
    //     "/engine/docs/",
    //     axum::routing::get(docs::handle_docs_request),
    // );
    // app = app.route(
    //     "/engine/docs/{*path}",
    //     axum::routing::get(docs::handle_docs_request),
    // );

    // Add catch-all dynamic routes
    let auth_enabled_for_home = auth_enabled;
    let auth_enabled_for_path = auth_enabled;
    let script_timeout_for_home = script_timeout_ms;
    let script_timeout_for_path = script_timeout_ms;
    let max_upload_for_home = config.repository.max_upload_size_bytes;
    let max_upload_for_path = config.repository.max_upload_size_bytes;

    app = app
        .route(
            "/",
            any(move |req: Request<Body>| async move {
                handle_dynamic_request(
                    req,
                    script_timeout_for_home,
                    auth_enabled_for_home,
                    max_upload_for_home,
                    max_request_body,
                )
                .await
            }),
        )
        .route(
            "/{*path}",
            any(move |req: Request<Body>| async move {
                handle_dynamic_request(
                    req,
                    script_timeout_for_path,
                    auth_enabled_for_path,
                    max_upload_for_path,
                    max_request_body,
                )
                .await
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

    // Before anything reads an address: this establishes which one the request
    // actually came from and rewrites the forwarding headers to say so, so no
    // layer, handler or script below it can be told otherwise. Entries that do
    // not parse were refused at startup, so an error here would be a bug rather
    // than a configuration mistake — trusting nothing is the safe reading of
    // one anyway.
    let trusted_proxies = Arc::new(
        security::TrustedProxies::parse(&config.server.trusted_proxies).unwrap_or_default(),
    );
    app = app.layer(axum::middleware::from_fn_with_state(
        trusted_proxies,
        security::normalize_client_ip,
    ));

    // Inside the header layer and outside everything else, because a preflight
    // must be answered before any middleware that wants a session: a browser
    // sends it without credentials, so authenticating it would refuse every
    // cross-origin request before the real one was ever made.
    app = app.layer(axum::middleware::from_fn_with_state(
        Arc::new(security::CorsConfig::from_settings(
            config.security.enable_cors,
            &config.security.cors_allowed_origins,
        )),
        security::cors_middleware,
    ));

    // Outermost, so it sees every response — including those produced by
    // layers inside it, and by scripts. It only ever fills in headers a
    // response did not set for itself.
    app = app.layer(axum::middleware::from_fn_with_state(
        Arc::new(security::SecurityHeadersConfig {
            enabled: config.security.enable_security_headers,
            content_security_policy: config.security.content_security_policy.clone(),
        }),
        security::security_headers_middleware,
    ));

    app
}

/// Handle dynamic requests by routing to registered JavaScript handlers
async fn handle_dynamic_request(
    req: Request<Body>,
    script_timeout_ms: u64,
    _auth_enabled: bool,
    max_upload_size: usize,
    max_request_body_bytes: usize,
) -> impl IntoResponse {
    let path = req.uri().path().to_string();
    let request_method = req.method().to_string();
    // Which host's registrations this request sees. Anything unrecognised
    // resolves to the default host, so direct-IP and dev access keep working.
    let canonical_host = hosts::canonical_host(request_host(req.headers()).as_deref());

    // The management router's guard does not cover engine endpoints reached
    // through this fallback — notably the /engine/script_updates stream, which
    // is served by the stream registry below. /engine is a reserved prefix, so
    // refusing the whole prefix here cannot shadow a script's own route.
    if path.starts_with("/engine/")
        && !engine_api::is_management_host(request_host(req.headers()).as_deref())
    {
        let request_id = req
            .extensions()
            .get::<middleware::RequestId>()
            .map(|rid| rid.0.clone())
            .unwrap_or_else(|| "unknown".to_string());
        warn!(
            "[{}] Engine endpoint {} refused: host is not in server.management_hosts",
            request_id, path
        );
        return error_to_response(error::errors::not_found(&path, &request_id));
    }

    // Check for registered asset paths first if it's a GET request
    if let Some(asset_response) = try_serve_asset(&path, &request_method, &canonical_host).await {
        return asset_response;
    }

    // Check if this is a request to a registered stream path
    if should_route_to_stream(&path, &request_method, &canonical_host).await {
        return handle_stream_request(req).await;
    }

    // Match against the cached route index (rebuilt lazily on script changes),
    // scoped to the host this request came in on
    let route_lookup = match route_index::lookup(&canonical_host, &path, &request_method).await {
        Ok(lookup) => lookup,
        Err(e) => {
            // Treat lookup failure as no match, mirroring the previous behavior
            // when script metadata could not be fetched
            error!("Route lookup failed for {} {}: {}", request_method, path, e);
            route_index::RouteLookup::NotFound
        }
    };

    let (owner_uri, handler_name, route_pattern, route_params, strip_body) = match route_lookup {
        route_index::RouteLookup::Handler {
            script_uri,
            handler_name,
            pattern,
            params,
            strip_body,
        } => (script_uri, handler_name, pattern, params, strip_body),
        no_handler => {
            // Extract request ID from extensions
            let request_id = req
                .extensions()
                .get::<middleware::RequestId>()
                .map(|rid| rid.0.clone())
                .unwrap_or_else(|| "unknown".to_string());

            if matches!(no_handler, route_index::RouteLookup::MethodNotAllowed) {
                warn!(
                    "[{}] ⚠️  Method not allowed: {} {} (path exists but method not registered)",
                    request_id, request_method, path
                );
                return error_to_response(error::errors::method_not_allowed(
                    &path,
                    &request_method,
                    &request_id,
                ));
            } else if path == "/favicon.ico" && request_method == "GET" {
                // No script claimed /favicon.ico — serve the engine default.
                return engine_api::favicon_route().await;
            } else if path == "/" && request_method == "GET" {
                info!(
                    "[{}] 🔄 Redirecting root path to /engine/installed for bootstrapping",
                    request_id
                );
                return Redirect::temporary("/engine/installed").into_response();
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

    // The absolute URL the request arrived on. `path` cannot say which of the
    // engine's hosts served it, and the query string here is the only copy that
    // still has duplicate parameters in it — `query_params` collapses them.
    let request_url = format!(
        "{}{}{}{}",
        hosts::origin(&canonical_host),
        path,
        if query_string.is_empty() { "" } else { "?" },
        query_string
    );

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

    // Read the body with a size cap: form submissions are bounded by the
    // configured upload limit (plus headroom for multipart framing), everything
    // else by the general request body limit. Oversized bodies are rejected
    // instead of being buffered into memory.
    let is_form_data = content_type
        .as_ref()
        .map(|ct| {
            ct.contains("application/x-www-form-urlencoded") || ct.contains("multipart/form-data")
        })
        .unwrap_or(false);

    let body_limit = if is_form_data {
        max_upload_size.saturating_add(64 * 1024)
    } else {
        max_request_body_bytes
    };

    let body_bytes = match to_bytes(body, body_limit).await {
        Ok(bytes) => bytes,
        Err(_) => {
            return Response::builder()
                .status(StatusCode::PAYLOAD_TOO_LARGE)
                .header("content-type", "text/plain")
                .body(Body::from("Request body too large"))
                .unwrap_or_else(|_| Response::new(Body::from("Payload Too Large")));
        }
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

    let (form_data, uploaded_files) = if is_form_data {
        // Parse form data from the bytes
        let body = Body::from(body_bytes.clone());
        match parse_form_data(content_type.as_deref(), body, max_upload_size).await {
            Ok((fields, files)) => (fields, files),
            Err(status) => {
                // Return error response for form parsing failures
                let error_message = match status {
                    StatusCode::PAYLOAD_TOO_LARGE => "File upload exceeds maximum size limit",
                    _ => "Failed to parse form data",
                };
                return Response::builder()
                    .status(status)
                    .header("content-type", "text/plain")
                    .body(Body::from(error_message))
                    .unwrap_or_else(|_| Response::new(Body::from(error_message)));
            }
        }
    } else {
        (HashMap::new(), Vec::new())
    };

    let path_clone = path.clone();
    let headers_for_worker = header_map;
    let request_id_worker = request_id.clone();
    let route_pattern_worker = route_pattern.clone();
    // Taken before the worker closure claims it, so the census can name the
    // request it was serving.
    let method_for_census = request_method.clone();

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
            } else if auth_user.is_editor {
                security::UserContext::editor(auth_user.user_id.clone())
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
            url: Some(request_url.clone()),
            form_data: Some(form_data.clone()),
            raw_body: raw_body.clone(),
            headers: headers_for_worker.clone(),
            user_context, // Use the properly constructed user_context
            auth_context: Some(auth_context),
            route_params: Some(route_params.clone()),
            uploaded_files: Some(uploaded_files.clone()),
            // Files this handler's log lines under the same id the caller got
            // back in `x-request-id`, and under the registration that matched
            // rather than the concrete path.
            request_id: Some(request_id_worker.clone()),
            route_pattern: Some(route_pattern_worker.clone()),
        };

        js_engine::execute_script_for_request_secure(params)
    };

    // What the engine itself writes about this request belongs with what the
    // handler wrote: a failure, a timeout or a panic is the line a developer
    // most needs, and it is useless if it cannot be tied to the run it describes.
    let request_log_context = js_engine::HandlerInvocationKind::HttpRoute.log_context(
        &owner_uri,
        request_id.clone(),
        Some(route_pattern.clone()),
    );

    // The timeout must wrap the un-awaited join handle: awaiting spawn_blocking first
    // would block until the script finishes and the timeout could never fire. On
    // timeout the blocking thread is abandoned; the QuickJS interrupt handler
    // (see js_engine::create_sandboxed_runtime) terminates the script itself.
    // The census is what turns "this request was slow" into "this thread is
    // still out there holding a connection". Without it an abandoned worker
    // leaves no trace beyond one log line, and a leak is invisible until the
    // pool is empty.
    let (ticket, watch) = worker_census::watch(format!("{} {}", method_for_census, route_pattern));

    // The wait for a slot is inside the timeout on purpose: a request that
    // cannot get one within its budget fails as a slow request rather than as
    // a distinct kind of error, and the permit rides into the closure so an
    // abandoned worker keeps holding the slot it is still using.
    let timed = match tokio::time::timeout(
        std::time::Duration::from_millis(script_timeout_ms),
        async move {
            let permit = execution_slots::acquire().await;
            tokio::task::spawn_blocking(move || {
                let _permit = permit;
                let _ticket = ticket;
                worker()
            })
            .await
        },
    )
    .await
    {
        Ok(join) => join.map_err(|e| format!("join error: {}", e)),
        Err(_) => {
            watch.abandon();
            // A timed-out request left no trace in the script's own log before
            // this: the handler was abandoned mid-run, so unless the engine
            // says what happened, the log simply stops mid-invocation.
            let error_msg = format!(
                "Handler '{}' exceeded its {}ms budget and was stopped",
                handler_name, script_timeout_ms
            );
            repository::insert_log_message_async_in_context(
                &owner_uri,
                &error_msg,
                "FATAL",
                &request_log_context,
            )
            .await;
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
            let mut response = build_http_response_from_js(js_response);
            if strip_body {
                *response.body_mut() = Body::empty();
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
            repository::insert_log_message_async_in_context(
                &owner_uri,
                &error_msg,
                "FATAL",
                &request_log_context,
            )
            .await;

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
            let error_msg = format!("Handler '{}' did not run: {}", handler_name, e);
            repository::insert_log_message_async_in_context(
                &owner_uri,
                &error_msg,
                "FATAL",
                &request_log_context,
            )
            .await;
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
    // With connection info, so the address a request came from is available to
    // the layer that decides whether to believe its forwarding headers.
    let svc = app.into_make_service_with_connect_info::<std::net::SocketAddr>();
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

/// Try to serve an asset if the path matches a registered asset
async fn try_serve_asset(path: &str, method: &str, host: &str) -> Option<Response> {
    // Asset routes have no per-method registration (see `AssetPathRegistration`),
    // so HEAD is served the same way as GET with the body dropped afterward.
    if method != "GET" && method != "HEAD" {
        return None;
    }

    let registration = asset_registry::get_global_registry().get_asset_registration(path)?;

    // An asset route belongs to its script, so it is published on the same
    // hosts. Returning None lets the caller fall through to normal route
    // matching and, failing that, a 404.
    if !route_index::script_serves_host(&registration.script_uri, host).await {
        return None;
    }

    if let Some(asset) =
        repository::fetch_asset_async(&registration.script_uri, &registration.asset_name).await
    {
        let mut response = asset.content.into_response();
        // For text/* types, ensure charset=utf-8 is declared so browsers don't
        // fall back to Windows-1252 and garble multi-byte UTF-8 characters.
        let content_type =
            if asset.mimetype.starts_with("text/") && !asset.mimetype.contains("charset") {
                format!("{}; charset=utf-8", asset.mimetype)
            } else {
                asset.mimetype.clone()
            };
        response.headers_mut().insert(
            axum::http::header::CONTENT_TYPE,
            axum::http::HeaderValue::from_str(&content_type).unwrap_or(
                axum::http::HeaderValue::from_static("application/octet-stream"),
            ),
        );
        if method == "HEAD" {
            *response.body_mut() = Body::empty();
        }
        return Some(response);
    }

    warn!(
        "Asset '{}' registered for path '{}' from script '{}' but not found in repository",
        registration.asset_name, path, registration.script_uri
    );
    None
}

/// Check if request should be routed to a stream handler
async fn should_route_to_stream(path: &str, method: &str, host: &str) -> bool {
    let is_get = method == "GET";
    let is_stream_registered = stream_registry::GLOBAL_STREAM_REGISTRY.is_stream_registered(path);

    info!(
        "Stream check - method: {}, is_get: {}, path: '{}', is_registered: {}",
        method, is_get, path, is_stream_registered
    );

    if is_get && is_stream_registered {
        // A stream is published on the hosts of the script that registered it.
        // The engine's own streams have no script behind them, so they are left
        // to the management host guard rather than checked here.
        if let Some(script_uri) =
            stream_registry::GLOBAL_STREAM_REGISTRY.get_stream_script_uri(path)
            && !script_uri.starts_with("engine://")
            && !route_index::script_serves_host(&script_uri, host).await
        {
            info!(
                "Stream {} is not published on host {}; not routing to it",
                path, host
            );
            return false;
        }
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
    use crate::route_index::{calculate_route_specificity, match_route_pattern};
    use std::sync::{Once, OnceLock};

    #[test]
    fn test_redirect_uri_for_base_swaps_origin_only() {
        assert_eq!(
            redirect_uri_for_base(
                "https://softagen.com/auth/callback/google",
                "https://manage.softagen.com"
            ),
            Some("https://manage.softagen.com/auth/callback/google".to_string())
        );
    }

    #[test]
    fn test_redirect_uri_for_base_keeps_query_and_port() {
        assert_eq!(
            redirect_uri_for_base(
                "https://softagen.com/auth/callback/google?flow=web",
                "http://localhost:3000"
            ),
            Some("http://localhost:3000/auth/callback/google?flow=web".to_string())
        );
    }

    #[test]
    fn test_redirect_uri_for_base_rejects_unparseable_input() {
        assert_eq!(
            redirect_uri_for_base("not-a-url", "https://manage.softagen.com"),
            None
        );
        assert_eq!(
            redirect_uri_for_base("https://softagen.com/auth/callback/google", "not-a-url"),
            None
        );
    }

    static INIT_DB: Once = Once::new();
    static DB_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

    fn get_test_runtime() -> &'static tokio::runtime::Runtime {
        DB_RUNTIME.get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap()
        })
    }

    fn should_skip_db_tests() -> bool {
        std::env::var("DATABASE_URL").is_err()
    }

    fn do_db_init(url: String) {
        // Must be called with an active tokio runtime context (either via block_on or block_in_place).
        // connect_lazy spawns maintenance tasks using the current runtime handle.
        // bootstrap_scripts() calls run_blocking → block_in_place, which works in multi-thread.
        let pool = sqlx::PgPool::connect_lazy(&url).unwrap();
        let db = Arc::new(crate::database::Database::from_pool(pool.clone()));
        crate::database::initialize_global_database(db);
        let server_id = crate::notifications::generate_server_id();
        crate::notifications::initialize_server_id(server_id.clone());
        let repo = crate::repository::PostgresRepository::new(pool, server_id);
        crate::repository::initialize_repository(repo);
        // Bootstrap feature scripts so tests can fetch them.
        let _ = crate::repository::bootstrap_scripts();
    }

    fn setup_db() {
        INIT_DB.call_once(|| {
            if std::env::var("DATABASE_URL").is_err() {
                return;
            }
            let url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
                "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine".to_string()
            });
            match tokio::runtime::Handle::try_current() {
                Ok(_) => {
                    // Called from within an active async context (#[tokio::test]).
                    // Use block_in_place so we can call sync run_blocking helpers inside do_db_init
                    // without needing to create another runtime. Pool tasks spawn in the current runtime.
                    tokio::task::block_in_place(|| do_db_init(url));
                }
                Err(_) => {
                    // Plain #[test] – no active runtime. Provide one so connect_lazy and
                    // bootstrap_scripts can work. block_in_place inside bootstrap_scripts is
                    // valid because get_test_runtime() is a multi-thread runtime.
                    get_test_runtime().block_on(async { do_db_init(url) });
                }
            }
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
        if should_skip_db_tests() {
            return;
        }
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

        // Test that test_editor script can be executed without errors
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

    #[tokio::test(flavor = "multi_thread")]
    async fn test_script_crud_operations() {
        if should_skip_db_tests() {
            return;
        }
        setup_db();
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
