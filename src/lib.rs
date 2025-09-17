use axum::body::{Body, to_bytes};
use axum::http::Request;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::{Router, routing::any};
use axum_server::Server;
use serde_urlencoded;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tracing::{debug, error, info};

pub mod config;
pub mod js_engine;
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

/// Type alias for route registrations: (path, method) -> (script_uri, handler_name)
type RouteRegistry = Arc<Mutex<HashMap<(String, String), (String, String)>>>;

/// Builds the route registry by loading all scripts and collecting their route registrations.
///
/// This function executes each script using the js_engine module and collects
/// any routes registered via the `register(path, handler)` function exposed to JavaScript.
fn build_registrations() -> anyhow::Result<RouteRegistry> {
    let regs = Arc::new(Mutex::new(HashMap::new()));

    let scripts = repository::fetch_scripts();
    debug!("Found {} scripts to load", scripts.len());

    for (uri, content) in scripts.into_iter() {
        debug!("Loading script {}", uri);

        // Execute the script using the js_engine module
        let result = js_engine::execute_script(&uri, &content);

        if result.success {
            // Merge the registrations from this script into the global registry
            if let Ok(mut global_regs) = regs.lock() {
                for ((path, method), handler) in result.registrations {
                    global_regs.insert((path, method), (uri.clone(), handler));
                }
            }
            debug!("Successfully loaded script {}", uri);
        } else {
            if let Some(error) = result.error {
                error!("Failed to load script {}: {}", uri, error);
            }
        }
    }

    // Debug: print all registered routes
    if let Ok(regs_locked) = regs.lock() {
        debug!("Final route registry: {:?}", *regs_locked);
    }

    Ok(regs)
}

/// Starts the web server with the given shutdown receiver.
///
/// This function:
/// 1. Builds the route registry from all available scripts
/// 2. Sets up the Axum router with dynamic route handling
/// 3. Starts the server on the configured address
/// 4. Listens for shutdown signal
pub async fn start_server(shutdown_rx: tokio::sync::oneshot::Receiver<()>) -> anyhow::Result<()> {
    start_server_with_config(config::Config::from_env(), shutdown_rx).await
}

/// Starts the web server with custom configuration
pub async fn start_server_with_config(
    config: config::Config,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
) -> anyhow::Result<()> {
    let registrations = Arc::new(build_registrations()?);

    let registrations_clone = Arc::clone(&registrations);

    let app = Router::new()
        .route(
            "/",
            any(move |req: Request<Body>| {
                let regs = Arc::clone(&registrations);
                async move {
                    let path = req.uri().path().to_string();
                    let request_method = req.method().to_string();

                    // Check for assets first if it's a GET request
                    if request_method == "GET" {
                        if let Some(asset) = repository::fetch_asset(&path) {
                            let mut response = asset.content.into_response();
                            response.headers_mut().insert(
                                axum::http::header::CONTENT_TYPE,
                                axum::http::HeaderValue::from_str(&asset.mimetype).unwrap_or(
                                    axum::http::HeaderValue::from_static(
                                        "application/octet-stream",
                                    ),
                                ),
                            );
                            return response;
                        }
                    }

                    // Check if any route exists for this path (including wildcards)
                    let path_exists = regs
                        .lock()
                        .ok()
                        .map(|g| {
                            // Check for exact match
                            if g.keys().any(|(p, _)| p == &path) {
                                return true;
                            }

                            // Check for wildcard matches
                            for (pattern, _) in g.keys() {
                                if pattern.ends_with("/*") {
                                    let prefix = &pattern[..pattern.len() - 1]; // Remove the *
                                    if path.starts_with(prefix) {
                                        return true;
                                    }
                                }
                            }

                            false
                        })
                        .unwrap_or(false);

                    let reg = regs.lock().ok().and_then(|g| {
                        // First try exact match
                        if let Some(exact_match) = g.get(&(path.clone(), request_method.clone())) {
                            return Some(exact_match.clone());
                        }

                        // If no exact match, try wildcard matching
                        for ((pattern, method), handler) in g.iter() {
                            if method == &request_method && pattern.ends_with("/*") {
                                let prefix = &pattern[..pattern.len() - 1]; // Remove the *
                                if path.starts_with(prefix) {
                                    return Some(handler.clone());
                                }
                            }
                        }

                        None
                    });
                    let (owner_uri, handler_name) = match reg {
                        Some(t) => t,
                        None => {
                            if path_exists {
                                return (
                                    StatusCode::METHOD_NOT_ALLOWED,
                                    "method not allowed".to_string(),
                                )
                                    .into_response();
                            } else {
                                return (StatusCode::NOT_FOUND, "not found".to_string())
                                    .into_response();
                            }
                        }
                    };
                    let owner_uri_cl = owner_uri.clone();
                    let handler_cl = handler_name.clone();
                    let path_log = path.to_string();
                    let query_string = req.uri().query().map(|s| s.to_string()).unwrap_or_default();
                    let query_params = parse_query_string(&query_string);

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
                    let raw_body = if request_method == "POST" && path.starts_with("/api/scripts/")
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

                    let worker = move || -> Result<(u16, String, Option<String>), String> {
                        js_engine::execute_script_for_request(
                            &owner_uri_cl,
                            &handler_cl,
                            &path,
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
                        std::time::Duration::from_millis(config.script_timeout_ms),
                        async { join },
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(_) => {
                            return (StatusCode::GATEWAY_TIMEOUT, "script timeout".to_string())
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
                            (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                format!("script error: {}", e),
                            )
                                .into_response()
                        }
                        Err(e) => {
                            error!("task error for {}: {}", path_log, e);
                            (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                format!("task error: {}", e),
                            )
                                .into_response()
                        }
                    }
                }
            }),
        )
        .route(
            "/{*path}",
            any(move |req: Request<Body>| {
                let regs = Arc::clone(&registrations_clone);
                async move {
                    let full_path = req.uri().path().to_string();
                    let request_method = req.method().to_string();

                    // Check if any route exists for this path (including wildcards)
                    let path_exists = regs
                        .lock()
                        .ok()
                        .map(|g| {
                            // Check for exact match
                            if g.keys().any(|(p, _)| p == &full_path) {
                                return true;
                            }

                            // Check for wildcard matches
                            for (pattern, _) in g.keys() {
                                if pattern.ends_with("/*") {
                                    let prefix = &pattern[..pattern.len() - 1]; // Remove the *
                                    if full_path.starts_with(prefix) {
                                        return true;
                                    }
                                }
                            }

                            false
                        })
                        .unwrap_or(false);

                    let reg = regs.lock().ok().and_then(|g| {
                        // First try exact match
                        if let Some(exact_match) =
                            g.get(&(full_path.clone(), request_method.clone()))
                        {
                            return Some(exact_match.clone());
                        }

                        // If no exact match, try wildcard matching
                        for ((pattern, method), handler) in g.iter() {
                            if method == &request_method && pattern.ends_with("/*") {
                                let prefix = &pattern[..pattern.len() - 1]; // Remove the *
                                if full_path.starts_with(prefix) {
                                    return Some(handler.clone());
                                }
                            }
                        }

                        None
                    });
                    let (owner_uri, handler_name) = match reg {
                        Some(t) => t,
                        None => {
                            if path_exists {
                                return (
                                    StatusCode::METHOD_NOT_ALLOWED,
                                    "method not allowed".to_string(),
                                )
                                    .into_response();
                            } else {
                                return (StatusCode::NOT_FOUND, "not found".to_string())
                                    .into_response();
                            }
                        }
                    };
                    let owner_uri_cl = owner_uri.clone();
                    let handler_cl = handler_name.clone();
                    let path_log = full_path.clone();
                    let query_string = req.uri().query().map(|s| s.to_string()).unwrap_or_default();
                    let query_params = parse_query_string(&query_string);

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
                    let raw_body =
                        if request_method == "POST" && full_path.starts_with("/api/scripts/") {
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

                    let worker = move || -> Result<(u16, String, Option<String>), String> {
                        js_engine::execute_script_for_request(
                            &owner_uri_cl,
                            &handler_cl,
                            &full_path,
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
                        std::time::Duration::from_millis(config.script_timeout_ms),
                        async { join },
                    )
                    .await
                    {
                        Ok(r) => r,
                        Err(_) => {
                            return (StatusCode::GATEWAY_TIMEOUT, "script timeout".to_string())
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
                            (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                format!("script error: {}", e),
                            )
                                .into_response()
                        }
                        Err(e) => {
                            error!("task error for {}: {}", path_log, e);
                            (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                format!("task error: {}", e),
                            )
                                .into_response()
                        }
                    }
                }
            }),
        );

    let addr = config
        .server_addr()
        .parse()
        .map_err(|e| anyhow::anyhow!("Invalid server address: {}", e))?;

    // record startup in logs so tests can observe server start
    repository::insert_log_message("server started");
    info!("listening on {}", addr);
    debug!(
        "Server configuration - host: {}, port: {}",
        config.host, config.port
    );
    let svc = app.into_make_service();
    let server = Server::bind(addr).serve(svc);

    tokio::select! {
        res = server => { res? },
        _ = &mut shutdown_rx => { /* graceful shutdown: stop accepting new connections */ }
    }

    Ok(())
}

pub async fn start_server_without_shutdown() -> anyhow::Result<()> {
    let (_tx, rx) = tokio::sync::oneshot::channel::<()>();
    start_server(rx).await
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
