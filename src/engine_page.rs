//! The HTML pages the engine renders itself, and the stylesheet behind them.
//!
//! Every such page — sign-in, account, consent, delegation, elevation, the
//! permission and install pages — shares one shell and one sheet,
//! `assets/engine.css`, whose custom properties are the vocabulary the pages are
//! written in. A page with a `<style>` block of its own is how designs
//! diverge.
//!
//! The sheet is inlined rather than linked. A page is often shown because
//! something has gone wrong — a refused permission, a failed sign-in — and one
//! that also needs a second request to succeed before it is legible is worse at
//! the one thing it is for. The same bytes are published at [`STYLESHEET_PATH`]
//! for scripts that want their own pages to match; see `docs/ENGINE_STYLES.md`.

use std::sync::OnceLock;

use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

/// The engine's stylesheet, compiled into the binary.
pub const STYLESHEET: &str = include_str!("../assets/engine.css");

/// Where [`STYLESHEET`] is published. Under `/engine` so that no script can
/// register a route that shadows it, and served on every host rather than
/// only on management hosts, since it is for every host's pages.
pub const STYLESHEET_PATH: &str = "/engine/engine.css";

/// The account menu's script, compiled into the binary. It defines
/// `<aw-account-menu>`, a person button in the top right that shows who is
/// signed in and offers the account page and sign-out; a page can put its own
/// items in it by nesting them inside the element.
pub const SCRIPT: &str = include_str!("../assets/engine.js");

/// Where [`SCRIPT`] is published, for the same reasons as [`STYLESHEET_PATH`].
pub const SCRIPT_PATH: &str = "/engine/engine.js";

/// The account menu, for a page only a signed-in person sees. Appended to the
/// body: the script is external (the policy allows `'self'`) and the element
/// positions itself in the corner of the viewport.
pub const ACCOUNT_MENU: &str = r#"<aw-account-menu></aw-account-menu>
    <script src="/engine/engine.js"></script>"#;

/// How wide a page's card is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Width {
    /// A form or a short message.
    Narrow,
    /// A page listing things, where a row holds a description and a control.
    Wide,
}

/// A complete engine page: one card, centred, under the shared stylesheet.
///
/// `title` is text and is escaped here. `body` is the card's contents and is
/// HTML the caller has already escaped. The `<style>` block carries `nonce`,
/// which the response's policy must name — use [`response`].
pub fn document(title: &str, nonce: &str, width: Width, body: &str) -> String {
    document_with_head(title, nonce, width, "", body)
}

/// [`document`], with extra elements for the `<head>`.
pub fn document_with_head(
    title: &str,
    nonce: &str,
    width: Width,
    head: &str,
    body: &str,
) -> String {
    let card = match width {
        Width::Narrow => "aw-card",
        Width::Wide => "aw-card aw-card--wide",
    };
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
    <meta charset="UTF-8">
    <meta name="viewport" content="width=device-width, initial-scale=1.0">
    <title>{title}</title>
    <link rel="icon" type="image/x-icon" href="/favicon.ico">{head}
    <style nonce="{nonce}">
{STYLESHEET}    </style>
</head>
<body class="aw-page">
    <main class="{card}">
{body}
    </main>
</body>
</html>"#,
        title = html_escape::encode_text(title),
        nonce = html_escape::encode_double_quoted_attribute(nonce),
    )
}

/// Serve an engine page under a policy naming its own inline blocks.
///
/// Set here rather than by the security-headers layer because only this side
/// knows the nonce it wrote into the markup. The layer fills in a header a
/// response did not set, so this wins.
pub fn response(status: StatusCode, html: String, nonce: &str) -> Response {
    match HeaderValue::from_str(&crate::security::engine_page_policy(nonce)) {
        Ok(policy) => {
            let mut response = (
                status,
                [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response();
            response
                .headers_mut()
                .insert(header::CONTENT_SECURITY_POLICY, policy);
            response
        }
        Err(e) => {
            // Serving the page without its policy would let an injected inline
            // block run, which is the thing the nonce exists to prevent.
            tracing::error!("Could not build a content security policy: {}", e);
            (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error").into_response()
        }
    }
}

/// An asset's entity tag: a digest of its bytes.
///
/// The paths carry no version, because a script linking one wants the look of
/// whatever engine is serving it and a versioned path would 404 after every
/// upgrade. So a browser revalidates instead, and an unchanged asset costs a
/// 304.
fn etag_of(body: &str) -> String {
    let digest = Sha256::digest(body.as_bytes());
    format!("\"{}\"", hex::encode(&digest[..16]))
}

fn stylesheet_etag() -> &'static str {
    static ETAG: OnceLock<String> = OnceLock::new();
    ETAG.get_or_init(|| etag_of(STYLESHEET))
}

fn script_etag() -> &'static str {
    static ETAG: OnceLock<String> = OnceLock::new();
    ETAG.get_or_init(|| etag_of(SCRIPT))
}

/// Serve `body`, or a 304 when the client already holds the copy `etag` names.
fn revalidated(
    headers: &HeaderMap,
    etag: &'static str,
    content_type: &'static str,
    body: &'static str,
) -> Response {
    let cache_headers = [
        (header::ETAG, etag),
        (header::CACHE_CONTROL, "public, no-cache"),
    ];

    let unchanged = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(',')
                .map(str::trim)
                .any(|candidate| candidate == etag || candidate == "*")
        });
    if unchanged {
        return (StatusCode::NOT_MODIFIED, cache_headers).into_response();
    }

    (
        StatusCode::OK,
        cache_headers,
        [(header::CONTENT_TYPE, content_type)],
        body,
    )
        .into_response()
}

/// The engine's stylesheet, for scripts whose pages should match the engine's.
#[utoipa::path(
    get,
    path = "/engine/engine.css",
    tags = ["Assets"],
    responses(
        (status = 200, description = "The stylesheet behind the engine's own pages", content_type = "text/css"),
        (status = 304, description = "Unchanged since the copy the client holds"),
    )
)]
pub async fn stylesheet_route(headers: HeaderMap) -> Response {
    revalidated(
        &headers,
        stylesheet_etag(),
        "text/css; charset=utf-8",
        STYLESHEET,
    )
}

/// The account menu's script, for engine pages and for scripts' own.
#[utoipa::path(
    get,
    path = "/engine/engine.js",
    tags = ["Assets"],
    responses(
        (status = 200, description = "The script defining the account menu element", content_type = "text/javascript"),
        (status = 304, description = "Unchanged since the copy the client holds"),
    )
)]
pub async fn script_route(headers: HeaderMap) -> Response {
    revalidated(
        &headers,
        script_etag(),
        "text/javascript; charset=utf-8",
        SCRIPT,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_page_carries_the_sheet_under_its_nonce() {
        let html = document("Sign in", "abc", Width::Narrow, "<p>hi</p>");
        assert!(html.contains(r#"<style nonce="abc">"#));
        assert!(html.contains("--aw-color-bg"));
        assert!(html.contains(r#"<main class="aw-card">"#));
    }

    #[tokio::test]
    async fn the_script_is_served_as_javascript() {
        let response = script_route(HeaderMap::new()).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::CONTENT_TYPE),
            Some(&HeaderValue::from_static("text/javascript; charset=utf-8"))
        );
    }

    #[test]
    fn a_title_is_text() {
        let html = document("<script>", "n", Width::Narrow, "");
        assert!(html.contains("<title>&lt;script&gt;</title>"));
    }

    #[test]
    fn a_nonce_cannot_leave_its_attribute() {
        let html = document("t", "\"><script>", Width::Narrow, "");
        assert!(!html.contains(r#""><script>"#));
    }

    #[tokio::test]
    async fn the_sheet_revalidates_to_a_304() {
        let first = stylesheet_route(HeaderMap::new()).await;
        assert_eq!(first.status(), StatusCode::OK);
        let etag = first
            .headers()
            .get(header::ETAG)
            .cloned()
            .unwrap_or_else(|| HeaderValue::from_static("missing"));

        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, etag);
        let second = stylesheet_route(headers).await;
        assert_eq!(second.status(), StatusCode::NOT_MODIFIED);
    }
}
