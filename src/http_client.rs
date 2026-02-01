//! HTTP Client Module
//!
//! Provides secure HTTP client functionality for making external API calls from JavaScript.
//! Implements the Web Fetch API with secret injection for secure API key handling.
//!
//! # Security Features
//!
//! 1. Secret injection via template syntax: `{{secret:identifier}}`
//! 2. URL validation to block private IPs and localhost
//! 3. Response size limits to prevent memory exhaustion
//! 4. Timeout enforcement for all requests
//! 5. TLS/SSL certificate validation
//! 6. Audit logging for secret access

use crate::secrets::get_global_secrets_manager;
use reqwest::Method;
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::IpAddr;
use std::str::FromStr;
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, info};
use url::Url;

/// Maximum response size (10MB)
const MAX_RESPONSE_SIZE: usize = 10 * 1024 * 1024;

/// Default request timeout (30 seconds)
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// HTTP client for making external requests
pub struct HttpClient {
    client: reqwest::blocking::Client,
    default_timeout: Duration,
    max_response_size: usize,
    /// Allow localhost/private IPs (test mode only)
    allow_private: bool,
}

impl HttpClient {
    /// Create a new HTTP client with default settings
    pub fn new() -> Result<Self, HttpError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .use_rustls_tls()
            .build()
            .map_err(|e| HttpError::ClientInitialization(e.to_string()))?;

        Ok(Self {
            client,
            default_timeout: DEFAULT_TIMEOUT,
            max_response_size: MAX_RESPONSE_SIZE,
            allow_private: false,
        })
    }

    /// Create a new HTTP client for testing (allows localhost/private IPs)
    /// Only use this for test purposes!
    #[doc(hidden)]
    pub fn new_for_tests() -> Result<Self, HttpError> {
        let client = reqwest::blocking::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .use_rustls_tls()
            .danger_accept_invalid_certs(true) // For testing with self-signed certs
            .build()
            .map_err(|e| HttpError::ClientInitialization(e.to_string()))?;

        Ok(Self {
            client,
            default_timeout: DEFAULT_TIMEOUT,
            max_response_size: MAX_RESPONSE_SIZE,
            allow_private: true,
        })
    }

    /// Make an HTTP request with the Fetch API interface
    ///
    /// Supports secret injection via `{{secret:identifier}}` template syntax in headers.
    /// Validates secret access based on target URL and script URI constraints.
    pub fn fetch(
        &self,
        url: String,
        options: FetchOptions,
        script_uri: Option<&str>,
    ) -> Result<FetchResponse, HttpError> {
        // Validate URL
        let parsed_url = if self.allow_private {
            Self::validate_url_test(&url)?
        } else {
            Self::validate_url(&url)?
        };

        // Parse HTTP method
        let method = Method::from_str(&options.method.to_uppercase())
            .map_err(|_| HttpError::InvalidMethod(options.method.clone()))?;

        // Process headers and inject secrets
        let headers = self.process_headers(options.headers, &url, script_uri)?;

        // Build request
        let timeout = options
            .timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(self.default_timeout);

        let mut request = self
            .client
            .request(method, parsed_url.as_str())
            .headers(headers);

        // Add body if provided
        if let Some(body) = options.body {
            request = request.body(body);
        }

        // Set timeout
        request = request.timeout(timeout);

        // Execute request
        debug!("Fetching URL: {} with method: {}", url, options.method);
        let response = request
            .send()
            .map_err(|e| HttpError::RequestFailed(e.to_string()))?;

        // Convert to FetchResponse
        self.convert_response(response)
    }

    /// Validate URL and block private IPs, localhost, and malicious URLs
    fn validate_url(url: &str) -> Result<Url, HttpError> {
        let parsed = Url::parse(url).map_err(|e| HttpError::InvalidUrl(e.to_string()))?;

        // Only allow HTTP and HTTPS
        match parsed.scheme() {
            "http" | "https" => {}
            _ => return Err(HttpError::InvalidUrlScheme(parsed.scheme().to_string())),
        }

        // Get host
        let host = parsed
            .host_str()
            .ok_or_else(|| HttpError::InvalidUrl("No host in URL".to_string()))?;

        // Check for localhost
        if host == "localhost" || host.ends_with(".localhost") {
            return Err(HttpError::BlockedUrl(
                "Localhost is not allowed".to_string(),
            ));
        }

        // Check for IP addresses
        if let Ok(ip) = IpAddr::from_str(host)
            && Self::is_private_ip(&ip)
        {
            return Err(HttpError::BlockedUrl(format!(
                "Private IP address not allowed: {}",
                ip
            )));
        }

        // Check for internal domains (optional, can be extended)
        if host == "127.0.0.1"
            || host == "0.0.0.0"
            || host.starts_with("10.")
            || host.starts_with("172.16.")
            || host.starts_with("192.168.")
        {
            return Err(HttpError::BlockedUrl(format!(
                "Internal address not allowed: {}",
                host
            )));
        }

        Ok(parsed)
    }

    /// Validate URL for testing (allows localhost and private IPs)
    fn validate_url_test(url: &str) -> Result<Url, HttpError> {
        let parsed = Url::parse(url).map_err(|e| HttpError::InvalidUrl(e.to_string()))?;

        // Only allow HTTP and HTTPS
        match parsed.scheme() {
            "http" | "https" => {}
            _ => return Err(HttpError::InvalidUrlScheme(parsed.scheme().to_string())),
        }

        // Get host - just check it exists
        let _host = parsed
            .host_str()
            .ok_or_else(|| HttpError::InvalidUrl("No host in URL".to_string()))?;

        // In test mode, allow all hosts including localhost and private IPs
        Ok(parsed)
    }

    /// Check if an IP address is private
    fn is_private_ip(ip: &IpAddr) -> bool {
        match ip {
            IpAddr::V4(ipv4) => {
                ipv4.is_private()
                    || ipv4.is_loopback()
                    || ipv4.is_link_local()
                    || ipv4.is_broadcast()
                    || ipv4.is_documentation()
                    || ipv4.is_unspecified()
            }
            IpAddr::V6(ipv6) => ipv6.is_loopback() || ipv6.is_unspecified(),
        }
    }

    /// Process headers and inject secrets with constraint validation
    fn process_headers(
        &self,
        headers: Option<HashMap<String, String>>,
        url: &str,
        script_uri: Option<&str>,
    ) -> Result<HeaderMap, HttpError> {
        let mut header_map = HeaderMap::new();

        if let Some(headers) = headers {
            for (key, value) in headers {
                let final_value = if value.starts_with("{{secret:") && value.ends_with("}}") {
                    // Extract secret identifier
                    let secret_id = value
                        .strip_prefix("{{secret:")
                        .unwrap()
                        .strip_suffix("}}")
                        .unwrap()
                        .trim();

                    // Get secrets manager
                    let secrets_manager =
                        get_global_secrets_manager().ok_or(HttpError::SecretsNotInitialized)?;

                    // Look up secret with constraint validation
                    let secret_value = secrets_manager
                        .get_with_constraints(secret_id, url, script_uri)
                        .map_err(|e| match e {
                            crate::secrets::SecretAccessError::NotFound(id) => {
                                HttpError::SecretNotFound(id)
                            }
                            crate::secrets::SecretAccessError::UrlConstraintViolation {
                                secret_id,
                                attempted_url,
                            } => HttpError::SecretUrlConstraintViolation {
                                secret_id,
                                attempted_url,
                            },
                            crate::secrets::SecretAccessError::ScriptConstraintViolation {
                                secret_id,
                                script_uri,
                            } => HttpError::SecretScriptConstraintViolation {
                                secret_id,
                                script_uri,
                            },
                        })?;

                    // Audit log (identifier only, never value)
                    info!(
                        secret_id = secret_id,
                        url = url,
                        script_uri = ?script_uri,
                        "Secret accessed in fetch request"
                    );

                    secret_value
                } else {
                    value
                };

                // Add to header map
                let header_name = reqwest::header::HeaderName::from_str(&key)
                    .map_err(|e| HttpError::InvalidHeader(format!("Invalid header name: {}", e)))?;
                let header_value =
                    reqwest::header::HeaderValue::from_str(&final_value).map_err(|e| {
                        HttpError::InvalidHeader(format!("Invalid header value: {}", e))
                    })?;

                header_map.insert(header_name, header_value);
            }
        }

        Ok(header_map)
    }

    /// Convert reqwest response to FetchResponse
    fn convert_response(
        &self,
        response: reqwest::blocking::Response,
    ) -> Result<FetchResponse, HttpError> {
        let status = response.status().as_u16();
        let ok = response.status().is_success();

        // Extract headers
        let mut headers = HashMap::new();
        for (key, value) in response.headers() {
            if let Ok(value_str) = value.to_str() {
                headers.insert(key.to_string(), value_str.to_string());
            }
        }

        // Check content length
        if let Some(content_length) = response.content_length()
            && content_length > self.max_response_size as u64
        {
            return Err(HttpError::ResponseTooLarge(content_length));
        }

        // Get response body
        let bytes = response
            .bytes()
            .map_err(|e| HttpError::ResponseReadFailed(e.to_string()))?;

        // Check size after reading
        if bytes.len() > self.max_response_size {
            return Err(HttpError::ResponseTooLarge(bytes.len() as u64));
        }

        // Convert to string (UTF-8)
        let body = String::from_utf8(bytes.to_vec())
            .map_err(|e| HttpError::ResponseEncodingError(e.to_string()))?;

        Ok(FetchResponse {
            status,
            headers,
            body,
            ok,
        })
    }
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new().expect("Failed to create HTTP client")
    }
}

/// Options for fetch request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchOptions {
    /// HTTP method (GET, POST, PUT, DELETE, etc.)
    #[serde(default = "default_method")]
    pub method: String,

    /// Request headers (supports {{secret:name}} template syntax)
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,

    /// Request body
    #[serde(default)]
    pub body: Option<String>,

    /// Timeout in milliseconds
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

fn default_method() -> String {
    "GET".to_string()
}

impl Default for FetchOptions {
    fn default() -> Self {
        Self {
            method: default_method(),
            headers: None,
            body: None,
            timeout_ms: None,
        }
    }
}

/// Response from fetch request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchResponse {
    /// HTTP status code
    pub status: u16,

    /// Response headers
    pub headers: HashMap<String, String>,

    /// Response body as string
    pub body: String,

    /// Whether the request was successful (2xx status)
    pub ok: bool,
}

impl FetchResponse {
    /// Parse JSON response body
    pub fn json<T: for<'de> Deserialize<'de>>(&self) -> Result<T, serde_json::Error> {
        serde_json::from_str(&self.body)
    }

    /// Get response body as text
    pub fn text(&self) -> &str {
        &self.body
    }
}

/// HTTP client errors
#[derive(Debug, Error)]
pub enum HttpError {
    #[error("Failed to initialize HTTP client: {0}")]
    ClientInitialization(String),

    #[error("Invalid URL: {0}")]
    InvalidUrl(String),

    #[error("Invalid URL scheme: {0} (only http and https are allowed)")]
    InvalidUrlScheme(String),

    #[error("Blocked URL: {0}")]
    BlockedUrl(String),

    #[error("Invalid HTTP method: {0}")]
    InvalidMethod(String),

    #[error("Invalid header: {0}")]
    InvalidHeader(String),

    #[error("Secret not found: {0}")]
    SecretNotFound(String),

    #[error("Secret '{secret_id}' not allowed for URL: {attempted_url}")]
    SecretUrlConstraintViolation {
        secret_id: String,
        attempted_url: String,
    },

    #[error("Secret '{secret_id}' not allowed for script: {script_uri}")]
    SecretScriptConstraintViolation {
        secret_id: String,
        script_uri: String,
    },

    #[error("Secrets manager not initialized")]
    SecretsNotInitialized,

    #[error("Request failed: {0}")]
    RequestFailed(String),

    #[error("Response too large: {0} bytes (max {MAX_RESPONSE_SIZE})")]
    ResponseTooLarge(u64),

    #[error("Failed to read response: {0}")]
    ResponseReadFailed(String),

    #[error("Response encoding error: {0}")]
    ResponseEncodingError(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_url_valid_https() {
        let result = HttpClient::validate_url("https://api.example.com/v1/test");
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_url_valid_http() {
        let result = HttpClient::validate_url("http://api.example.com");
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_url_blocks_localhost() {
        let result = HttpClient::validate_url("https://localhost/api");
        assert!(matches!(result, Err(HttpError::BlockedUrl(_))));
    }

    #[test]
    fn test_validate_url_blocks_127001() {
        let result = HttpClient::validate_url("http://127.0.0.1:8080/api");
        assert!(matches!(result, Err(HttpError::BlockedUrl(_))));
    }

    #[test]
    fn test_validate_url_blocks_private_ip() {
        let result = HttpClient::validate_url("http://192.168.1.1/api");
        assert!(matches!(result, Err(HttpError::BlockedUrl(_))));
    }

    #[test]
    fn test_validate_url_blocks_10_network() {
        let result = HttpClient::validate_url("http://10.0.0.1/api");
        assert!(matches!(result, Err(HttpError::BlockedUrl(_))));
    }

    #[test]
    fn test_validate_url_invalid_scheme() {
        let result = HttpClient::validate_url("ftp://example.com");
        assert!(matches!(result, Err(HttpError::InvalidUrlScheme(_))));
    }

    #[test]
    fn test_validate_url_file_scheme() {
        let result = HttpClient::validate_url("file:///etc/passwd");
        assert!(matches!(result, Err(HttpError::InvalidUrlScheme(_))));
    }

    #[test]
    fn test_is_private_ip() {
        assert!(HttpClient::is_private_ip(&"127.0.0.1".parse().unwrap()));
        assert!(HttpClient::is_private_ip(&"192.168.1.1".parse().unwrap()));
        assert!(HttpClient::is_private_ip(&"10.0.0.1".parse().unwrap()));
        assert!(HttpClient::is_private_ip(&"172.16.0.1".parse().unwrap()));
        assert!(!HttpClient::is_private_ip(&"8.8.8.8".parse().unwrap()));
        assert!(!HttpClient::is_private_ip(&"1.1.1.1".parse().unwrap()));
    }
}
