//! Refusing state-changing requests a browser made on another site's behalf.
//!
//! A session cookie is `SameSite=Lax`, which keeps it off a cross-*site*
//! `POST` but not off a cross-*origin* one from the same site — and this engine
//! serves several hosts that are one site to a browser. A page on one host
//! could otherwise make a signed-in visitor's browser `POST` to a script on
//! another, carrying their session. A script defending itself needs a token
//! scheme, a place to keep the token and a check on every route that changes
//! something; nearly every script gets one of those wrong or skips it.
//!
//! So the engine decides, from what the browser says about where a request
//! came from. `Sec-Fetch-Site` is set by the browser and not by page script,
//! and every browser in use sends it; where it is missing, `Origin` is compared
//! with the host the request arrived on. A request carrying neither is not
//! from a browser page and has no ambient credential to abuse, so it passes —
//! which is what keeps webhooks and command-line clients working.

use std::collections::HashMap;

/// Methods that must not change anything, and so are never refused here.
fn is_safe_method(method: &str) -> bool {
    matches!(
        method.to_ascii_uppercase().as_str(),
        "GET" | "HEAD" | "OPTIONS"
    )
}

/// Why a request was refused, for the response and the log; `None` to let it
/// through.
///
/// `host` is the `Host` the request arrived on. Header names in `headers` are
/// matched case-insensitively.
pub fn refusal(
    method: &str,
    host: Option<&str>,
    headers: &HashMap<String, String>,
) -> Option<String> {
    if is_safe_method(method) {
        return None;
    }

    let header = |name: &str| {
        headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.trim())
    };

    if let Some(site) = header("sec-fetch-site") {
        return match site.to_ascii_lowercase().as_str() {
            // `none` is the person acting directly: a bookmark, the address bar.
            "same-origin" | "none" => None,
            other => Some(format!(
                "a {} request from another origin ({}) was refused",
                method.to_ascii_uppercase(),
                other
            )),
        };
    }

    let origin = header("origin")?;
    let origin_host = origin
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(origin)
        .trim_end_matches('/');
    match host {
        Some(host) if origin_host.eq_ignore_ascii_case(host.trim()) => None,
        _ => Some(format!(
            "a {} request from origin {} was refused",
            method.to_ascii_uppercase(),
            origin
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect()
    }

    #[test]
    fn a_safe_method_is_never_refused() {
        let cross = headers(&[("sec-fetch-site", "cross-site")]);
        for method in ["GET", "head", "OPTIONS"] {
            assert!(refusal(method, Some("a.example"), &cross).is_none());
        }
    }

    #[test]
    fn the_browser_saying_where_a_request_came_from_decides() {
        for (site, refused) in [
            ("same-origin", false),
            ("none", false),
            ("same-site", true),
            ("cross-site", true),
        ] {
            let decided = refusal(
                "POST",
                Some("a.example"),
                &headers(&[("Sec-Fetch-Site", site)]),
            );
            assert_eq!(decided.is_some(), refused, "Sec-Fetch-Site: {site}");
        }
    }

    #[test]
    fn without_fetch_metadata_the_origin_must_be_the_host() {
        let same = headers(&[("origin", "https://a.example")]);
        assert!(refusal("POST", Some("a.example"), &same).is_none());
        assert!(refusal("POST", Some("A.EXAMPLE"), &same).is_none());

        let sibling = headers(&[("origin", "https://b.example")]);
        assert!(refusal("POST", Some("a.example"), &sibling).is_some());

        let opaque = headers(&[("origin", "null")]);
        assert!(refusal("DELETE", Some("a.example"), &opaque).is_some());
    }

    #[test]
    fn a_request_that_is_not_from_a_page_passes() {
        assert!(refusal("POST", Some("a.example"), &HashMap::new()).is_none());
    }
}
