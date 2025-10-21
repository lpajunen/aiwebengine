/// Mock HTTP server for testing HTTP fetch functionality
/// This replaces the dependency on external services like httpbin.org
use axum::{
    Json, Router,
    body::Body,
    extract::Query,
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::oneshot;

#[derive(Clone)]
pub struct MockServer {
    pub port: u16,
    shutdown_tx: Arc<tokio::sync::Mutex<Option<oneshot::Sender<()>>>>,
}

#[derive(Serialize)]
struct GetResponse {
    args: HashMap<String, String>,
    headers: HashMap<String, String>,
    origin: String,
    url: String,
}

#[derive(Serialize)]
struct PostResponse {
    args: HashMap<String, String>,
    data: String,
    files: HashMap<String, String>,
    form: HashMap<String, String>,
    headers: HashMap<String, String>,
    json: Option<Value>,
    origin: String,
    url: String,
}

#[derive(Serialize)]
struct HeadersResponse {
    headers: HashMap<String, String>,
}

impl MockServer {
    /// Start a new mock server on a random available port
    pub async fn start() -> anyhow::Result<Self> {
        let app = Router::new()
            .route("/get", axum::routing::get(handle_get))
            .route("/post", axum::routing::post(handle_post))
            .route("/put", axum::routing::put(handle_put))
            .route("/delete", axum::routing::delete(handle_delete))
            .route("/patch", axum::routing::patch(handle_patch))
            .route("/headers", axum::routing::get(handle_headers))
            .route(
                "/response-headers",
                axum::routing::get(handle_response_headers),
            )
            .route("/status/{code}", axum::routing::get(handle_status));

        // Bind to random port
        let addr = SocketAddr::from(([127, 0, 0, 1], 0));
        let listener = tokio::net::TcpListener::bind(addr).await?;
        let port = listener.local_addr()?.port();

        let (shutdown_tx, shutdown_rx) = oneshot::channel();

        // Spawn server
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async {
                    shutdown_rx.await.ok();
                })
                .await
                .expect("Server failed to start");
        });

        // Give server a moment to start
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        Ok(Self {
            port,
            shutdown_tx: Arc::new(tokio::sync::Mutex::new(Some(shutdown_tx))),
        })
    }

    /// Get the base URL for this mock server
    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{}", self.port, path)
    }

    /// Shutdown the server
    pub async fn shutdown(self) {
        if let Some(tx) = self.shutdown_tx.lock().await.take() {
            let _ = tx.send(());
        }
    }
}

// Handler functions

async fn handle_get(
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> Json<GetResponse> {
    let headers_map = extract_headers(&headers);

    Json(GetResponse {
        args: params,
        headers: headers_map,
        origin: "127.0.0.1".to_string(),
        url: "http://127.0.0.1/get".to_string(),
    })
}

async fn handle_post(headers: HeaderMap, body: String) -> Json<PostResponse> {
    let headers_map = extract_headers(&headers);

    // Try to parse as JSON
    let json_data = serde_json::from_str::<Value>(&body).ok();

    Json(PostResponse {
        args: HashMap::new(),
        data: body.clone(),
        files: HashMap::new(),
        form: HashMap::new(),
        headers: headers_map,
        json: json_data,
        origin: "127.0.0.1".to_string(),
        url: "http://127.0.0.1/post".to_string(),
    })
}

async fn handle_put(headers: HeaderMap, body: String) -> Json<Value> {
    let headers_map = extract_headers(&headers);

    Json(json!({
        "args": {},
        "data": body,
        "files": {},
        "form": {},
        "headers": headers_map,
        "json": serde_json::from_str::<Value>(&body).ok(),
        "origin": "127.0.0.1",
        "url": "http://127.0.0.1/put"
    }))
}

async fn handle_delete(headers: HeaderMap) -> Json<Value> {
    let headers_map = extract_headers(&headers);

    Json(json!({
        "args": {},
        "data": "",
        "files": {},
        "form": {},
        "headers": headers_map,
        "json": null,
        "origin": "127.0.0.1",
        "url": "http://127.0.0.1/delete"
    }))
}

async fn handle_patch(headers: HeaderMap, body: String) -> Json<Value> {
    let headers_map = extract_headers(&headers);

    Json(json!({
        "args": {},
        "data": body,
        "files": {},
        "form": {},
        "headers": headers_map,
        "json": serde_json::from_str::<Value>(&body).ok(),
        "origin": "127.0.0.1",
        "url": "http://127.0.0.1/patch"
    }))
}

async fn handle_headers(headers: HeaderMap) -> Json<HeadersResponse> {
    let headers_map = extract_headers(&headers);

    Json(HeadersResponse {
        headers: headers_map,
    })
}

async fn handle_response_headers(
    headers: HeaderMap,
    Query(params): Query<HashMap<String, String>>,
) -> impl IntoResponse {
    let mut response_headers = HeaderMap::new();

    // Add any query parameters as response headers
    for (key, value) in params {
        if let Ok(header_value) = header::HeaderValue::from_str(&value) {
            if let Ok(header_name) = header::HeaderName::from_bytes(key.as_bytes()) {
                response_headers.insert(header_name, header_value);
            }
        }
    }

    // Add standard headers
    response_headers.insert(
        header::CONTENT_TYPE,
        header::HeaderValue::from_static("application/json"),
    );

    let headers_map = extract_headers(&headers);
    let body = json!({
        "headers": headers_map,
    });

    (response_headers, Json(body))
}

async fn handle_status(axum::extract::Path(code): axum::extract::Path<u16>) -> Response {
    let status = StatusCode::from_u16(code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);

    (status, Body::empty()).into_response()
}

// Helper function to extract headers as a HashMap
fn extract_headers(headers: &HeaderMap) -> HashMap<String, String> {
    headers
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|v| (name.as_str().to_string(), v.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_server_starts() {
        let server = MockServer::start().await.unwrap();
        assert!(server.port > 0);

        // Test that we can make a request
        let client = reqwest::Client::new();
        let response = client.get(&server.url("/get")).send().await.unwrap();

        assert_eq!(response.status(), 200);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn test_mock_server_post() {
        let server = MockServer::start().await.unwrap();

        let client = reqwest::Client::new();
        let response = client
            .post(&server.url("/post"))
            .json(&json!({"test": "data"}))
            .send()
            .await
            .unwrap();

        assert_eq!(response.status(), 200);
        let body: Value = response.json().await.unwrap();
        assert!(body.get("json").is_some());

        server.shutdown().await;
    }

    #[tokio::test]
    async fn test_mock_server_status() {
        let server = MockServer::start().await.unwrap();

        let client = reqwest::Client::new();
        let response = client.get(&server.url("/status/404")).send().await.unwrap();

        assert_eq!(response.status(), 404);
        server.shutdown().await;
    }
}
