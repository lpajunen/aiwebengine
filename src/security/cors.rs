//! Which other origins may read the engine's own responses.
//!
//! `security.enable_cors` and `security.cors_allowed_origins` were parsed and
//! read by nothing, and `main.rs` printed "CORS enabled: true" at startup on
//! the strength of it. This is where they take effect.
//!
//! The split is the one [`super::headers`] already draws for the CSP, and for
//! the same reason: **the engine speaks only for responses it wrote.**
//!
//! - **Engine-owned paths** ([`crate::engine_api::RESERVED_ROUTE_PREFIXES`])
//!   get the configured policy. These carry session cookies and administrative
//!   data, so they are exactly the responses another origin must not be able to
//!   read by default.
//! - **Script routes get nothing.** A solution serving a public API knows which
//!   callers it wants; the engine does not, and a policy applied on its behalf
//!   would be a guess that either breaks the solution or over-permits it.
//!   Scripts set their own headers, as they already do for the CSP.
//! - **A header the response already carries is never replaced.** The OAuth2
//!   protocol endpoints under `/auth` install a deliberately wide layer of
//!   their own — browser-based MCP clients reach the token endpoint from
//!   origins nobody can enumerate, and no cookie rides on a PKCE code exchange
//!   — so that layer runs first and this one leaves it alone.
//!
//! ## The wildcard cannot carry a session
//!
//! `Access-Control-Allow-Origin: *` and `Access-Control-Allow-Credentials:
//! true` are mutually exclusive: a browser rejects the pair rather than
//! honouring it. So `cors_allowed_origins = ["*"]` means *any origin, no
//! credentials* — which for `/engine/*` means the caller is unauthenticated and
//! sees what an unauthenticated caller sees. Naming origins explicitly is what
//! allows a cross-origin admin UI to send its cookie, and it is the only form
//! that does.
//!
//! Reflecting an arbitrary origin *and* allowing credentials would be the
//! recognisable way to get this wrong: every site a signed-in administrator
//! visits could then read `/engine/*` as them. The wildcard is deliberately not
//! a shortcut to that.

use axum::extract::{Request, State};
use axum::http::{HeaderName, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::Response;
use std::sync::Arc;

/// What the layer was configured to allow.
#[derive(Debug, Clone, Default)]
pub struct CorsConfig {
    /// `security.enable_cors`. False sends nothing, which is same-origin only.
    pub enabled: bool,
    /// `security.cors_allowed_origins`. Empty is also same-origin only.
    pub allowed_origins: Vec<String>,
}

impl CorsConfig {
    /// Builds the policy from configuration, dropping what cannot be honoured.
    ///
    /// An unexpanded `${VAR}` means the environment the template expected was
    /// never provided. Kept as a literal it would sit in the allowlist matching
    /// nothing, so the operator would see a configured origin and a browser
    /// refusing it with no explanation.
    pub fn from_settings(enabled: bool, origins: &[String]) -> Self {
        let mut allowed_origins = Vec::new();

        for origin in origins {
            let origin = origin.trim();
            if origin.is_empty() {
                continue;
            }
            if origin.starts_with("${") {
                tracing::warn!(
                    "Ignoring CORS origin '{}': the environment variable it names is unset, so \
                     it would match nothing",
                    origin
                );
                continue;
            }
            if origin == WILDCARD {
                allowed_origins.push(WILDCARD.to_string());
                continue;
            }
            match normalize_origin(origin) {
                Some(normalized) => allowed_origins.push(normalized),
                None => tracing::warn!(
                    "Ignoring CORS origin '{}': expected scheme://host[:port] with no path",
                    origin
                ),
            }
        }

        if enabled && allowed_origins.iter().any(|o| o == WILDCARD) {
            tracing::warn!(
                "security.cors_allowed_origins contains \"*\", so engine responses are readable \
                 by any origin without credentials. A cross-origin caller cannot send its session \
                 cookie under a wildcard — name the origins to allow a signed-in one."
            );
        }

        Self {
            enabled,
            allowed_origins,
        }
    }

    fn decide(&self, origin: &str) -> Decision {
        if !self.enabled || self.allowed_origins.is_empty() {
            return Decision::Refuse;
        }
        if self.allowed_origins.iter().any(|o| o == WILDCARD) {
            return Decision::Wildcard;
        }
        match normalize_origin(origin) {
            Some(normalized) if self.allowed_origins.contains(&normalized) => {
                Decision::Allow(normalized)
            }
            _ => Decision::Refuse,
        }
    }
}

const WILDCARD: &str = "*";

/// How long a browser may cache a preflight result.
const PREFLIGHT_MAX_AGE_SECS: &str = "600";

/// The methods the engine's own endpoints use.
const ALLOWED_METHODS: &str = "GET, POST, PUT, PATCH, DELETE, OPTIONS";

/// Sent when a preflight names no headers of its own.
const DEFAULT_ALLOWED_HEADERS: &str = "content-type, authorization";

#[derive(Debug, PartialEq, Eq)]
enum Decision {
    /// Echo this origin and allow credentials with it.
    Allow(String),
    /// Any origin, but without credentials — see the module note.
    Wildcard,
    /// Send nothing, which is what makes a browser refuse the read.
    Refuse,
}

/// Lower-cases scheme and host, drops a trailing slash, and rejects anything
/// carrying a path, query or fragment.
///
/// Comparison has to be exact — an allowlist that matched by prefix would let
/// `https://example.com.attacker.test` through — so both sides are put in the
/// one form a browser sends.
fn normalize_origin(origin: &str) -> Option<String> {
    let origin = origin.trim().trim_end_matches('/');
    if origin.eq_ignore_ascii_case("null") {
        // The opaque origin a sandboxed frame or a `file://` page sends. It
        // names no one, so it can never be on an allowlist.
        return None;
    }

    let (scheme, rest) = origin.split_once("://")?;
    if scheme.is_empty() || rest.is_empty() {
        return None;
    }
    if !scheme
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '-' || c == '.')
    {
        return None;
    }
    // An origin is scheme, host and port. Anything after the authority means
    // the operator wrote a URL where an origin was wanted.
    if rest.contains('/') || rest.contains('?') || rest.contains('#') {
        return None;
    }
    if rest.contains('@') {
        return None;
    }

    // Host and port too, not just the scheme: a host is case-insensitive and a
    // browser sends it lower-cased, so an allowlist written with capitals would
    // otherwise match nothing. Digits and `:` are unaffected, and so are the
    // hex digits of a bracketed IPv6 literal.
    Some(format!(
        "{}://{}",
        scheme.to_ascii_lowercase(),
        rest.to_ascii_lowercase()
    ))
}

/// Whether this path installs a CORS policy of its own.
///
/// The OAuth2 protocol endpoints do, and it is deliberately wide: browser-based
/// MCP clients reach them from origins nobody can enumerate, and a PKCE code
/// exchange carries no cookie for a permissive policy to expose.
///
/// Leaving them out of this layer is not the same as letting their own layer
/// win by `set_if_absent`. A preflight is answered here and never reaches the
/// router, so a policy applied further in would never run at all — an origin
/// outside the engine's allowlist would get a 204 with no headers, and every
/// such client would break.
fn owns_its_cors_policy(path: &str) -> bool {
    [
        crate::auth::routes::AUTHORIZE_PATH,
        crate::auth::routes::TOKEN_PATH,
        crate::auth::routes::CONSENT_PATH,
    ]
    .contains(&path)
}

/// Whether this path belongs to the engine rather than to a script.
fn is_engine_path(path: &str) -> bool {
    crate::engine_api::RESERVED_ROUTE_PREFIXES
        .iter()
        .any(|prefix| path == *prefix || path.starts_with(&format!("{}/", prefix)))
}

/// Whether this is a preflight rather than a real request.
fn is_preflight(request: &Request) -> bool {
    request.method() == Method::OPTIONS
        && request
            .headers()
            .contains_key(header::ACCESS_CONTROL_REQUEST_METHOD)
}

fn set_if_absent(response: &mut Response, name: HeaderName, value: &str) {
    if response.headers().contains_key(&name) {
        return;
    }
    match HeaderValue::from_str(value) {
        Ok(value) => {
            response.headers_mut().insert(name, value);
        }
        Err(e) => tracing::warn!("Refusing to set malformed CORS header {}: {}", name, e),
    }
}

/// Adds `Origin` to `Vary` without dropping what is already there.
///
/// The response differs by request origin, so a shared cache that ignored this
/// could hand one origin's allowance to another.
fn vary_on_origin(response: &mut Response) {
    let existing = response
        .headers()
        .get(header::VARY)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let value = match existing {
        Some(existing)
            if existing
                .split(',')
                .any(|part| part.trim().eq_ignore_ascii_case("origin")) =>
        {
            return;
        }
        Some(existing) => format!("{}, origin", existing),
        None => "origin".to_string(),
    };

    if let Ok(value) = HeaderValue::from_str(&value) {
        response.headers_mut().insert(header::VARY, value);
    }
}

fn apply(response: &mut Response, decision: &Decision) {
    match decision {
        Decision::Allow(origin) => {
            set_if_absent(response, header::ACCESS_CONTROL_ALLOW_ORIGIN, origin);
            set_if_absent(response, header::ACCESS_CONTROL_ALLOW_CREDENTIALS, "true");
        }
        Decision::Wildcard => {
            set_if_absent(response, header::ACCESS_CONTROL_ALLOW_ORIGIN, WILDCARD);
        }
        Decision::Refuse => {}
    }
}

/// Applies the engine's CORS policy to its own paths.
pub async fn cors_middleware(
    State(config): State<Arc<CorsConfig>>,
    request: Request,
    next: Next,
) -> Response {
    let origin = request
        .headers()
        .get(header::ORIGIN)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    // Not a cross-origin request, not our path, or nothing configured: the
    // request is none of this layer's business.
    let Some(origin) = origin else {
        return next.run(request).await;
    };
    let path = request.uri().path();
    if !config.enabled || !is_engine_path(path) || owns_its_cors_policy(path) {
        return next.run(request).await;
    }

    let decision = config.decide(&origin);
    let preflight = is_preflight(&request);
    let requested_headers = request
        .headers()
        .get(header::ACCESS_CONTROL_REQUEST_HEADERS)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    if preflight {
        // Answered here rather than by the router. A preflight is an OPTIONS to
        // a path whose handler only takes GET or POST, so letting it through
        // would earn a 405 and the browser would call that a CORS failure.
        let mut response = Response::builder()
            .status(StatusCode::NO_CONTENT)
            .body(axum::body::Body::empty())
            .unwrap_or_default();

        vary_on_origin(&mut response);
        if decision != Decision::Refuse {
            apply(&mut response, &decision);
            set_if_absent(
                &mut response,
                header::ACCESS_CONTROL_ALLOW_METHODS,
                ALLOWED_METHODS,
            );
            // Reflected: the origin is already allowed, and the engine's own
            // endpoints take a wider set of request headers than it is worth
            // enumerating here.
            set_if_absent(
                &mut response,
                header::ACCESS_CONTROL_ALLOW_HEADERS,
                requested_headers
                    .as_deref()
                    .unwrap_or(DEFAULT_ALLOWED_HEADERS),
            );
            set_if_absent(
                &mut response,
                header::ACCESS_CONTROL_MAX_AGE,
                PREFLIGHT_MAX_AGE_SECS,
            );
        }
        return response;
    }

    let mut response = next.run(request).await;
    vary_on_origin(&mut response);
    apply(&mut response, &decision);
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(origins: &[&str]) -> CorsConfig {
        CorsConfig::from_settings(
            true,
            &origins.iter().map(|o| o.to_string()).collect::<Vec<_>>(),
        )
    }

    #[test]
    fn an_allowlisted_origin_is_echoed() {
        let c = config(&["https://admin.example.com"]);
        assert_eq!(
            c.decide("https://admin.example.com"),
            Decision::Allow("https://admin.example.com".to_string())
        );
    }

    #[test]
    fn an_origin_that_is_not_listed_is_refused() {
        let c = config(&["https://admin.example.com"]);
        assert_eq!(c.decide("https://elsewhere.example.com"), Decision::Refuse);
    }

    /// The failure an allowlist exists to prevent: a matching prefix or suffix
    /// is not a matching origin.
    #[test]
    fn a_lookalike_origin_is_refused() {
        let c = config(&["https://example.com"]);
        for impostor in [
            "https://example.com.attacker.test",
            "https://notexample.com",
            "https://example.com:8443",
            "http://example.com",
            "https://sub.example.com",
        ] {
            assert_eq!(c.decide(impostor), Decision::Refuse, "{impostor}");
        }
    }

    #[test]
    fn matching_ignores_case_and_a_trailing_slash() {
        let c = config(&["https://Admin.Example.com/"]);
        assert_eq!(
            c.decide("https://admin.example.com"),
            Decision::Allow("https://admin.example.com".to_string())
        );
    }

    #[test]
    fn the_wildcard_allows_anyone_without_credentials() {
        let c = config(&["*"]);
        assert_eq!(c.decide("https://anywhere.example.com"), Decision::Wildcard);
    }

    #[test]
    fn nothing_configured_is_same_origin_only() {
        assert_eq!(config(&[]).decide("https://example.com"), Decision::Refuse);

        let disabled = CorsConfig::from_settings(false, &["https://example.com".to_string()]);
        assert_eq!(disabled.decide("https://example.com"), Decision::Refuse);
    }

    /// An unexpanded placeholder is not an origin, and keeping it would show an
    /// operator a configured entry that can never match.
    #[test]
    fn an_unexpanded_placeholder_is_dropped() {
        let c = config(&["${APP_SECURITY__CORS_ORIGIN_1}", "https://real.example.com"]);
        assert_eq!(c.allowed_origins, vec!["https://real.example.com"]);
    }

    #[test]
    fn a_url_is_not_an_origin() {
        for not_an_origin in [
            "example.com",
            "https://example.com/admin",
            "https://example.com?x=1",
            "https://user@example.com",
            "null",
        ] {
            assert!(normalize_origin(not_an_origin).is_none(), "{not_an_origin}");
        }
    }

    /// The OAuth2 endpoints are skipped rather than merely not overwritten,
    /// because a preflight never reaches the layer that would answer it.
    #[test]
    fn the_oauth2_endpoints_keep_their_own_policy() {
        for owned in [
            "/auth/oauth2/authorize",
            "/auth/oauth2/token",
            "/auth/oauth2/consent",
        ] {
            assert!(owns_its_cors_policy(owned), "{owned}");
        }
        for ours in ["/auth/login", "/auth/oauth2", "/engine/scripts"] {
            assert!(!owns_its_cors_policy(ours), "{ours}");
        }
    }

    #[test]
    fn only_engine_paths_are_covered() {
        for engine in ["/engine/scripts", "/auth/login", "/mcp", "/graphql"] {
            assert!(is_engine_path(engine), "{engine}");
        }
        for script in ["/", "/shop/items", "/engineering", "/mcpx"] {
            assert!(!is_engine_path(script), "{script}");
        }
    }
}
