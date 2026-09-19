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
#[derive(Debug)]
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

        // Process headers and inject secrets
        let mut headers = self.process_headers(options.headers, &url, script_uri, user_id)?;

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

        debug!("Fetching URL: {} with method: {}", url, options.method);

        let response = self.send_request(method, &url, headers, options.body, timeout)?;
        self.convert_response(response)
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

        let response = self.send_request(Method::GET, url, headers, None, self.default_timeout)?;

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

            debug!("Following redirect ({}) to {}", status.as_u16(), next_url);
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
                let (final_value, secrets_used) = substitute_secrets(&key, &value, |name| {
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
/// Only header values. A URL is not substituted, and deliberately: a URL is
/// written to the audit line [`HttpClient::process_headers`] emits, to this
/// engine's logs and to the far end's access log, which is the one place a
/// credential should never appear.
fn substitute_secrets(
    header_name: &str,
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
                "header '{}' opens a {}...{} template that is never closed",
                header_name, SECRET_TEMPLATE_OPEN, SECRET_TEMPLATE_CLOSE
            )));
        };

        let name = after_open[..close].trim();
        if name.is_empty() {
            return Err(HttpError::InvalidHeader(format!(
                "header '{}' has a {}...{} template naming no secret",
                header_name, SECRET_TEMPLATE_OPEN, SECRET_TEMPLATE_CLOSE
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
