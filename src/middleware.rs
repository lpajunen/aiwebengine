use axum::{
    extract::Request,
    http::HeaderMap,
    middleware::Next,
    response::Response,
};
use std::sync::atomic::{AtomicU64, Ordering};
use tracing::debug;

/// Global counter for generating request IDs
static REQUEST_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Header name for request ID
pub const REQUEST_ID_HEADER: &str = "x-request-id";

/// Middleware that generates and injects request IDs
pub async fn request_id_middleware(request: Request, next: Next) -> Response {
    let request_id = generate_request_id();

    // Add request ID to request extensions for use in handlers
    let mut request = request;
    request.extensions_mut().insert(RequestId(request_id.clone()));

    // Add request ID to response headers
    let mut response = next.run(request).await;

    if let Ok(header_value) = axum::http::HeaderValue::from_str(&request_id) {
        response.headers_mut().insert(
            axum::http::header::HeaderName::from_static(REQUEST_ID_HEADER),
            header_value,
        );
    }

    debug!("Request {} processed", request_id);
    response
}

/// Generate a unique request ID
pub fn generate_request_id() -> String {
    let counter = REQUEST_COUNTER.fetch_add(1, Ordering::SeqCst);
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();

    format!("req_{}_{}", timestamp, counter)
}

/// Extract request ID from request headers or generate a new one
pub fn extract_or_generate_request_id(headers: &HeaderMap) -> String {
    if let Some(request_id) = headers.get(REQUEST_ID_HEADER) {
        if let Ok(id) = request_id.to_str() {
            return id.to_string();
        }
    }

    generate_request_id()
}

/// Type for storing request ID in request extensions
#[derive(Clone, Debug)]
pub struct RequestId(pub String);

/// Helper trait to get request ID from various sources
pub trait HasRequestId {
    fn request_id(&self) -> &str;
}

impl HasRequestId for RequestId {
    fn request_id(&self) -> &str {
        &self.0
    }
}

impl HasRequestId for &RequestId {
    fn request_id(&self) -> &str {
        &self.0
    }
}

impl HasRequestId for String {
    fn request_id(&self) -> &str {
        self
    }
}

impl HasRequestId for &str {
    fn request_id(&self) -> &str {
        self
    }
}