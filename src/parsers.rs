use axum::body::Body;
use serde_urlencoded;
use std::collections::HashMap;
use tracing::error;

/// Parse form data based on content type.
/// Supports application/x-www-form-urlencoded and returns an empty map for multipart
/// (multipart handling is intentionally left out - it's more complex and should use a library).
pub async fn parse_form_data(
    content_type: Option<&str>,
    body: Body,
) -> Option<HashMap<String, String>> {
    if let Some(ct) = content_type {
        if ct.starts_with("application/x-www-form-urlencoded") {
            // Convert body to bytes and parse as URL-encoded form data
            match axum::body::to_bytes(body, usize::MAX).await {
                Ok(bytes) => {
                    let body_str = String::from_utf8(bytes.to_vec()).ok()?;
                    Some(serde_urlencoded::from_str(&body_str).unwrap_or_default())
                }
                Err(e) => {
                    error!("Failed to read form data body: {}", e);
                    None
                }
            }
        } else if ct.starts_with("multipart/form-data") {
            // multipart parsing omitted - return empty map for now
            Some(HashMap::new())
        } else {
            None
        }
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;

    #[tokio::test]
    async fn test_parse_form_data_urlencoded() {
        let body = Body::from("a=1&b=hello+world");
        let result = parse_form_data(Some("application/x-www-form-urlencoded"), body)
            .await
            .unwrap();
        assert_eq!(result.get("a"), Some(&"1".to_string()));
        assert_eq!(result.get("b"), Some(&"hello world".to_string()));
    }

    #[tokio::test]
    async fn test_parse_form_data_multipart() {
        let body = Body::from("ignored");
        let result = parse_form_data(Some("multipart/form-data; boundary=abc"), body)
            .await
            .unwrap();
        assert!(result.is_empty());
    }
}
