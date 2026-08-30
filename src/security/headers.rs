//! Response headers the engine sets on its own behalf.
//!
//! `security.enable_security_headers` and `security.content_security_policy`
//! were configuration that did nothing: both were parsed into `AppConfig` and
//! then read by no one, so no response carried either. This is where they take
//! effect.
//!
//! Two rules shape what goes where.
//!
//! **A header the response already set is never replaced.** Scripts choose
//! their own response headers, and a solution that has thought about its
//! framing or its policy knows more about its own page than the engine does.
//!
//! **The Content-Security-Policy is only applied to the engine's own
//! responses.** A policy is a statement about how a particular page is built,
//! and the engine can only make that statement about pages it wrote. Applying
//! `script-src 'self'` to every script-served response would break any solution
//! with an inline `<script>` — which is most of them — in exchange for a
//! promise the engine is not in a position to keep. Solutions set their own.
//!
//! The headers that *are* global are the ones that say nothing about how a page
//! is built: don't guess content types, don't leak the full URL in a referrer,
//! and keep using HTTPS.

use axum::extract::{Request, State};
use axum::http::{HeaderName, HeaderValue, header};
use axum::middleware::Next;
use axum::response::Response;
use std::sync::Arc;

/// What the layer was configured to send.
#[derive(Debug, Clone)]
pub struct SecurityHeadersConfig {
    /// `security.enable_security_headers`. False disables the whole layer,
    /// including the policy.
    pub enabled: bool,
    /// `security.content_security_policy`, applied to engine-owned paths.
    pub content_security_policy: Option<String>,
}

/// How long a browser should remember to use HTTPS for this host.
///
/// Only ever sent on a request that already arrived over HTTPS. Sending it
/// over plaintext is meaningless — the header is not trustworthy there — and
/// on a development instance served over HTTP it would be a footgun that
/// outlives the instance.
const HSTS_VALUE: &str = "max-age=31536000; includeSubDomains";

/// Whether this path belongs to the engine rather than to a script.
///
/// Reuses the prefixes scripts are already forbidden from registering under
/// ([`crate::engine_api::RESERVED_ROUTE_PREFIXES`]), so the two answers to
/// "whose page is this" cannot drift apart.
fn is_engine_path(path: &str) -> bool {
    crate::engine_api::RESERVED_ROUTE_PREFIXES
        .iter()
        .any(|prefix| path == *prefix || path.starts_with(&format!("{}/", prefix)))
}

/// Whether the request reached the engine over HTTPS.
///
/// The engine usually sits behind a reverse proxy that terminates TLS, so the
/// request's own scheme says http even when the browser used https. The
/// proxy's `X-Forwarded-Proto` is what carries the truth, and it is only
/// trustworthy because nothing but the proxy should be able to reach the
/// engine directly.
fn is_https(request: &Request) -> bool {
    if let Some(proto) = request
        .headers()
        .get("x-forwarded-proto")
        .and_then(|value| value.to_str().ok())
        && proto
            .split(',')
            .next()
            .is_some_and(|first| first.trim().eq_ignore_ascii_case("https"))
    {
        return true;
    }

    request.uri().scheme_str() == Some("https")
}

/// Set a header, unless the response already carries one.
fn set_if_absent(response: &mut Response, name: HeaderName, value: &str) {
    if response.headers().contains_key(&name) {
        return;
    }
    match HeaderValue::from_str(value) {
        Ok(value) => {
            response.headers_mut().insert(name, value);
        }
        Err(e) => {
            tracing::warn!("Refusing to set malformed security header {}: {}", name, e);
        }
    }
}

/// Add the engine's security headers to a response.
pub async fn security_headers_middleware(
    State(config): State<Arc<SecurityHeadersConfig>>,
    request: Request,
    next: Next,
) -> Response {
    if !config.enabled {
        return next.run(request).await;
    }

    let path = request.uri().path().to_string();
    let https = is_https(&request);

    let mut response = next.run(request).await;

    // Safe everywhere: none of these describe how a page is built.
    set_if_absent(&mut response, header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    set_if_absent(
        &mut response,
        header::REFERRER_POLICY,
        "strict-origin-when-cross-origin",
    );
    if https {
        set_if_absent(&mut response, header::STRICT_TRANSPORT_SECURITY, HSTS_VALUE);
    }

    // The policy, only for pages the engine wrote. Engine pages carrying
    // inline styles or scripts set a nonce policy of their own before reaching
    // here, and `set_if_absent` leaves it alone.
    if let Some(policy) = config.content_security_policy.as_deref()
        && is_engine_path(&path)
    {
        set_if_absent(&mut response, header::CONTENT_SECURITY_POLICY, policy);
    }

    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_paths_are_recognised() {
        for path in [
            "/engine",
            "/engine/scripts",
            "/auth/login",
            "/mcp",
            "/graphql",
            "/.well-known/oauth-authorization-server",
            "/health",
        ] {
            assert!(is_engine_path(path), "{} is the engine's", path);
        }
    }

    /// A path that merely starts with the same letters belongs to whoever
    /// registered it, and must not be handed the engine's policy.
    #[test]
    fn a_script_path_is_not_an_engine_path_by_prefix_alone() {
        for path in [
            "/",
            "/engineering",
            "/authors",
            "/mcpx",
            "/healthy",
            "/game/engine",
        ] {
            assert!(!is_engine_path(path), "{} is not the engine's", path);
        }
    }
}
