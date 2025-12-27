use axum::body::Body;
use axum::http::StatusCode;
use futures_util::TryStreamExt;
use multer::Multipart;
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_urlencoded;
use std::collections::HashMap;
use tracing::error;

/// Represents an uploaded file from multipart form data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadedFile {
    pub field_name: String,
    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub data: Vec<u8>,
    pub size: usize,
}

/// Parse form data based on content type.
/// Supports application/x-www-form-urlencoded and multipart/form-data.
/// Returns 413 status code if any file exceeds the size limit.
pub async fn parse_form_data(
    content_type: Option<&str>,
    body: Body,
    max_upload_size: usize,
) -> Result<(HashMap<String, String>, Vec<UploadedFile>), StatusCode> {
    if let Some(ct) = content_type {
        if ct.starts_with("application/x-www-form-urlencoded") {
            // Convert body to bytes and parse as URL-encoded form data
            match axum::body::to_bytes(body, usize::MAX).await {
                Ok(bytes) => {
                    let body_str =
                        String::from_utf8(bytes.to_vec()).map_err(|_| StatusCode::BAD_REQUEST)?;
                    let fields: HashMap<String, String> =
                        serde_urlencoded::from_str(&body_str).unwrap_or_default();
                    Ok((fields, Vec::new()))
                }
                Err(e) => {
                    error!("Failed to read form data body: {}", e);
                    Err(StatusCode::BAD_REQUEST)
                }
            }
        } else if ct.starts_with("multipart/form-data") {
            // Extract boundary from content type
            let boundary = multer::parse_boundary(ct).map_err(|_| StatusCode::BAD_REQUEST)?;

            // Convert Body to Stream for multer
            let stream = body.into_data_stream().map_err(|e| {
                error!("Failed to convert body to stream: {}", e);
                std::io::Error::other(e)
            });

            let mut multipart = Multipart::new(stream, boundary);
            let mut fields = HashMap::new();
            let mut files = Vec::new();

            // Filename validation regex: only alphanumeric, underscore, hyphen, and dot
            let filename_regex = Regex::new(r"^[a-zA-Z0-9_\-\.]+$").unwrap();

            while let Some(field) = multipart.next_field().await.map_err(|e| {
                error!("Failed to read multipart field: {}", e);
                StatusCode::BAD_REQUEST
            })? {
                let field_name = field.name().unwrap_or("").to_string();
                let filename = field.file_name().map(|s| s.to_string());
                let content_type = field.content_type().map(|m| m.to_string());

                // Read field data
                let data = field.bytes().await.map_err(|e| {
                    error!("Failed to read field data: {}", e);
                    StatusCode::BAD_REQUEST
                })?;

                let size = data.len();

                // Check size limit
                if size > max_upload_size {
                    error!(
                        "File upload exceeds size limit: {} bytes (max: {} bytes)",
                        size, max_upload_size
                    );
                    return Err(StatusCode::PAYLOAD_TOO_LARGE);
                }

                // If it's a file upload (has filename)
                if let Some(ref fname) = filename {
                    // Validate filename: check for path traversal and invalid characters
                    if fname.is_empty()
                        || fname.contains("..")
                        || fname.contains('/')
                        || fname.contains('\\')
                        || !filename_regex.is_match(fname)
                    {
                        error!("Invalid filename: {}", fname);
                        return Err(StatusCode::BAD_REQUEST);
                    }

                    files.push(UploadedFile {
                        field_name: field_name.clone(),
                        filename,
                        content_type,
                        data: data.to_vec(),
                        size,
                    });
                } else {
                    // Regular form field (text)
                    if let Ok(value) = String::from_utf8(data.to_vec()) {
                        fields.insert(field_name, value);
                    }
                }
            }

            Ok((fields, files))
        } else {
            Ok((HashMap::new(), Vec::new()))
        }
    } else {
        Ok((HashMap::new(), Vec::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;

    #[tokio::test]
    async fn test_parse_form_data_urlencoded() {
        let body = Body::from("a=1&b=hello+world");
        let result = parse_form_data(
            Some("application/x-www-form-urlencoded"),
            body,
            10 * 1024 * 1024,
        )
        .await
        .unwrap();
        assert_eq!(result.0.get("a"), Some(&"1".to_string()));
        assert_eq!(result.0.get("b"), Some(&"hello world".to_string()));
        assert!(result.1.is_empty());
    }

    #[tokio::test]
    async fn test_parse_form_data_multipart() {
        let body = Body::from("ignored");
        // Note: This is a simplified test - real multipart requires proper formatting
        // In practice, multipart parsing will fail on invalid data
        let result = parse_form_data(
            Some("multipart/form-data; boundary=abc"),
            body,
            10 * 1024 * 1024,
        )
        .await;
        // Expect error on malformed multipart data
        assert!(result.is_err());
    }
}
