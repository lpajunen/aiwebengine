use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use axum::response::{IntoResponse, Response, Sse, sse::Event};
use axum::{Router, routing::any};
use axum_server::Server;
use serde_urlencoded;
use std::collections::HashMap;
use tokio_stream::{StreamExt, wrappers::BroadcastStream};
use tracing::{debug, error, info};

pub mod config;
pub mod error;
pub mod graphql;
pub mod js_engine;
pub mod middleware;
pub mod repository;
pub mod safe_helpers;
pub mod stream_manager;
pub mod stream_registry;

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
                .unwrap();
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
    let sse_stream = receiver_stream.map(move |result| {
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

/// Dynamically find a route handler by checking all current scripts
fn find_route_handler(path: &str, method: &str) -> Option<(String, String)> {
    let scripts = repository::fetch_scripts();

    for (uri, content) in scripts.into_iter() {
        // Execute the script to get its route registrations
        let result = js_engine::execute_script(&uri, &content);

        if result.success {
            // Check for exact match
            if let Some(handler) = result
                .registrations
                .get(&(path.to_string(), method.to_string()))
            {
                return Some((uri, handler.clone()));
            }

            // Check for wildcard matches
            for ((pattern, reg_method), handler) in &result.registrations {
                if reg_method == method && pattern.ends_with("/*") {
                    let prefix = &pattern[..pattern.len() - 1]; // Remove the *
                    if path.starts_with(prefix) {
                        return Some((uri.clone(), handler.clone()));
                    }
                }
            }
        }
    }

    None
}

/// Check if any script registers a route for the given path (used for 405 responses)
fn path_has_any_route(path: &str) -> bool {
    let scripts = repository::fetch_scripts();

    for (uri, content) in scripts.into_iter() {
        let result = js_engine::execute_script(&uri, &content);

        if result.success {
            // Check for exact match
            if result.registrations.keys().any(|(p, _)| p == path) {
                return true;
            }

            // Check for wildcard matches
            for (pattern, _) in result.registrations.keys() {
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
    // Clone the timeout value to avoid borrow checker issues in async closures
    let script_timeout_ms = config.script_timeout_ms();

    // Execute all scripts at startup to populate GraphQL registry
    info!("Executing all scripts at startup to populate GraphQL registry...");
    let scripts = repository::fetch_scripts();
    for (uri, content) in scripts.iter() {
        info!("Executing script: {}", uri);
        let result = js_engine::execute_script(uri, content);
        if !result.success {
            error!("Failed to execute script {}: {:?}", uri, result.error);
        } else {
            info!("Successfully executed script: {}", uri);
        }
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

    // GraphQL SSE handler - handles subscriptions over Server-Sent Events
    async fn graphql_sse(
        schema: async_graphql::dynamic::Schema,
        req: axum::http::Request<axum::body::Body>,
    ) -> impl IntoResponse {
        // Helper function to extract subscription name from GraphQL query
        fn extract_subscription_name(query: &str) -> Option<String> {
            // Basic regex to extract subscription field name
            // This is a simple implementation - a full GraphQL parser would be more robust
            if let Ok(re) = regex::Regex::new(r"subscription\s*\{\s*(\w+)") {
                if let Some(captures) = re.captures(query) {
                    return Some(captures[1].to_string());
                }
            }
            None
        }

        let (_parts, body) = req.into_parts();
        let body_bytes = match axum::body::to_bytes(body, usize::MAX).await {
            Ok(bytes) => bytes,
            Err(_) => {
                return axum::response::Response::builder()
                    .status(400)
                    .header("content-type", "text/plain")
                    .body(axum::body::Body::from("Failed to read request body"))
                    .unwrap();
            }
        };

        let request: async_graphql::Request = match serde_json::from_slice(&body_bytes) {
            Ok(req) => req,
            Err(e) => {
                return axum::response::Response::builder()
                    .status(400)
                    .header("content-type", "text/plain")
                    .body(axum::body::Body::from(format!("Invalid JSON: {}", e)))
                    .unwrap();
            }
        };

        // Check if this is a subscription operation
        let is_subscription = request.query.trim_start().starts_with("subscription");

        if is_subscription {
            // For GraphQL subscriptions, we need to:
            // 1. Execute the subscription to initialize it (call the resolver)
            // 2. Extract the subscription name from the query
            // 3. Connect to the corresponding stream path that was auto-registered

            // Try to extract subscription name from the query (basic parsing)
            let subscription_name = extract_subscription_name(&request.query);

            // Execute the subscription to initialize it
            let initial_response = schema.execute(request).await;

            if let Some(sub_name) = subscription_name {
                let stream_path = format!("/graphql/subscription/{}", sub_name);

                // Create a connection to the stream for this subscription
                match crate::stream_manager::StreamConnectionManager::new()
                    .create_connection(&stream_path, None)
                    .await
                {
                    Ok(connection) => {
                        let mut receiver = connection.receiver;

                        // Create an SSE stream that sends the initial response and then listens for updates
                        let initial_json = serde_json::to_string(&initial_response)
                            .unwrap_or_else(|_| "{}".to_string());
                        let initial_sse = format!("data: {}\n\n", initial_json);

                        // Create a stream that starts with the initial response and then streams updates
                        let stream = async_stream::stream! {
                            // Send initial response
                            yield Ok::<String, std::convert::Infallible>(initial_sse);

                            // Then stream updates
                            while let Ok(message) = receiver.recv().await {
                                let sse_data = format!("data: {}\n\n", message);
                                yield Ok::<String, std::convert::Infallible>(sse_data);
                            }
                        };

                        let body = axum::body::Body::from_stream(stream);

                        return axum::response::Response::builder()
                            .header("content-type", "text/event-stream")
                            .header("cache-control", "no-cache")
                            .header("connection", "keep-alive")
                            .header("access-control-allow-origin", "*")
                            .header("access-control-allow-headers", "content-type")
                            .body(body)
                            .unwrap();
                    }
                    Err(e) => {
                        tracing::error!(
                            "Failed to create stream connection for subscription: {}",
                            e
                        );
                    }
                }
            }

            // Fallback: return initial response only
            let json_data =
                serde_json::to_string(&initial_response).unwrap_or_else(|_| "{}".to_string());
            let sse_data = format!("data: {}\n\n", json_data);
            axum::response::Response::builder()
                .header("content-type", "text/event-stream")
                .header("cache-control", "no-cache")
                .header("connection", "keep-alive")
                .header("access-control-allow-origin", "*")
                .header("access-control-allow-headers", "content-type")
                .body(axum::body::Body::from(sse_data))
                .unwrap()
        } else {
            // Handle regular queries/mutations as single response
            let response = schema.execute(request).await;
            let json_data = serde_json::to_string(&response).unwrap_or_else(|_| "{}".to_string());

            // Return SSE formatted response
            let sse_data = format!("data: {}\n\n", json_data);
            axum::response::Response::builder()
                .header("content-type", "text/event-stream")
                .header("cache-control", "no-cache")
                .header("connection", "keep-alive")
                .header("access-control-allow-origin", "*")
                .header("access-control-allow-headers", "content-type")
                .body(axum::body::Body::from(sse_data))
                .unwrap()
        }
    }

    // Clone schema for handlers
    let schema_for_post = schema.clone();
    let schema_for_sse = schema.clone();

    let app = Router::new()
        // GraphQL endpoints
        .route("/graphql", axum::routing::get(graphql_get))
        .route(
            "/graphql",
            axum::routing::post(move |req| graphql_post(schema_for_post, req)),
        )
        .route(
            "/graphql/sse",
            axum::routing::post(move |req| graphql_sse(schema_for_sse, req)),
        )
        .route(
            "/",
            any(move |req: Request<Body>| async move {
                let path = req.uri().path().to_string();
                let request_method = req.method().to_string();

                // Check for assets first if it's a GET request
                if request_method == "GET" {
                    if let Some(asset) = repository::fetch_asset(&path) {
                        let mut response = asset.content.into_response();
                        response.headers_mut().insert(
                            axum::http::header::CONTENT_TYPE,
                            axum::http::HeaderValue::from_str(&asset.mimetype).unwrap_or(
                                axum::http::HeaderValue::from_static("application/octet-stream"),
                            ),
                        );
                        return response;
                    }
                }

                // Check if this is a request to a registered stream path
                let is_get = request_method == "GET";
                let is_stream_registered = stream_registry::GLOBAL_STREAM_REGISTRY.is_stream_registered(&path);
                info!("Stream check - method: {}, is_get: {}, path: '{}', is_registered: {}", 
                      request_method, is_get, path, is_stream_registered);

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
                            let error_response = error::errors::method_not_allowed(
                                &path,
                                &request_method,
                                &request_id,
                            );
                            return (
                                StatusCode::from_u16(error_response.status).unwrap(),
                                serde_json::to_string(&error_response).unwrap(),
                            )
                                .into_response();
                        } else {
                            let error_response = error::errors::not_found(&path, &request_id);
                            return (
                                StatusCode::from_u16(error_response.status).unwrap(),
                                serde_json::to_string(&error_response).unwrap(),
                            )
                                .into_response();
                        }
                    }
                };
                let owner_uri_cl = owner_uri.clone();
                let handler_cl = handler_name.clone();
                let path_log = path.to_string();
                let query_string = req.uri().query().map(|s| s.to_string()).unwrap_or_default();
                let query_params = parse_query_string(&query_string);

                // Extract request ID from extensions before consuming the request
                let request_id = req
                    .extensions()
                    .get::<middleware::RequestId>()
                    .map(|rid| rid.0.clone())
                    .unwrap_or_else(|| "unknown".to_string());

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

                // For POST requests to editor API, use raw body
                let raw_body = if request_method == "POST" && path.starts_with("/api/scripts/") {
                    Some(String::from_utf8(body_bytes.to_vec()).unwrap_or_default())
                } else {
                    None
                };

                let form_data = if raw_body.is_some() {
                    // If we have raw body, don't parse as form data
                    HashMap::new()
                } else {
                    // Parse form data from the bytes
                    let body = Body::from(body_bytes);
                    if let Some(ct) = content_type {
                        parse_form_data(Some(&ct), body).await.unwrap_or_default()
                    } else {
                        parse_form_data(None, body).await.unwrap_or_default()
                    }
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

                let timed = match tokio::time::timeout(
                    std::time::Duration::from_millis(script_timeout_ms),
                    async { join },
                )
                .await
                {
                    Ok(r) => r,
                    Err(_) => {
                        let error_response = error::errors::script_timeout(&path, &request_id);
                        return (
                            StatusCode::from_u16(error_response.status).unwrap(),
                            serde_json::to_string(&error_response).unwrap(),
                        )
                            .into_response();
                    }
                };

                match timed {
                    Ok(Ok((status, body, content_type))) => {
                        let mut response = (
                            StatusCode::from_u16(status)
                                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                            body,
                        )
                            .into_response();

                        if let Some(ct) = content_type {
                            response.headers_mut().insert(
                                axum::http::header::CONTENT_TYPE,
                                axum::http::HeaderValue::from_str(&ct).unwrap_or_else(|_| {
                                    axum::http::HeaderValue::from_static("text/plain")
                                }),
                            );
                        }

                        response
                    }
                    Ok(Err(e)) => {
                        error!("script error for {}: {}", path_log, e);
                        let error_response =
                            error::errors::script_execution_failed(&path, &e, &request_id);
                        (
                            StatusCode::from_u16(error_response.status).unwrap(),
                            serde_json::to_string(&error_response).unwrap(),
                        )
                            .into_response()
                    }
                    Err(e) => {
                        error!("task error for {}: {}", path_log, e);
                        let error_response =
                            error::errors::internal_server_error(&path, &e, &request_id);
                        (
                            StatusCode::from_u16(error_response.status).unwrap(),
                            serde_json::to_string(&error_response).unwrap(),
                        )
                            .into_response()
                    }
                }
            }),
        )
        .route(
            "/{*path}",
            any(move |req: Request<Body>| async move {
                let full_path = req.uri().path().to_string();
                let request_method = req.method().to_string();

                // Check for assets first if it's a GET request
                if request_method == "GET" {
                    if let Some(asset) = repository::fetch_asset(&full_path) {
                        let mut response = asset.content.into_response();
                        response.headers_mut().insert(
                            axum::http::header::CONTENT_TYPE,
                            axum::http::HeaderValue::from_str(&asset.mimetype).unwrap_or(
                                axum::http::HeaderValue::from_static("application/octet-stream"),
                            ),
                        );
                        return response;
                    }
                }

                // Check if this is a request to a registered stream path
                let is_get = request_method == "GET";
                let is_stream_registered = stream_registry::GLOBAL_STREAM_REGISTRY.is_stream_registered(&full_path);
                info!("Stream check (wildcard) - method: {}, is_get: {}, path: '{}', is_registered: {}", 
                      request_method, is_get, full_path, is_stream_registered);

                if is_get && is_stream_registered {
                    info!("Routing to stream handler for path: {}", full_path);
                    return handle_stream_request(full_path).await;
                }

                // Check if any route exists for this path (including wildcards)
                let path_exists = path_has_any_route(&full_path);

                let reg = find_route_handler(&full_path, &request_method);
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
                            let error_response = error::errors::method_not_allowed(
                                &full_path,
                                &request_method,
                                &request_id,
                            );
                            return (
                                StatusCode::from_u16(error_response.status).unwrap(),
                                serde_json::to_string(&error_response).unwrap(),
                            )
                                .into_response();
                        } else {
                            let error_response = error::errors::not_found(&full_path, &request_id);
                            return (
                                StatusCode::from_u16(error_response.status).unwrap(),
                                serde_json::to_string(&error_response).unwrap(),
                            )
                                .into_response();
                        }
                    }
                };
                let owner_uri_cl = owner_uri.clone();
                let handler_cl = handler_name.clone();
                let path_log = full_path.clone();
                let query_string = req.uri().query().map(|s| s.to_string()).unwrap_or_default();
                let query_params = parse_query_string(&query_string);

                // Extract request ID from extensions before consuming the request
                let request_id = req
                    .extensions()
                    .get::<middleware::RequestId>()
                    .map(|rid| rid.0.clone())
                    .unwrap_or_else(|| "unknown".to_string());

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

                // For POST requests to editor API, use raw body
                let raw_body = if request_method == "POST" && full_path.starts_with("/api/scripts/")
                {
                    Some(String::from_utf8(body_bytes.to_vec()).unwrap_or_default())
                } else {
                    None
                };

                let form_data = if raw_body.is_some() {
                    // If we have raw body, don't parse as form data
                    HashMap::new()
                } else {
                    // Parse form data from the bytes
                    let body = Body::from(body_bytes);
                    if let Some(ct) = content_type {
                        parse_form_data(Some(&ct), body).await.unwrap_or_default()
                    } else {
                        parse_form_data(None, body).await.unwrap_or_default()
                    }
                };

                let full_path_clone = full_path.clone();
                let worker = move || -> Result<(u16, String, Option<String>), String> {
                    js_engine::execute_script_for_request(
                        &owner_uri_cl,
                        &handler_cl,
                        &full_path_clone,
                        &request_method,
                        Some(&query_params),
                        Some(&form_data),
                        raw_body,
                    )
                };

                let join = tokio::task::spawn_blocking(worker)
                    .await
                    .map_err(|e| format!("join error: {}", e));

                let timed = match tokio::time::timeout(
                    std::time::Duration::from_millis(script_timeout_ms),
                    async { join },
                )
                .await
                {
                    Ok(r) => r,
                    Err(_) => {
                        let error_response = error::errors::script_timeout(&full_path, &request_id);
                        return (
                            StatusCode::from_u16(error_response.status).unwrap(),
                            serde_json::to_string(&error_response).unwrap(),
                        )
                            .into_response();
                    }
                };

                match timed {
                    Ok(Ok((status, body, content_type))) => {
                        let mut response = (
                            StatusCode::from_u16(status)
                                .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                            body,
                        )
                            .into_response();

                        if let Some(ct) = content_type {
                            response.headers_mut().insert(
                                axum::http::header::CONTENT_TYPE,
                                axum::http::HeaderValue::from_str(&ct).unwrap_or_else(|_| {
                                    axum::http::HeaderValue::from_static("text/plain")
                                }),
                            );
                        }

                        response
                    }
                    Ok(Err(e)) => {
                        error!("script error for {}: {}", path_log, e);
                        let error_response =
                            error::errors::script_execution_failed(&full_path, &e, &request_id);
                        (
                            StatusCode::from_u16(error_response.status).unwrap(),
                            serde_json::to_string(&error_response).unwrap(),
                        )
                            .into_response()
                    }
                    Err(e) => {
                        error!("task error for {}: {}", path_log, e);
                        let error_response =
                            error::errors::internal_server_error(&full_path, &e, &request_id);
                        (
                            StatusCode::from_u16(error_response.status).unwrap(),
                            serde_json::to_string(&error_response).unwrap(),
                        )
                            .into_response()
                    }
                }
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

        // Test that the editor script can be executed without errors
        let result = js_engine::execute_script(
            "https://example.com/editor",
            include_str!("../scripts/feature_scripts/editor.js"),
        );
        assert!(
            result.success,
            "Editor script should execute successfully: {:?}",
            result.error
        );
        assert!(
            !result.registrations.is_empty(),
            "Editor script should register routes"
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
        assert!(
            !test_editor_result.registrations.is_empty(),
            "Test editor script should register routes"
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
