/// Mock HTTP server for testing HTTP fetch functionality
/// This replaces the dependency on external services like httpbin.org
use axum::{
    Json, Router,
    body::Body,
    extract::{Path, Query},
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
            .route("/status/{code}", axum::routing::get(handle_status))
            // Answers with the path segment it was given, which is how a test
            // sees what actually arrived in the URL rather than what was
            // intended to. `{{secret:...}}` in a path is proved by this.
            .route("/path/{tail}", axum::routing::get(handle_path))
            .route("/redirect/{n}", axum::routing::any(handle_redirect))
            .route("/redirect-loop", axum::routing::get(handle_redirect_loop))
            .route(
                "/compressed/{coding}",
                axum::routing::get(handle_compressed),
            )
            // Answers after `ms` milliseconds. What makes a parallel fetch
            // measurable: three of these together take one delay, and three
            // in series take three.
            .route("/slow/{ms}", axum::routing::any(handle_slow))
            // A chunked response that arrives in pieces over time, which is
            // the shape a model's token stream has.
            .route("/stream/{pieces}", axum::routing::any(handle_stream))
            // A chunked response whose pieces split a multi-byte character
            // down the middle — the boundary a naive decoder turns into
            // U+FFFD.
            .route("/stream-split", axum::routing::get(handle_stream_split))
            .route("/binary", axum::routing::get(handle_binary));

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

/// Answer with a body under a `Content-Encoding`.
///
/// `coding` picks what is applied: `gzip` and `deflate` are real, `br` is
/// labelled but never applied (nothing here can produce brotli, and the point
/// is what the client does with a coding it cannot undo), and `bomb` is a
/// gzip stream that inflates past the client's response ceiling.
///
/// The body repeats the request's `Accept-Encoding` so a test can see what was
/// offered as well as what came back.
async fn handle_compressed(Path(coding): Path<String>, headers: HeaderMap) -> Response {
    use std::io::Write;

    let offered = headers
        .get(header::ACCEPT_ENCODING)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();

    let payload = if coding == "bomb" {
        "a".repeat(11 * 1024 * 1024)
    } else {
        json!({ "accept_encoding": offered, "message": "compressed payload" }).to_string()
    };

    let (label, body) = match coding.as_str() {
        "gzip" | "bomb" => {
            let mut encoder =
                flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(payload.as_bytes()).expect("gzip");
            ("gzip", encoder.finish().expect("finish gzip"))
        }
        "deflate" => {
            let mut encoder =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(payload.as_bytes()).expect("deflate");
            ("deflate", encoder.finish().expect("finish deflate"))
        }
        "raw-deflate" => {
            let mut encoder =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            encoder.write_all(payload.as_bytes()).expect("raw deflate");
            ("deflate", encoder.finish().expect("finish raw deflate"))
        }
        // Labelled brotli, sent as plain text: a client that trusts the label
        // must say it cannot read this, not hand the bytes on as if it had.
        _ => ("br", payload.into_bytes()),
    };

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::CONTENT_ENCODING, label)
        .header(header::CONTENT_LENGTH, body.len())
        .body(Body::from(body))
        .expect("build compressed response")
}

/// Bytes that are not text, which is what a photo or a PDF actually is.
///
/// A PNG signature: it begins with `0x89`, which is not a valid UTF-8 start
/// byte, so a client decoding it as a string fails on the first character
/// rather than somewhere in the middle. That is the case worth pinning — the
/// failure is immediate and total, not a few replacement characters.
async fn handle_binary() -> Response {
    let body: Vec<u8> = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0xff, 0xfe];
    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "image/png")
        .header(header::CONTENT_LENGTH, body.len())
        .body(Body::from(body))
        .expect("build binary response")
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
        if let Ok(header_value) = header::HeaderValue::from_str(&value)
            && let Ok(header_name) = header::HeaderName::from_bytes(key.as_bytes())
        {
            response_headers.insert(header_name, header_value);
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

async fn handle_path(axum::extract::Path(tail): axum::extract::Path<String>) -> String {
    tail
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

/// Redirects n times (302) before landing on /get
async fn handle_redirect(axum::extract::Path(n): axum::extract::Path<u32>) -> Response {
    let location = if n <= 1 {
        "/get".to_string()
    } else {
        format!("/redirect/{}", n - 1)
    };
    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, location)
        .body(Body::empty())
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Redirects to itself forever (302)
async fn handle_redirect_loop() -> Response {
    Response::builder()
        .status(StatusCode::FOUND)
        .header(header::LOCATION, "/redirect-loop")
        .body(Body::empty())
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// Answers after a delay, so a test can tell parallel from sequential.
async fn handle_slow(Path(ms): Path<u64>) -> impl IntoResponse {
    tokio::time::sleep(tokio::time::Duration::from_millis(ms)).await;
    Json(json!({ "sleptMs": ms }))
}

/// A chunked body delivered in pieces with a gap between them.
///
/// The gap is what makes it a stream rather than a buffered body that
/// happens to be sent in parts: a reader that only returns when the whole
/// response has arrived cannot tell the difference without one.
async fn handle_stream(Path(pieces): Path<usize>) -> impl IntoResponse {
    let body = async_stream::stream! {
        for index in 0..pieces {
            tokio::time::sleep(tokio::time::Duration::from_millis(20)).await;
            yield Ok::<_, std::io::Error>(axum::body::Bytes::from(format!("piece-{} ", index)));
        }
    };

    axum::response::Response::builder()
        .status(200)
        .header("content-type", "text/plain; charset=utf-8")
        .body(axum::body::Body::from_stream(body))
        .expect("a streaming response")
}

/// A body whose chunk boundaries fall inside multi-byte characters.
///
/// Each piece is one byte, so every character above ASCII is split across
/// reads. Decoding a read on its own would replace each half with U+FFFD,
/// silently and most often on exactly the text a model is generating.
async fn handle_stream_split() -> impl IntoResponse {
    // Three characters of three bytes each, plus one four-byte emoji.
    let text = "日本語🙂";
    let bytes: Vec<u8> = text.bytes().collect();

    let body = async_stream::stream! {
        for byte in bytes {
            tokio::time::sleep(tokio::time::Duration::from_millis(2)).await;
            yield Ok::<_, std::io::Error>(axum::body::Bytes::from(vec![byte]));
        }
    };

    axum::response::Response::builder()
        .status(200)
        .header("content-type", "text/plain; charset=utf-8")
        .body(axum::body::Body::from_stream(body))
        .expect("a streaming response")
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
        let response = client.get(server.url("/get")).send().await.unwrap();

        assert_eq!(response.status(), 200);
        server.shutdown().await;
    }

    #[tokio::test]
    async fn test_mock_server_post() {
        let server = MockServer::start().await.unwrap();

        let client = reqwest::Client::new();
        let response = client
            .post(server.url("/post"))
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
        let response = client.get(server.url("/status/404")).send().await.unwrap();

        assert_eq!(response.status(), 404);
        server.shutdown().await;
    }
}
