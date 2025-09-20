use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use axum::response::IntoResponse;
use axum::{Router, routing::any};
use axum_server::Server;
use serde_urlencoded;
use std::collections::HashMap;
use tracing::{debug, error, info};

pub mod config;
pub mod error;
pub mod js_engine;
pub mod middleware;
pub mod repository;

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
    let script_timeout_ms = config.script_timeout_ms;

    let app = Router::new()
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
    let mut current_port = config.port;
    let mut actual_addr = addr;
    let mut attempts = 0;
    const MAX_PORT_ATTEMPTS: u16 = 100; // Try up to 100 ports

    loop {
        // First check if the port is available using TcpListener
        let test_bind = std::net::TcpListener::bind(actual_addr);
        match test_bind {
            Ok(_) => {
                // Port is available, close the test listener and proceed with axum-server
                drop(test_bind);

                // Successfully found an available port
                if current_port != config.port {
                    info!("Requested port {} was in use, using port {} instead", config.port, current_port);
                } else {
                    info!("listening on {}", actual_addr);
                }

                // record startup in logs so tests can observe server start
                repository::insert_log_message("server started");
                debug!(
                    "Server configuration - host: {}, requested port: {}, actual port: {}",
                    config.host, config.port, current_port
                );

                let svc = app.into_make_service();
                let server = Server::bind(actual_addr).serve(svc);

                tokio::select! {
                    res = server => { res? },
                    _ = &mut shutdown_rx => { /* graceful shutdown: stop accepting new connections */ }
                }

                return Ok(current_port);
            }
            Err(e) => {
                // Check if it's an "Address already in use" error
                let error_msg = e.to_string().to_lowercase();
                if error_msg.contains("address already in use") ||
                   error_msg.contains("address in use") ||
                   error_msg.contains("eaddrinuse") ||
                   e.kind() == std::io::ErrorKind::AddrInUse {
                    attempts += 1;
                    if attempts >= MAX_PORT_ATTEMPTS {
                        return Err(anyhow::anyhow!(
                            "Could not find an available port after trying {} ports starting from {}",
                            MAX_PORT_ATTEMPTS, config.port
                        ));
                    }

                    // Try the next port
                    current_port += 1;
                    actual_addr = format!("{}:{}", config.host, current_port)
                        .parse()
                        .map_err(|e| anyhow::anyhow!("Invalid server address: {}", e))?;

                    debug!("Port {} in use, trying port {}", current_port - 1, current_port);
                } else {
                    // Some other error, return it
                    return Err(anyhow::anyhow!("Failed to bind to address {}: {}", actual_addr, e));
                }
            }
        }
    }
}

pub async fn start_server_without_shutdown() -> anyhow::Result<u16> {
    let (_tx, rx) = tokio::sync::oneshot::channel::<()>();
    start_server(rx).await
}

pub async fn start_server_without_shutdown_with_config(config: config::Config) -> anyhow::Result<u16> {
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
        repository::upsert_script(
            "https://example.com/test_editor",
            include_str!("../scripts/test_editor.js"),
        );
        repository::upsert_script(
            "https://example.com/test_editor_api",
            include_str!("../scripts/test_editor_api.js"),
        );

        // Test that the editor script can be executed without errors
        let result = js_engine::execute_script(
            "https://example.com/editor",
            include_str!("../scripts/editor.js"),
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
            include_str!("../scripts/test_editor.js"),
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
        repository::upsert_script(test_uri, test_content);

        let retrieved = repository::fetch_script(test_uri);
        assert_eq!(
            retrieved,
            Some(test_content.to_string()),
            "Script should be retrievable after upsert"
        );
    }
}
