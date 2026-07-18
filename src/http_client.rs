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

use reqwest::Method;
use reqwest::header::HeaderMap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::{IpAddr, ToSocketAddrs};
use std::str::FromStr;
use std::sync::OnceLock;
use std::time::Duration;
use thiserror::Error;
use tracing::{debug, info};
use url::Url;

/// Maximum response size (10MB)
const MAX_RESPONSE_SIZE: usize = 10 * 1024 * 1024;

/// Default request timeout (30 seconds)
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum redirects followed per fetch (each hop is re-validated)
const MAX_REDIRECTS: usize = 5;

/// Shared connection-pooled client. Redirects are disabled: `fetch` follows
/// them manually so every hop gets URL and DNS validation (a public URL
/// redirecting to an internal address must be blocked).
fn shared_client() -> Result<&'static reqwest::blocking::Client, HttpError> {
    static CLIENT: OnceLock<Result<reqwest::blocking::Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::blocking::Client::builder()
                .timeout(DEFAULT_TIMEOUT)
                .use_rustls_tls()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|e| HttpError::ClientInitialization(e.clone()))
}

/// Shared client for tests: follows redirects automatically and accepts
/// self-signed certificates.
fn shared_test_client() -> Result<&'static reqwest::blocking::Client, HttpError> {
    static CLIENT: OnceLock<Result<reqwest::blocking::Client, String>> = OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::blocking::Client::builder()
                .timeout(DEFAULT_TIMEOUT)
                .use_rustls_tls()
                .danger_accept_invalid_certs(true)
                .build()
                .map_err(|e| e.to_string())
        })
        .as_ref()
        .map_err(|e| HttpError::ClientInitialization(e.clone()))
}

/// HTTP client for making external requests. Cheap to construct: the
/// underlying reqwest client (connection pool) is shared process-wide.
pub struct HttpClient {
    default_timeout: Duration,
    max_response_size: usize,
    /// Allow localhost/private IPs (test mode only)
    allow_private: bool,
    /// Follow redirects manually, validating every hop
    manual_redirects: bool,
}

impl HttpClient {
    /// Create a new HTTP client with default settings
    pub fn new() -> Result<Self, HttpError> {
        Ok(Self {
            default_timeout: DEFAULT_TIMEOUT,
            max_response_size: MAX_RESPONSE_SIZE,
            allow_private: false,
            manual_redirects: true,
        })
    }

    /// Create a new HTTP client for testing (allows localhost/private IPs)
    /// Only use this for test purposes!
    #[doc(hidden)]
    pub fn new_for_tests() -> Result<Self, HttpError> {
        Ok(Self {
            default_timeout: DEFAULT_TIMEOUT,
            max_response_size: MAX_RESPONSE_SIZE,
            allow_private: true,
            manual_redirects: false,
        })
    }

    /// Test-only client that exercises the manual redirect loop against
    /// localhost mock servers.
    #[doc(hidden)]
    pub fn new_for_redirect_tests() -> Result<Self, HttpError> {
        Ok(Self {
            default_timeout: DEFAULT_TIMEOUT,
            max_response_size: MAX_RESPONSE_SIZE,
            allow_private: true,
            manual_redirects: true,
        })
    }

    fn validate(&self, url: &str) -> Result<Url, HttpError> {
        if self.allow_private {
            Self::validate_url_test(url)
        } else {
            Self::validate_url(url)
        }
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
        user_id: Option<&str>,
    ) -> Result<FetchResponse, HttpError> {
        // Parse HTTP method
        let method = Method::from_str(&options.method.to_uppercase())
            .map_err(|_| HttpError::InvalidMethod(options.method.clone()))?;

        // Process headers and inject secrets
        let headers = self.process_headers(options.headers, &url, script_uri, user_id)?;

        let timeout = options
            .timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(self.default_timeout);

        debug!("Fetching URL: {} with method: {}", url, options.method);

        if !self.manual_redirects {
            // Test mode: single request through the redirect-following client
            let parsed_url = self.validate(&url)?;
            let mut request = shared_test_client()?
                .request(method, parsed_url.as_str())
                .headers(headers)
                .timeout(timeout);
            if let Some(body) = options.body {
                request = request.body(body);
            }
            let response = request
                .send()
                .map_err(|e| HttpError::RequestFailed(e.to_string()))?;
            return self.convert_response(response);
        }

        // Follow redirects manually so every hop is validated (URL scheme,
        // host, and DNS resolution). The shared client has redirects disabled.
        let client = shared_client()?;
        let mut current_url = self.validate(&url)?;
        let mut current_method = method;
        let mut current_body = options.body;
        let mut current_headers = headers;

        for _ in 0..=MAX_REDIRECTS {
            let mut request = client
                .request(current_method.clone(), current_url.as_str())
                .headers(current_headers.clone())
                .timeout(timeout);
            if let Some(body) = &current_body {
                request = request.body(body.clone());
            }

            let response = request
                .send()
                .map_err(|e| HttpError::RequestFailed(e.to_string()))?;

            let status = response.status();
            let is_redirect = matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308);
            if !is_redirect {
                return self.convert_response(response);
            }

            let Some(location) = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
            else {
                // Redirect status without a Location header: return as-is
                return self.convert_response(response);
            };

            // Resolve relative redirects against the current URL, then apply
            // the same validation as the original request
            let next_url = current_url
                .join(location)
                .map_err(|e| HttpError::InvalidUrl(format!("Invalid redirect target: {}", e)))?;
            let next_url = self.validate(next_url.as_str())?;

            // 301/302/303 switch non-GET/HEAD methods to GET and drop the
            // body (browser/fetch semantics); 307/308 preserve both
            if matches!(status.as_u16(), 301..=303)
                && current_method != Method::GET
                && current_method != Method::HEAD
            {
                current_method = Method::GET;
                current_body = None;
            }

            // Strip credentials when the redirect changes host, mirroring
            // reqwest's own redirect policy
            if next_url.host_str() != current_url.host_str() {
                current_headers.remove(reqwest::header::AUTHORIZATION);
                current_headers.remove(reqwest::header::COOKIE);
                current_headers.remove(reqwest::header::PROXY_AUTHORIZATION);
                current_headers.remove(reqwest::header::WWW_AUTHENTICATE);
            }

            debug!("Following redirect ({}) to {}", status.as_u16(), next_url);
            current_url = next_url;
        }

        Err(HttpError::RequestFailed(format!(
            "Too many redirects (max {})",
            MAX_REDIRECTS
        )))
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

        // Check for IP address literals
        if let Ok(ip) = IpAddr::from_str(host.trim_start_matches('[').trim_end_matches(']')) {
            if Self::is_private_ip(&ip) {
                return Err(HttpError::BlockedUrl(format!(
                    "Private IP address not allowed: {}",
                    ip
                )));
            }
        } else {
            // Hostname: resolve it and validate every address it maps to,
            // blocking DNS-based SSRF (a public name resolving to e.g.
            // 10.0.0.5). Resolution failures are left for the request itself
            // to surface — the connection uses the same resolver and would
            // fail identically. Note: a TTL-0 DNS-rebinding window between
            // this check and the connection remains; closing it would require
            // pinning the connection to the validated address.
            let port = parsed.port_or_known_default().unwrap_or(443);
            if let Ok(addrs) = (host, port).to_socket_addrs() {
                for addr in addrs {
                    if Self::is_private_ip(&addr.ip()) {
                        return Err(HttpError::BlockedUrl(format!(
                            "Host '{}' resolves to blocked address {}",
                            host,
                            addr.ip()
                        )));
                    }
                }
            }
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

    /// Check if an IP address is private, loopback, link-local, or otherwise
    /// not a legitimate public destination for script-initiated requests
    fn is_private_ip(ip: &IpAddr) -> bool {
        match ip {
            IpAddr::V4(ipv4) => {
                let octets = ipv4.octets();
                ipv4.is_private()
                    || ipv4.is_loopback()
                    || ipv4.is_link_local()
                    || ipv4.is_broadcast()
                    || ipv4.is_documentation()
                    || ipv4.is_unspecified()
                    // Carrier-grade NAT range 100.64.0.0/10 (RFC 6598)
                    || (octets[0] == 100 && (octets[1] & 0xC0) == 64)
            }
            IpAddr::V6(ipv6) => {
                // IPv4-mapped IPv6 (::ffff:10.0.0.5) must not bypass V4 rules
                if let Some(mapped) = ipv6.to_ipv4_mapped() {
                    return Self::is_private_ip(&IpAddr::V4(mapped));
                }
                ipv6.is_loopback()
                    || ipv6.is_unspecified()
                    || ipv6.is_unique_local()
                    || ipv6.is_unicast_link_local()
            }
        }
    }

    /// Process headers and inject secrets, looking up values from the database.
    /// Checks `user_secrets` first (when `user_id` is given), then `script_secrets`.
    /// Environment variables and config files are never consulted.
    fn process_headers(
        &self,
        headers: Option<HashMap<String, String>>,
        url: &str,
        script_uri: Option<&str>,
        user_id: Option<&str>,
    ) -> Result<HeaderMap, HttpError> {
        let mut header_map = HeaderMap::new();

        if let Some(headers) = headers {
            for (key, value) in headers {
                let final_value = if value.starts_with("{{secret:") && value.ends_with("}}") {
                    // Extract secret identifier — guarded by starts_with/ends_with above
                    let secret_id = value["{{secret:".len()..value.len() - "}}".len()].trim();

                    // Look up secret from database: user_secrets first, then script_secrets.
                    // Environment variables and config files are never consulted.
                    let secret_value = crate::repository::resolve_secret_db(
                        script_uri.unwrap_or(""),
                        secret_id,
                        user_id,
                    )
                    .ok_or_else(|| HttpError::SecretNotFound(secret_id.to_string()))?;

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

        // Read the body with a hard cap so responses without a Content-Length
        // header (e.g. chunked) cannot buffer unbounded memory
        use std::io::Read;
        let mut bytes = Vec::new();
        response
            .take(self.max_response_size as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| HttpError::ResponseReadFailed(e.to_string()))?;

        if bytes.len() > self.max_response_size {
            return Err(HttpError::ResponseTooLarge(bytes.len() as u64));
        }

        // Convert to string (UTF-8)
        let body = String::from_utf8(bytes)
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

    #[test]
    fn test_is_private_ip_cgnat_range() {
        // RFC 6598 carrier-grade NAT: 100.64.0.0/10
        assert!(HttpClient::is_private_ip(&"100.64.0.1".parse().unwrap()));
        assert!(HttpClient::is_private_ip(
            &"100.127.255.254".parse().unwrap()
        ));
        assert!(!HttpClient::is_private_ip(
            &"100.63.255.255".parse().unwrap()
        ));
        assert!(!HttpClient::is_private_ip(&"100.128.0.1".parse().unwrap()));
    }

    #[test]
    fn test_is_private_ip_ipv6_ranges() {
        assert!(HttpClient::is_private_ip(&"::1".parse().unwrap()));
        // Unique-local fc00::/7
        assert!(HttpClient::is_private_ip(&"fd00::1".parse().unwrap()));
        // Link-local fe80::/10
        assert!(HttpClient::is_private_ip(&"fe80::1".parse().unwrap()));
        // Public IPv6 stays allowed
        assert!(!HttpClient::is_private_ip(
            &"2606:4700:4700::1111".parse().unwrap()
        ));
    }

    #[test]
    fn test_is_private_ip_v4_mapped_v6_no_bypass() {
        // ::ffff:10.0.0.5 must be treated as the private 10.0.0.5, not as a
        // public IPv6 address
        assert!(HttpClient::is_private_ip(
            &"::ffff:10.0.0.5".parse().unwrap()
        ));
        assert!(HttpClient::is_private_ip(
            &"::ffff:127.0.0.1".parse().unwrap()
        ));
        assert!(!HttpClient::is_private_ip(
            &"::ffff:8.8.8.8".parse().unwrap()
        ));
    }

    #[test]
    fn test_validate_url_blocks_v4_mapped_v6_literal() {
        let result = HttpClient::validate_url("http://[::ffff:10.0.0.5]/api");
        assert!(matches!(result, Err(HttpError::BlockedUrl(_))));
    }
}
