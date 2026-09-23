//! HTTP Client Module
//!
//! Provides secure HTTP client functionality for making external API calls from JavaScript.
//! Implements the Web Fetch API with secret injection for secure API key handling.
//!
//! # Security Features
//!
//! 1. Secret injection via template syntax: `{{secret:identifier}}`, anywhere
//!    within a header value
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
pub const MAX_RESPONSE_SIZE: usize = 10 * 1024 * 1024;

/// Default request timeout (30 seconds)
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// Maximum redirects followed per fetch (each hop is re-validated)
pub const MAX_REDIRECTS: usize = 5;

/// Content codings this client asks for and can undo.
///
/// Advertising exactly what [`decode_body`] implements is the whole contract:
/// a server may only use a coding the request offered, so naming `br` or
/// `zstd` here without being able to inflate them would hand a script a body
/// it cannot read. `gzip` and `deflate` are what `flate2` — already a
/// dependency, for the git archives — gives us.
pub const SUPPORTED_ENCODINGS: &str = "gzip, deflate";

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
#[derive(Debug, Clone, Copy)]
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
    /// Header values are rendered through [`substitute_secrets`] first, so a
    /// `{{secret:identifier}}` anywhere in one is replaced by the secret it
    /// names — `user_secrets` for `user_id` when there is one, then
    /// `script_secrets` for `script_uri`. Which secrets are reachable is
    /// decided by that pair and by nothing about the URL.
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

        // The URL first, so what follows logs the template rather than the
        // credential. `display` is what every line below writes down.
        let resolved = resolve_url(&url, script_uri, user_id)?;
        for secret_id in &resolved.names {
            info!(
                secret_id = %secret_id,
                url = %resolved.display,
                script_uri = ?script_uri,
                "Secret accessed in fetch URL"
            );
        }

        // Process headers and inject secrets
        let mut headers =
            self.process_headers(options.headers, &resolved.display, script_uri, user_id)?;

        // Offer the codings we can undo. A caller that named its own is left
        // alone — some APIs answer `406` unless the request carries a
        // particular `Accept-Encoding`, so the header is a thing a script has
        // a reason to set, and what comes back is decoded either way because
        // `convert_response` reads the response's `Content-Encoding` rather
        // than remembering what was asked for.
        if !headers.contains_key(reqwest::header::ACCEPT_ENCODING) {
            headers.insert(
                reqwest::header::ACCEPT_ENCODING,
                reqwest::header::HeaderValue::from_static(SUPPORTED_ENCODINGS),
            );
        }

        let timeout = options
            .timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(self.default_timeout);

        debug!(
            "Fetching URL: {} with method: {}",
            resolved.display, options.method
        );

        let response = self
            .send_request(
                method,
                &resolved.target,
                headers,
                options.body,
                timeout,
                &resolved,
            )
            .map_err(|e| resolved.scrub_error(e))?;
        self.convert_response(response, options.binary)
    }

    /// Fetch a URL and return its body as bytes.
    ///
    /// [`HttpClient::fetch`] decodes to `String` and fails on anything that is
    /// not UTF-8, which is right for the JavaScript `fetch` it backs and wrong
    /// for an archive. Everything ahead of that decode is shared: the same URL
    /// checks, the same DNS checks, the same manually validated redirect loop.
    /// That sharing is the point — routing git traffic through this client
    /// rather than a reqwest of its own is worth nothing if the validation
    /// differs between the two entry points.
    ///
    /// `max_bytes` is the caller's ceiling rather than this client's: the 10MB
    /// bounding a script's `fetch` is not the right bound for a repository
    /// archive, and the caller is the only one that knows what it is reading.
    ///
    /// It also neither offers nor undoes a content coding, which the text path
    /// does: a `.tar.gz` is gzip as *content*, and a client that inflated
    /// bodies on this path would hand [`crate::git_sync::extract_tree`] a bare
    /// tar its own decoder cannot read. Bytes here means the bytes that came.
    pub fn fetch_bytes(
        &self,
        url: &str,
        headers: Option<HashMap<String, String>>,
        max_bytes: usize,
    ) -> Result<BytesResponse, HttpError> {
        let headers = self.process_headers(headers, url, None, None)?;

        debug!("Fetching URL as bytes: {}", url);

        // No URL resolution here on purpose: this path's callers build their
        // own URLs (`git_sync` does) rather than taking one from a script, so
        // a template would be a bug rather than a feature. `plain` carries no
        // secrets and so scrubs nothing.
        let plain = ResolvedUrl {
            target: url.to_string(),
            display: url.to_string(),
            names: Vec::new(),
            values: Vec::new(),
        };
        let response = self.send_request(
            Method::GET,
            url,
            headers,
            None,
            self.default_timeout,
            &plain,
        )?;

        let status = response.status().as_u16();
        let ok = response.status().is_success();

        if let Some(content_length) = response.content_length()
            && content_length > max_bytes as u64
        {
            return Err(HttpError::ResponseTooLarge(content_length));
        }

        // Same hard cap as the text path, for the same reason: a response
        // without a Content-Length must not be able to buffer unbounded.
        use std::io::Read;
        let mut body = Vec::new();
        response
            .take(max_bytes as u64 + 1)
            .read_to_end(&mut body)
            .map_err(|e| HttpError::ResponseReadFailed(e.to_string()))?;

        if body.len() > max_bytes {
            return Err(HttpError::ResponseTooLarge(body.len() as u64));
        }

        Ok(BytesResponse { status, body, ok })
    }

    /// Begin a request and hand back the response before its body has
    /// arrived.
    ///
    /// [`HttpClient::fetch`] reads the whole body before it returns anything,
    /// which is right for an API call and wrong for anything that streams: a
    /// model's token stream, an events endpoint, a log tail. A script could
    /// not consume one at all, so an agent's page updated once per turn and a
    /// turn was as long as the whole model call.
    ///
    /// Everything before the body is shared with `fetch` — the same URL and
    /// DNS validation, the same manually-followed and re-validated redirects,
    /// the same secret substitution — because a streaming path that quietly
    /// acquired weaker checks than the buffered one is the failure this
    /// client is arranged to prevent.
    ///
    /// One thing differs on purpose: it asks for **no content coding**.
    /// `fetch` offers gzip and undoes it after reading the whole body, which
    /// a stream cannot do — undoing a coding incrementally is a decoder and a
    /// buffer of its own, and the endpoints that stream are not compressed in
    /// practice. Asking for identity is the honest way to say that rather
    /// than discovering it as `invalid utf8` halfway through a response.
    pub fn fetch_streaming(
        &self,
        url: String,
        options: FetchOptions,
        script_uri: Option<&str>,
        user_id: Option<&str>,
    ) -> Result<StreamingResponse, HttpError> {
        let method = Method::from_str(&options.method.to_uppercase())
            .map_err(|_| HttpError::InvalidMethod(options.method.clone()))?;

        let resolved = resolve_url(&url, script_uri, user_id)?;
        for secret_id in &resolved.names {
            info!(
                secret_id = %secret_id,
                url = %resolved.display,
                script_uri = ?script_uri,
                "Secret accessed in fetch URL"
            );
        }

        let mut headers =
            self.process_headers(options.headers, &resolved.display, script_uri, user_id)?;
        headers.insert(
            reqwest::header::ACCEPT_ENCODING,
            reqwest::header::HeaderValue::from_static("identity"),
        );

        let timeout = options
            .timeout_ms
            .map(Duration::from_millis)
            .unwrap_or(self.default_timeout);

        debug!(
            "Streaming URL: {} with method: {}",
            resolved.display, options.method
        );

        let response = self
            .send_request(
                method,
                &resolved.target,
                headers,
                options.body,
                timeout,
                &resolved,
            )
            .map_err(|e| resolved.scrub_error(e))?;

        let status = response.status().as_u16();
        let ok = response.status().is_success();
        let mut header_map: HashMap<String, String> = HashMap::new();
        for (key, value) in response.headers() {
            if let Ok(value) = value.to_str() {
                header_map.insert(key.to_string(), value.to_string());
            }
        }

        // The cap a buffered fetch applies to the whole body applies here to
        // everything read over the life of the stream. A response with no
        // end is the case it exists for.
        if let Some(content_length) = response.content_length()
            && content_length > self.max_response_size as u64
        {
            return Err(HttpError::ResponseTooLarge(content_length));
        }

        Ok(StreamingResponse {
            status,
            ok,
            headers: header_map,
            body: response,
            pending: Vec::new(),
            read_total: 0,
            max_total: self.max_response_size,
            finished: false,
        })
    }

    /// Send a request, validating every redirect hop, and hand back the
    /// response undecoded.
    ///
    /// Shared by [`HttpClient::fetch`] and [`HttpClient::fetch_bytes`]. What
    /// separates those two is only how the final body is read; keeping one
    /// implementation of everything before that is what stops the binary path
    /// from quietly acquiring weaker validation than the text path.
    fn send_request(
        &self,
        method: Method,
        url: &str,
        headers: HeaderMap,
        body: Option<String>,
        timeout: Duration,
        // What must not appear in anything this function writes down. A
        // redirect resolves against a URL that may carry a credential, and
        // the hop is logged.
        resolved: &ResolvedUrl,
    ) -> Result<reqwest::blocking::Response, HttpError> {
        if !self.manual_redirects {
            // Test mode: single request through the redirect-following client
            let parsed_url = self.validate(url)?;
            let mut request = shared_test_client()?
                .request(method, parsed_url.as_str())
                .headers(headers)
                .timeout(crate::database::within_host_budget(timeout));
            if let Some(body) = body {
                request = request.body(body);
            }
            return request.send().map_err(Self::transport_error);
        }

        // Follow redirects manually so every hop is validated (URL scheme,
        // host, and DNS resolution). The shared client has redirects disabled.
        let client = shared_client()?;
        let mut current_url = self.validate(url)?;
        let mut current_method = method;
        let mut current_body = body;
        let mut current_headers = headers;

        for _ in 0..=MAX_REDIRECTS {
            // Recomputed per hop rather than once: the budget is what bounds
            // the whole fetch, and a chain of redirects each given the full
            // timeout could outlast the script by several multiples of it.
            let mut request = client
                .request(current_method.clone(), current_url.as_str())
                .headers(current_headers.clone())
                .timeout(crate::database::within_host_budget(timeout));
            if let Some(body) = &current_body {
                request = request.body(body.clone());
            }

            let response = request.send().map_err(Self::transport_error)?;

            let status = response.status();
            let is_redirect = matches!(status.as_u16(), 301 | 302 | 303 | 307 | 308);
            if !is_redirect {
                return Ok(response);
            }

            let Some(location) = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
            else {
                // Redirect status without a Location header: return as-is
                return Ok(response);
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

            debug!(
                "Following redirect ({}) to {}",
                status.as_u16(),
                resolved.scrub(next_url.as_str())
            );
            current_url = next_url;
        }

        Err(HttpError::RequestFailed(format!(
            "Too many redirects (max {})",
            MAX_REDIRECTS
        )))
    }

    /// A transport failure, keeping a timeout distinguishable from the rest.
    ///
    /// The distinction belongs to the caller rather than to this client: a
    /// timeout is the one transport failure a caller can act on — shorten the
    /// work, raise the budget — and collapsing it into the same string as a
    /// refused connection costs that. `mcp_client` reports one as such.
    fn transport_error(e: reqwest::Error) -> HttpError {
        if e.is_timeout() {
            HttpError::Timeout
        } else {
            HttpError::RequestFailed(e.to_string())
        }
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
                let where_ = format!("header '{}'", key);
                let (final_value, secrets_used) = substitute_secrets(&where_, &value, |name| {
                    // Look up secret from database: user_secrets first, then
                    // script_secrets. Environment variables and config files
                    // are never consulted.
                    crate::repository::resolve_secret_db(script_uri.unwrap_or(""), name, user_id)
                })?;

                // Audit log (identifier only, never value). One line per
                // secret, since a value may now name more than one.
                for secret_id in &secrets_used {
                    info!(
                        secret_id = %secret_id,
                        url = url,
                        script_uri = ?script_uri,
                        "Secret accessed in fetch request"
                    );
                }

                // Add to header map
                let header_name = reqwest::header::HeaderName::from_str(&key)
                    .map_err(|e| HttpError::InvalidHeader(format!("Invalid header name: {}", e)))?;
                // Also the guard against a secret smuggling a header: a value
                // carrying CR or LF is refused here rather than split into two
                // headers by whatever reads it next.
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
        binary: bool,
    ) -> Result<FetchResponse, HttpError> {
        let status = response.status().as_u16();
        let ok = response.status().is_success();

        // Extract headers
        let mut headers: HashMap<String, String> = HashMap::new();
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

        // Undo the content coding before the UTF-8 decode: a gzipped body is
        // not text, and reading it as such is where this used to fail with
        // `invalid utf8` on any host that compresses by default.
        let encoding = headers
            .get("content-encoding")
            .map(|value| value.to_string())
            .unwrap_or_default();
        // An empty body carries no stream to inflate however it is labelled —
        // a `HEAD` or a `204` may still name a coding — and handing a
        // decompressor nothing is an error about the wrong thing.
        let bytes = if bytes.is_empty() || is_identity(&encoding) {
            bytes
        } else {
            let decoded = decode_body(&encoding, bytes, self.max_response_size)?;
            // The headers now describe a body that no longer exists: the
            // content is decoded, and `Content-Length` counted the compressed
            // bytes. Browsers drop both from what `fetch` exposes, for the
            // same reason — a script reading either would be misled.
            headers.remove("content-encoding");
            headers.remove("content-length");
            decoded
        };

        if binary {
            use base64::Engine as _;
            return Ok(FetchResponse {
                status,
                headers,
                body: String::new(),
                body_base64: Some(base64::engine::general_purpose::STANDARD.encode(&bytes)),
                ok,
            });
        }

        // Convert to string (UTF-8)
        let body = String::from_utf8(bytes).map_err(|e| {
            // The message names the way out, because this failure is
            // recoverable and the caller cannot guess how: every script that
            // hit it was fetching something that was never text.
            HttpError::ResponseEncodingError(format!(
                "{e} - the body is not text; pass {{ binary: true }} to receive                  it as base64 in bodyBase64"
            ))
        })?;

        Ok(FetchResponse {
            status,
            headers,
            body,
            body_base64: None,
            ok,
        })
    }
}

/// Whether a `Content-Encoding` means "nothing was applied".
///
/// Absent and `identity` are the same answer, and an empty header field is
/// treated as absent rather than as an error nobody can act on.
fn is_identity(encoding: &str) -> bool {
    encoding
        .split(',')
        .map(str::trim)
        .all(|coding| coding.is_empty() || coding.eq_ignore_ascii_case("identity"))
}

/// Undo the codings named by a `Content-Encoding` header.
///
/// The header lists them in the order they were applied, so they come off in
/// reverse. In practice there is one, but a chain is what the field means and
/// applying it backwards would silently produce rubbish.
///
/// `max_bytes` bounds what comes _out_, not what went in: the input is already
/// capped, and without a second bound a small compressed body could inflate to
/// whatever a decompression bomb wanted. Exceeding it is `ResponseTooLarge`
/// for the same reason an oversized plain body is.
fn decode_body(encoding: &str, body: Vec<u8>, max_bytes: usize) -> Result<Vec<u8>, HttpError> {
    let codings: Vec<&str> = encoding
        .split(',')
        .map(str::trim)
        .filter(|coding| !coding.is_empty() && !coding.eq_ignore_ascii_case("identity"))
        .collect();

    let mut body = body;
    for coding in codings.into_iter().rev() {
        body = if coding.eq_ignore_ascii_case("gzip") || coding.eq_ignore_ascii_case("x-gzip") {
            read_capped(flate2::read::GzDecoder::new(body.as_slice()), max_bytes)?
        } else if coding.eq_ignore_ascii_case("deflate") {
            // `deflate` is zlib-wrapped per RFC 9110, but enough servers send
            // the raw stream that every browser accepts both. Falling back
            // costs one failed read of a body that is in memory already.
            match read_capped(flate2::read::ZlibDecoder::new(body.as_slice()), max_bytes) {
                Ok(decoded) => decoded,
                Err(HttpError::ResponseTooLarge(size)) => {
                    return Err(HttpError::ResponseTooLarge(size));
                }
                Err(_) => read_capped(
                    flate2::read::DeflateDecoder::new(body.as_slice()),
                    max_bytes,
                )?,
            }
        } else {
            return Err(HttpError::UnsupportedContentEncoding(coding.to_string()));
        };
    }

    Ok(body)
}

/// Read a decoder to its end, refusing anything past `max_bytes`.
fn read_capped(reader: impl std::io::Read, max_bytes: usize) -> Result<Vec<u8>, HttpError> {
    use std::io::Read;

    let mut out = Vec::new();
    reader
        .take(max_bytes as u64 + 1)
        .read_to_end(&mut out)
        .map_err(|e| HttpError::ResponseEncodingError(format!("Could not decompress: {}", e)))?;

    if out.len() > max_bytes {
        return Err(HttpError::ResponseTooLarge(out.len() as u64));
    }

    Ok(out)
}

/// The template a header value uses to name a secret: `{{secret:NAME}}`.
///
/// The two halves are constants because three separate steps read them —
/// finding a template, taking the name out of it, and resuming the scan past
/// it — and an off-by-one between those is a wrong answer rather than a
/// failure.
const SECRET_TEMPLATE_OPEN: &str = "{{secret:";
const SECRET_TEMPLATE_CLOSE: &str = "}}";

thread_local! {
    /// The response streams this execution has open.
    ///
    /// Thread-local because a script runs on one blocking thread and a
    /// stream is read across several host calls: the socket has to outlive
    /// the call that opened it and die with the execution. `HostCallBudget`
    /// is what marks that span, so its `Drop` empties this.
    ///
    /// Keyed by a counter rather than by anything a script chooses. An id a
    /// script could name would be one it could guess, and a stream is a
    /// readable socket — though the registry being per thread already means
    /// a guess reaches only the guesser's own execution.
    static OPEN_STREAMS: std::cell::RefCell<HashMap<u64, StreamingResponse>> =
        std::cell::RefCell::new(HashMap::new());
    static NEXT_STREAM_ID: std::cell::Cell<u64> = const { std::cell::Cell::new(1) };
}

/// How many response streams one execution may hold open at once.
///
/// Each is a socket held for as long as the execution lasts. A script
/// reading two model responses side by side is reasonable; one opening
/// streams in a loop and abandoning them is what this bounds.
pub const MAX_OPEN_STREAMS: usize = 8;

/// Register an open stream and hand back the id that reads it.
pub fn register_stream(stream: StreamingResponse) -> Result<u64, HttpError> {
    OPEN_STREAMS.with(|streams| {
        let mut streams = streams.borrow_mut();
        if streams.len() >= MAX_OPEN_STREAMS {
            return Err(HttpError::RequestFailed(format!(
                "this execution already has {} response streams open",
                MAX_OPEN_STREAMS
            )));
        }
        let id = NEXT_STREAM_ID.with(|next| {
            let id = next.get();
            next.set(id.saturating_add(1));
            id
        });
        streams.insert(id, stream);
        Ok(id)
    })
}

/// Read the next piece of an open stream.
///
/// The stream is taken out of the registry for the read and put back after,
/// because reading needs `&mut` and a `RefCell` held across a blocking
/// network read would refuse every other borrow for the length of it. Taking
/// it out also makes a re-entrant read — a script reading the same stream
/// from inside a callback — answer "no such stream" instead of panicking on
/// the borrow.
pub fn read_stream(id: u64) -> Result<Option<String>, HttpError> {
    let Some(mut stream) = OPEN_STREAMS.with(|streams| streams.borrow_mut().remove(&id)) else {
        return Err(HttpError::RequestFailed(
            "that response stream is not open".to_string(),
        ));
    };

    let answer = stream.read_chunk();

    // Put it back unless it is spent: a stream that ended or failed has
    // nothing more to give, and leaving it registered would hold its socket
    // until the execution ended.
    if matches!(answer, Ok(Some(_))) {
        OPEN_STREAMS.with(|streams| {
            streams.borrow_mut().insert(id, stream);
        });
    }

    answer
}

/// Close one stream early, for a caller that has read enough.
pub fn close_stream(id: u64) -> bool {
    OPEN_STREAMS.with(|streams| streams.borrow_mut().remove(&id).is_some())
}

/// Drop every stream this thread holds. Called when an execution ends.
pub fn close_all_streams() {
    OPEN_STREAMS.with(|streams| streams.borrow_mut().clear());
}

/// How many requests of one `fetchAll` are in flight at once.
///
/// Each one holds a thread from tokio's blocking pool for its round trip, so
/// this is a claim on a resource every other blocking host call shares. Eight
/// is past what an agent fanning out over tool calls needs and far short of
/// what would starve the pool; a larger batch is run in waves rather than
/// refused, since the caller's intent is legible and the only question is how
/// fast it happens.
pub const MAX_PARALLEL_FETCHES: usize = 8;

/// One request of a parallel batch.
pub struct ParallelRequest {
    pub url: String,
    pub options: FetchOptions,
}

/// Run several requests at once, answering in the order they were given.
///
/// `fetch` is a synchronous host call, so `Promise.all` over three of them
/// sequences them: each holds an execution slot and a blocking thread for its
/// whole round trip, and the wall clock is the sum. For an agent wanting to
/// run three tool calls at once that is not an ergonomic complaint, it is the
/// difference between fitting inside the execution budget and not.
///
/// This is the same `fetch` per request — the same validation, the same
/// redirects, the same secret substitution — run on the blocking pool instead
/// of on the caller's thread. One thread is blocked on the batch rather than
/// one per request in series, and the wall clock becomes the slowest rather
/// than the sum.
///
/// **The budget travels with the work.** Every host call reads its deadline
/// from a thread-local, and a request handed to another thread would find
/// none and take its own full timeout — so a script with two seconds left
/// could start a thirty-second fetch. The remaining budget is read on the
/// calling thread and armed again on each worker.
///
/// A failure is per request: one refused URL answers as an error in its own
/// slot rather than failing the batch, because the caller asked for several
/// answers and has a use for the ones that arrived.
impl HttpClient {
    pub fn fetch_all(
        &self,
        requests: Vec<ParallelRequest>,
        script_uri: Option<&str>,
        user_id: Option<&str>,
    ) -> Vec<Result<FetchResponse, HttpError>> {
        let budget = crate::database::host_budget_remaining();
        let script_uri = script_uri.map(str::to_string);
        let user_id = user_id.map(str::to_string);

        let mut answers = Vec::with_capacity(requests.len());

        for wave in requests.chunks(MAX_PARALLEL_FETCHES) {
            let handles: Vec<_> = wave
                .iter()
                .map(|request| {
                    let url = request.url.clone();
                    let options = request.options.clone();
                    let script_uri = script_uri.clone();
                    let user_id = user_id.clone();
                    let deadline = budget.map(|remaining| std::time::Instant::now() + remaining);
                    // *This* client, copied, rather than a fresh default one.
                    // A batch that quietly used different settings from the
                    // `fetch` beside it — a different size ceiling, different
                    // address rules — would be the kind of divergence this
                    // module is arranged to prevent.
                    let client = *self;

                    std::thread::spawn(move || {
                        // Armed here, on this thread, for the reason above.
                        // The guard restores what was there when it ends.
                        let _budget = deadline.map(crate::database::bound_host_calls);
                        client.fetch(url, options, script_uri.as_deref(), user_id.as_deref())
                    })
                })
                .collect();

            for handle in handles {
                answers.push(match handle.join() {
                    Ok(answer) => answer,
                    // A panicked worker is reported as a failed request
                    // rather than propagated: the other requests in the wave
                    // have answers, and losing them to one thread's failure
                    // would be the sequential behaviour this replaces.
                    Err(_) => Err(HttpError::RequestFailed(
                        "the request thread stopped unexpectedly".to_string(),
                    )),
                });
            }
        }

        answers
    }
}

/// A response being read a piece at a time.
///
/// Holds the open connection, so it lives between host calls rather than
/// inside one. What that costs is stated where it is registered
/// ([`crate::security::secure_globals`]): a stream nobody finishes reading
/// holds a socket until the execution that opened it ends.
pub struct StreamingResponse {
    pub status: u16,
    pub ok: bool,
    pub headers: HashMap<String, String>,
    body: reqwest::blocking::Response,
    /// Bytes read that did not yet form whole characters.
    ///
    /// A chunk boundary falls wherever the network put it, which is
    /// regularly in the middle of a multi-byte character. Decoding each read
    /// on its own would replace those halves with `U+FFFD` — silently, and
    /// most often on exactly the text a model is generating. So the tail
    /// that is not yet a character is kept here and prepended to the next
    /// read.
    pending: Vec<u8>,
    read_total: usize,
    max_total: usize,
    finished: bool,
}

/// How much is read from the socket per `read_chunk`.
///
/// A ceiling rather than a target: a read returns whatever has arrived, so a
/// token stream yields a token at a time and a fast bulk response yields
/// this much. Small enough that a chunk crosses into JavaScript promptly,
/// large enough that a megabyte does not cost a thousand host calls.
const STREAM_READ_BYTES: usize = 16 * 1024;

impl StreamingResponse {
    /// The next piece of the body, or `None` once there is no more.
    ///
    /// Blocks until something arrives, which is the point: the caller is a
    /// script that has asked for the next token and has nothing else to do
    /// until it has one.
    pub fn read_chunk(&mut self) -> Result<Option<String>, HttpError> {
        use std::io::Read;

        if self.finished {
            return Ok(None);
        }

        loop {
            let mut buffer = [0u8; STREAM_READ_BYTES];
            let read = self
                .body
                .read(&mut buffer)
                .map_err(|e| HttpError::ResponseReadFailed(e.to_string()))?;

            if read == 0 {
                self.finished = true;
                // Whatever is left cannot become a character now, so it is a
                // truncated response rather than a boundary to wait on.
                if self.pending.is_empty() {
                    return Ok(None);
                }
                let tail = String::from_utf8_lossy(&self.pending).into_owned();
                self.pending.clear();
                return Ok(Some(tail));
            }

            self.read_total = self.read_total.saturating_add(read);
            if self.read_total > self.max_total {
                self.finished = true;
                return Err(HttpError::ResponseTooLarge(self.read_total as u64));
            }

            self.pending.extend_from_slice(&buffer[..read]);

            match take_valid_utf8(&mut self.pending) {
                // Everything read so far is the tail of a character. Read
                // again rather than answering with nothing, which a caller
                // would have to tell apart from the end of the stream.
                text if text.is_empty() => continue,
                text => return Ok(Some(text)),
            }
        }
    }
}

/// Split `buffer` at the end of its last whole character, returning the text
/// and leaving the remainder behind.
///
/// The remainder is only ever the few bytes of a character split across two
/// reads. A genuinely invalid sequence is not held back forever: it fails to
/// decode, is not a valid prefix either, and is replaced where it stands.
fn take_valid_utf8(buffer: &mut Vec<u8>) -> String {
    match std::str::from_utf8(buffer) {
        Ok(text) => {
            let text = text.to_string();
            buffer.clear();
            text
        }
        Err(error) => {
            let valid = error.valid_up_to();
            match error.error_len() {
                // An incomplete character at the end: keep it for the next
                // read, which is what this function exists for.
                None => {
                    let text = String::from_utf8_lossy(&buffer[..valid]).into_owned();
                    buffer.drain(..valid);
                    text
                }
                // Genuinely invalid bytes. Waiting cannot fix them, so take
                // everything and let the lossy decode mark the damage.
                Some(_) => {
                    let text = String::from_utf8_lossy(buffer).into_owned();
                    buffer.clear();
                    text
                }
            }
        }
    }
}

/// Whether this request would read a secret if it were sent.
///
/// The capability layer asks before calling, so a context holding
/// `use_network` but not `read_secrets` is refused rather than having its
/// template resolved. It reads the same constant [`substitute_secrets`] does,
/// so the two cannot come to disagree about what a template looks like — and
/// it is deliberately over-eager in the one direction that is safe: an
/// unclosed `{{secret:` is a request that would error anyway.
pub fn names_a_secret(url: &str, options: &FetchOptions) -> bool {
    // The URL is checked as well as the headers, and forgetting it here would
    // be the whole of the hole: a context holding `use_network` but not
    // `read_secrets` would have its URL template resolved because nothing
    // asked, which is exactly the check this function exists to make.
    url.contains(SECRET_TEMPLATE_OPEN)
        || options.headers.as_ref().is_some_and(|headers| {
            headers
                .values()
                .any(|value| value.contains(SECRET_TEMPLATE_OPEN))
        })
}

/// Replace every `{{secret:NAME}}` in one header value with what `resolve`
/// answers for `NAME`, and report which names were used.
///
/// Substitution is **inline**: the template stands for a secret within the
/// value rather than for the whole of it. That is what the requirement always
/// said — `docs/engine-contributors/planning/REQUIREMENTS.md` gives
/// `"Authorization": "Bearer {{secret:api_token}}"` as a worked example — and
/// what the implementation did not do. It matched only a value that was
/// nothing *but* a template, which put every bearer-token API, meaning most of
/// them, out of reach of a stored secret. A script written from the published
/// example sent the template text to the API as itself and got back a 401 that
/// explained nothing.
///
/// Three things are errors rather than text passed through, because passing
/// them through is the failure this replaced: a request that looks right,
/// carries a credential that is not one, and fails somewhere the script cannot
/// see.
///
/// - a name nothing resolves ([`HttpError::SecretNotFound`]),
/// - a template naming no secret at all (`{{secret:}}`),
/// - an opening with no closing.
///
/// The scan runs once over the *input*. A resolved value is appended to the
/// output and never read again, so a secret whose own value contains the
/// template text is sent as it stands rather than standing for a second
/// lookup.
///
/// Used for header values and, through [`resolve_url`], for the path of a
/// URL. The URL case was refused outright until an API that leaves no choice
/// turned up: the Telegram Bot API puts its token in the path and offers no
/// header to carry it, so "headers only" meant "not reachable from this
/// engine at all". What made that refusal right is still true — a URL is
/// written to the audit line [`HttpClient::process_headers`] emits, to this
/// engine's logs and to the far end's access log — so the rule is narrower
/// now rather than gone. [`resolve_url`] is what narrows it.
fn substitute_secrets(
    where_: &str,
    value: &str,
    mut resolve: impl FnMut(&str) -> Option<String>,
) -> Result<(String, Vec<String>), HttpError> {
    let Some(mut open) = value.find(SECRET_TEMPLATE_OPEN) else {
        return Ok((value.to_string(), Vec::new()));
    };

    let mut rendered = String::with_capacity(value.len());
    let mut rest = value;
    let mut used = Vec::new();

    loop {
        rendered.push_str(&rest[..open]);

        // Everything after this template's opening, so the name and the
        // remainder are both measured from one place.
        let after_open = &rest[open + SECRET_TEMPLATE_OPEN.len()..];

        // The messages name the header and never the value: by this point the
        // value may hold a secret that resolved before the malformed one.
        let Some(close) = after_open.find(SECRET_TEMPLATE_CLOSE) else {
            return Err(HttpError::InvalidHeader(format!(
                "{} opens a {}...{} template that is never closed",
                where_, SECRET_TEMPLATE_OPEN, SECRET_TEMPLATE_CLOSE
            )));
        };

        let name = after_open[..close].trim();
        if name.is_empty() {
            return Err(HttpError::InvalidHeader(format!(
                "{} has a {}...{} template naming no secret",
                where_, SECRET_TEMPLATE_OPEN, SECRET_TEMPLATE_CLOSE
            )));
        }

        rendered
            .push_str(&resolve(name).ok_or_else(|| HttpError::SecretNotFound(name.to_string()))?);
        used.push(name.to_string());

        rest = &after_open[close + SECRET_TEMPLATE_CLOSE.len()..];
        match rest.find(SECRET_TEMPLATE_OPEN) {
            Some(next) => open = next,
            None => {
                rendered.push_str(rest);
                return Ok((rendered, used));
            }
        }
    }
}

/// A URL with its `{{secret:...}}` templates resolved, beside the template it
/// came from.
///
/// The template is what anything that *writes the URL down* uses — the audit
/// line, the debug log, an error handed back to a script — and the resolved
/// form goes to reqwest and nowhere else.
///
/// That split is only trustworthy because of the invariant [`resolve_url`]
/// enforces: **a secret may change the path, the query and the fragment, and
/// may not change the origin.** So the URL in the log names the host that was
/// actually contacted. Without it the log would be a guess, which is worse
/// than the credential-in-the-log problem this whole arrangement exists to
/// avoid — a wrong audit line is believed.
struct ResolvedUrl {
    /// Carries the credential. For reqwest, and for nothing that is recorded.
    target: String,
    /// The template. Safe to log, to put in an error, to return to a script.
    display: String,
    /// Which secrets were read, for the audit line.
    names: Vec<String>,
    /// What they resolved to, so anything derived from `target` can be
    /// scrubbed before it is written down.
    values: Vec<String>,
}

/// Written by hand rather than derived, and that is the point of it.
///
/// A derived `Debug` would print `target` and `values` — the resolved URL and
/// the credential inside it — into whatever formatted this struct, which is
/// the failure the rest of this type exists to prevent. Anything that wants to
/// show a URL wants `display`.
impl std::fmt::Debug for ResolvedUrl {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedUrl")
            .field("display", &self.display)
            .field("names", &self.names)
            .field("secrets", &self.values.len())
            .finish()
    }
}

impl ResolvedUrl {
    /// Whether any secret was involved at all.
    fn is_plain(&self) -> bool {
        self.values.is_empty()
    }

    /// Replace every resolved secret in `text` with a marker.
    ///
    /// The backstop for text derived from `target` rather than written by us:
    /// a reqwest transport error quotes the URL it was given, and a redirect
    /// resolves against it. Cheap, and it costs nothing when no secret was
    /// used.
    fn scrub(&self, text: &str) -> String {
        if self.is_plain() {
            return text.to_string();
        }
        let mut out = text.to_string();
        for value in &self.values {
            // An empty secret would match everywhere and replace nothing
            // usefully; a stored empty value is a configuration mistake rather
            // than something to defend against here.
            if value.is_empty() {
                continue;
            }
            out = out.replace(value.as_str(), "{{secret}}");
        }
        out
    }

    /// The same, for an error on its way back to a script.
    fn scrub_error(&self, error: HttpError) -> HttpError {
        if self.is_plain() {
            return error;
        }
        match error {
            HttpError::InvalidUrl(s) => HttpError::InvalidUrl(self.scrub(&s)),
            HttpError::InvalidUrlScheme(s) => HttpError::InvalidUrlScheme(self.scrub(&s)),
            HttpError::BlockedUrl(s) => HttpError::BlockedUrl(self.scrub(&s)),
            HttpError::RequestFailed(s) => HttpError::RequestFailed(self.scrub(&s)),
            HttpError::InvalidHeader(s) => HttpError::InvalidHeader(self.scrub(&s)),
            // The rest carry no text that a URL could have reached.
            other => other,
        }
    }
}

/// Resolve `{{secret:...}}` in a URL, refusing anything that would move the
/// request somewhere else.
///
/// Substituting into a URL was refused outright for a good reason: a URL ends
/// up in three logs the credential has no business being in. The Telegram Bot
/// API is the case that made the refusal untenable rather than merely strict —
/// `https://api.telegram.org/bot<token>/sendMessage`, with no header form — so
/// "headers only" meant a whole class of API was unreachable from a script.
///
/// What makes this safe is that the *template* already carries the scheme and
/// the host. So every check this client makes about where a request may go
/// runs against a string with no credential in it, the log lines keep using
/// that string, and the resolved form exists only long enough to be sent.
///
/// The origin check is what holds it together. A secret whose value contains
/// `/`, `?` or `#` can only add path, query or fragment — none of those move
/// the host — but a template like `https://{{secret:x}}/` would, and a stored
/// value is not necessarily one the script's author chose. Refusing on a
/// changed origin means the displayed URL cannot lie about where the request
/// went, which is the property the audit line is for.
fn resolve_url(
    url: &str,
    script_uri: Option<&str>,
    user_id: Option<&str>,
) -> Result<ResolvedUrl, HttpError> {
    resolve_url_with(url, |name| {
        // user_secrets first, then script_secrets. Environment variables and
        // config files are never consulted, as everywhere else here.
        crate::repository::resolve_secret_db(script_uri.unwrap_or(""), name, user_id)
    })
}

/// The whole of [`resolve_url`] except where the secrets come from.
///
/// Split out so the rules above can be tested without a database behind them:
/// the origin check and the redaction are the parts worth pinning, and neither
/// has anything to do with where a value was stored.
fn resolve_url_with(
    url: &str,
    mut resolve: impl FnMut(&str) -> Option<String>,
) -> Result<ResolvedUrl, HttpError> {
    if !url.contains(SECRET_TEMPLATE_OPEN) {
        return Ok(ResolvedUrl {
            target: url.to_string(),
            display: url.to_string(),
            names: Vec::new(),
            values: Vec::new(),
        });
    }

    let mut values = Vec::new();
    let (target, names) = substitute_secrets("the URL", url, |name| {
        let found = resolve(name);
        if let Some(value) = &found {
            values.push(value.clone());
        }
        found
    })
    // `substitute_secrets` reports a malformed template as a header problem,
    // which is what it is everywhere else it is called. Here it is a URL
    // problem, and "Invalid header: the URL opens ..." would send somebody
    // looking in the wrong place.
    .map_err(|e| match e {
        HttpError::InvalidHeader(detail) => HttpError::InvalidUrl(detail),
        other => other,
    })?;

    // Both sides parsed, and compared as origins rather than as text: the
    // template's `{` and `}` are percent-encoded by the parser, so comparing
    // the strings would report a difference that is not one.
    let template_parsed = Url::parse(url)
        .map_err(|e| HttpError::InvalidUrl(format!("{} (before substitution)", e)))?;
    let target_parsed = Url::parse(&target).map_err(|_| {
        // Deliberately not quoting the parse error: it would contain the URL
        // it failed on, which by here has the secret in it.
        HttpError::InvalidUrl("the URL is not valid once its secret is substituted".to_string())
    })?;

    if template_parsed.scheme() != target_parsed.scheme()
        || template_parsed.host_str() != target_parsed.host_str()
        || template_parsed.port_or_known_default() != target_parsed.port_or_known_default()
    {
        return Err(HttpError::BlockedUrl(
            "a secret in a URL may fill in the path, and may not change the scheme, host or port"
                .to_string(),
        ));
    }

    Ok(ResolvedUrl {
        target,
        display: url.to_string(),
        names,
        values,
    })
}

/// Validate a caller-supplied URL the way a request through [`HttpClient`]
/// validates its target: scheme, literal address, and every address the
/// hostname resolves to.
///
/// For a caller holding a URL before it has a request to make.
/// [`crate::mcp_client::McpClient`] takes its server URL from a script, and
/// refusing a blocked one when the client is constructed is what keeps a call
/// to an address it was never going to be allowed to contact from reaching a
/// secret lookup first. It is the early answer rather than the enforcement:
/// the request path validates again, redirect hops included, because DNS can
/// say something different by then.
pub fn validate_public_url(url: &str) -> Result<Url, HttpError> {
    HttpClient::validate_url(url)
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

    /// Request headers (a `{{secret:name}}` anywhere in a value is replaced)
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,

    /// Request body
    #[serde(default)]
    pub body: Option<String>,

    /// Timeout in milliseconds
    ///
    /// The alias is not cosmetic: the type declarations have documented this
    /// option as `timeout` since they were written, while serde has only ever
    /// read `timeout_ms` — so every script that followed the documentation got
    /// the default timeout and no indication that its own had been ignored. An
    /// alias fixes it in the direction that breaks nothing, since callers using
    /// the real name go on working.
    #[serde(default, alias = "timeout")]
    pub timeout_ms: Option<u64>,

    /// Ask for the body as base64 rather than as text.
    ///
    /// The body of a response is a `String`, and a body that is not UTF-8 is an
    /// error rather than a lossy decode — right for the JSON and HTML that
    /// nearly every call here fetches, and the reason an image could not be
    /// retrieved at all. A photo somebody sends a bot is the case: the bytes
    /// exist, the caller knows perfectly well they are not text, and there was
    /// no way to say so.
    ///
    /// An option rather than a second field on every response, because base64
    /// is a third again the size and every other call would pay for it. And an
    /// option rather than a silent fallback, because a binary body answering as
    /// an empty string reads exactly like a server that sent nothing.
    #[serde(default)]
    pub binary: bool,
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
            binary: false,
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

    /// Response body as string. Empty when [`FetchOptions::binary`] was asked
    /// for — the bytes are in `body_base64` instead.
    pub body: String,

    /// Response body as base64, present only when `binary` was asked for.
    ///
    /// Never populated alongside a non-empty `body`: exactly one of the two
    /// carries the answer, so there is no question of which to believe and no
    /// caller paying for an encoding it did not want.
    #[serde(rename = "bodyBase64", skip_serializing_if = "Option::is_none")]
    pub body_base64: Option<String>,

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

/// An undecoded response body, for callers reading something that is not text.
#[derive(Debug, Clone)]
pub struct BytesResponse {
    /// HTTP status code
    pub status: u16,
    /// Response body, exactly as it arrived
    pub body: Vec<u8>,
    /// Whether the request was successful (2xx status)
    pub ok: bool,
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

    #[error("Request timed out")]
    Timeout,

    #[error("Response too large: {0} bytes (max {MAX_RESPONSE_SIZE})")]
    ResponseTooLarge(u64),

    #[error("Failed to read response: {0}")]
    ResponseReadFailed(String),

    #[error("Response encoding error: {0}")]
    ResponseEncodingError(String),

    #[error("Unsupported Content-Encoding: {0} (this client understands {SUPPORTED_ENCODINGS})")]
    UnsupportedContentEncoding(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Resolves the names this map holds and nothing else.
    fn resolver(pairs: &[(&str, &str)]) -> impl FnMut(&str) -> Option<String> + use<> {
        let known: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name: &str| known.get(name).cloned()
    }

    // ---- a secret in the URL --------------------------------------------
    //
    // The rule these pin: a secret may fill in the path, and the URL that is
    // written down stays the template. Telegram is why this exists — its Bot
    // API takes the token in the path and offers no header for it.

    #[test]
    fn a_url_with_no_template_is_left_alone() {
        let resolved = resolve_url_with("https://example.com/x", resolver(&[])).expect("plain");
        assert_eq!(resolved.target, "https://example.com/x");
        assert_eq!(resolved.display, "https://example.com/x");
        assert!(resolved.is_plain());
    }

    #[test]
    fn a_secret_fills_in_the_path() {
        let resolved = resolve_url_with(
            "https://api.telegram.org/bot{{secret:tg}}/sendMessage",
            resolver(&[("tg", "12345:AAbbCC")]),
        )
        .expect("resolved");

        assert_eq!(
            resolved.target,
            "https://api.telegram.org/bot12345:AAbbCC/sendMessage"
        );
        assert_eq!(resolved.names, vec!["tg".to_string()]);
    }

    #[test]
    fn what_gets_written_down_is_the_template() {
        let resolved = resolve_url_with(
            "https://api.telegram.org/bot{{secret:tg}}/sendMessage",
            resolver(&[("tg", "12345:AAbbCC")]),
        )
        .expect("resolved");

        // The audit line, the debug log and any error use this.
        assert_eq!(
            resolved.display,
            "https://api.telegram.org/bot{{secret:tg}}/sendMessage"
        );
        assert!(!resolved.display.contains("AAbbCC"));
    }

    #[test]
    fn a_secret_may_not_change_the_host() {
        // Refused twice over, which is worth knowing. The origin check below
        // would catch it, but this never reaches the origin check: a `{` is
        // not legal in a host, so the *template* fails to parse. The host
        // position cannot be templated at all.
        let error = resolve_url_with(
            "https://{{secret:where}}/path",
            resolver(&[("where", "evil.example.com")]),
        )
        .expect_err("a moved origin is refused");

        assert!(!error.to_string().contains("evil.example.com"));
    }

    #[test]
    fn a_secret_may_not_change_the_port() {
        let error = resolve_url_with(
            "https://example.com:443{{secret:rest}}",
            resolver(&[("rest", ":8443/x")]),
        )
        .expect_err("a moved port is refused");

        // Whichever way it is caught, the refusal must not quote the value.
        assert!(!error.to_string().contains("8443"));
    }

    #[test]
    fn nothing_a_secret_can_hold_moves_the_request() {
        // The property, over the ways a secret might try to leave the origin
        // it was written against. Every one is refused, and no refusal quotes
        // the value.
        //
        // Worth recording what this exercise showed: the origin check in
        // `resolve_url_with` is a backstop that nothing here reaches. A `{` is
        // not legal in an authority, so a template with a secret anywhere at
        // or before the host fails to parse *as a template* — and once the
        // secret sits after the first `/`, it is in the path, where `@`, `..`
        // and `:` are ordinary characters that move nothing. The check stays
        // because "no input I thought of" is a weaker guarantee than a check,
        // and it is two comparisons.
        let hostile = [
            ("https://{{secret:x}}/p", "evil.example.com"),
            ("https://example.com{{secret:x}}/p", "@evil.example.com"),
            ("https://example.com:443{{secret:x}}", ":8443/p"),
            ("https://ex{{secret:x}}ample.com/p", "@evil.example.com#"),
        ];

        for (template, value) in hostile {
            let error = resolve_url_with(template, resolver(&[("x", value)]))
                .expect_err(&format!("{} should be refused", template));
            assert!(
                !error.to_string().contains(value),
                "{} leaked its secret: {}",
                template,
                error
            );
        }
    }

    #[test]
    fn a_secret_in_the_path_stays_in_the_path() {
        // The other half of the same property: characters that would matter
        // in an authority are inert once the secret is past the first slash.
        let resolved = resolve_url_with(
            "https://example.com/{{secret:x}}/end",
            resolver(&[("x", "a@b:1/../c")]),
        )
        .expect("the origin has not moved");

        let parsed = Url::parse(&resolved.target).expect("valid");
        assert_eq!(parsed.host_str(), Some("example.com"));
        assert_eq!(parsed.scheme(), "https");
    }

    #[test]
    fn a_value_carrying_slashes_only_deepens_the_path() {
        let resolved = resolve_url_with(
            "https://example.com/{{secret:p}}/end",
            resolver(&[("p", "a/b/c")]),
        )
        .expect("still the same origin");
        assert_eq!(resolved.target, "https://example.com/a/b/c/end");
    }

    #[test]
    fn a_missing_secret_names_the_secret_and_not_the_url() {
        let error = resolve_url_with("https://example.com/{{secret:absent}}", resolver(&[]))
            .expect_err("unresolvable");
        match error {
            HttpError::SecretNotFound(name) => assert_eq!(name, "absent"),
            other => panic!("expected SecretNotFound, got {:?}", other),
        }
    }

    #[test]
    fn an_unclosed_template_in_a_url_says_so_without_naming_a_header() {
        let error = resolve_url_with("https://example.com/{{secret:oops", resolver(&[]))
            .expect_err("unclosed");
        let text = error.to_string();
        assert!(text.contains("the URL"), "got {}", text);
        // Reported as a URL problem. It used to say "Invalid header", which
        // sends somebody looking in the wrong place entirely.
        assert!(text.starts_with("Invalid URL"), "got {}", text);
    }

    #[test]
    fn scrubbing_takes_the_value_out_of_anything_derived() {
        let resolved = resolve_url_with(
            "https://api.telegram.org/bot{{secret:tg}}/sendMessage",
            resolver(&[("tg", "12345:AAbbCC")]),
        )
        .expect("resolved");

        // What a reqwest transport error looks like: it quotes the URL it was
        // handed, which is the resolved one.
        let leaked =
            "error sending request for url (https://api.telegram.org/bot12345:AAbbCC/sendMessage)";
        let scrubbed = resolved.scrub(leaked);
        assert!(!scrubbed.contains("AAbbCC"), "got {}", scrubbed);
        assert!(scrubbed.contains("{{secret}}"), "got {}", scrubbed);
    }

    #[test]
    fn scrubbing_reaches_the_error_a_script_is_handed() {
        let resolved = resolve_url_with(
            "https://api.telegram.org/bot{{secret:tg}}/sendMessage",
            resolver(&[("tg", "12345:AAbbCC")]),
        )
        .expect("resolved");

        let scrubbed = resolved.scrub_error(HttpError::RequestFailed(
            "failed to connect to https://api.telegram.org/bot12345:AAbbCC/sendMessage".to_string(),
        ));
        assert!(!scrubbed.to_string().contains("AAbbCC"));
    }

    #[test]
    fn a_plain_url_scrubs_nothing_and_costs_nothing() {
        let resolved = resolve_url_with("https://example.com/x", resolver(&[])).expect("plain");
        assert_eq!(resolved.scrub("anything at all"), "anything at all");
    }

    #[test]
    fn the_capability_check_sees_a_template_in_the_url() {
        // The hole this closes: a context holding `use_network` but not
        // `read_secrets` would otherwise have its URL resolved unasked.
        let bare = FetchOptions::default();
        assert!(names_a_secret(
            "https://api.telegram.org/bot{{secret:tg}}/x",
            &bare
        ));
        assert!(!names_a_secret("https://example.com/x", &bare));
    }

    #[test]
    fn a_template_that_is_the_whole_value_is_replaced() {
        let (rendered, used) = substitute_secrets(
            "x-api-key",
            "{{secret:api_key}}",
            resolver(&[("api_key", "sk-12345")]),
        )
        .expect("the secret resolves");

        assert_eq!(rendered, "sk-12345");
        assert_eq!(used, vec!["api_key".to_string()]);
    }

    /// The case the whole change is for: a bearer token is a prefix and a
    /// secret, and matching only a whole value put every API shaped like that
    /// out of reach.
    #[test]
    fn a_template_inside_a_value_is_replaced() {
        let (rendered, used) = substitute_secrets(
            "authorization",
            "Bearer {{secret:api_token}}",
            resolver(&[("api_token", "sk-12345")]),
        )
        .expect("the secret resolves");

        assert_eq!(rendered, "Bearer sk-12345");
        assert_eq!(used, vec!["api_token".to_string()]);
    }

    #[test]
    fn several_templates_in_one_value_are_all_replaced() {
        let (rendered, used) = substitute_secrets(
            "authorization",
            "Basic {{secret:user}}:{{secret:pass}} (v1)",
            resolver(&[("user", "alice"), ("pass", "hunter2")]),
        )
        .expect("both secrets resolve");

        assert_eq!(rendered, "Basic alice:hunter2 (v1)");
        assert_eq!(used, vec!["user".to_string(), "pass".to_string()]);
    }

    #[test]
    fn a_value_with_no_template_is_left_alone() {
        let (rendered, used) =
            substitute_secrets("accept", "application/json", resolver(&[])).expect("nothing to do");

        assert_eq!(rendered, "application/json");
        assert!(used.is_empty());
    }

    #[test]
    fn whitespace_around_the_name_is_ignored() {
        let (rendered, _) = substitute_secrets(
            "x-api-key",
            "{{secret:  api_key  }}",
            resolver(&[("api_key", "sk-12345")]),
        )
        .expect("the secret resolves");

        assert_eq!(rendered, "sk-12345");
    }

    /// A name nothing resolves fails the request. Sending the template text as
    /// itself is what produced a 401 with nothing to explain it.
    #[test]
    fn an_unknown_name_is_an_error_rather_than_literal_text() {
        let error = substitute_secrets(
            "authorization",
            "Bearer {{secret:missing}}",
            resolver(&[("api_token", "sk-12345")]),
        )
        .expect_err("nothing resolves `missing`");

        assert!(matches!(&error, HttpError::SecretNotFound(name) if name == "missing"));
    }

    #[test]
    fn a_template_that_is_never_closed_is_an_error() {
        let error = substitute_secrets(
            "authorization",
            "Bearer {{secret:api_token",
            resolver(&[("api_token", "sk-12345")]),
        )
        .expect_err("the template is unterminated");

        assert!(matches!(error, HttpError::InvalidHeader(_)));
    }

    #[test]
    fn a_template_naming_no_secret_is_an_error() {
        let error = substitute_secrets("authorization", "Bearer {{secret:   }}", resolver(&[]))
            .expect_err("the template names nothing");

        assert!(matches!(error, HttpError::InvalidHeader(_)));
    }

    /// Neither message may carry the value: by the time one is built the
    /// rendered half may already hold a secret that resolved.
    #[test]
    fn a_malformed_template_does_not_report_what_had_already_resolved() {
        let error = substitute_secrets(
            "authorization",
            "{{secret:api_token}} then {{secret:unclosed",
            resolver(&[("api_token", "sk-12345")]),
        )
        .expect_err("the second template is unterminated");

        assert!(!error.to_string().contains("sk-12345"));
    }

    /// The scan reads the input once. A secret whose own value looks like a
    /// template stands for itself, not for another lookup.
    #[test]
    fn a_resolved_value_is_not_scanned_again() {
        let (rendered, used) = substitute_secrets(
            "authorization",
            "{{secret:outer}}",
            resolver(&[
                ("outer", "{{secret:inner}}"),
                ("inner", "should-not-appear"),
            ]),
        )
        .expect("the outer secret resolves");

        assert_eq!(rendered, "{{secret:inner}}");
        assert_eq!(used, vec!["outer".to_string()]);
    }

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
