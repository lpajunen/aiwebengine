mod common;

use common::setup_env;

mod mock_server;

use aiwebengine::http_client::{FetchOptions, HttpClient};
use mock_server::MockServer;
use std::collections::HashMap;

#[tokio::test]
async fn test_fetch_get_request() {
    let mock = MockServer::start()
        .await
        .expect("Failed to start mock server");
    let url = mock.url("/get");

    let result = tokio::task::spawn_blocking(move || {
        let client = HttpClient::new_for_tests().expect("Failed to create client");
        client.fetch(url, FetchOptions::default(), None, None)
    })
    .await
    .expect("Task panicked");

    assert!(
        result.is_ok(),
        "GET request should succeed: {:?}",
        result.err()
    );
    let response = result.unwrap();
    assert_eq!(response.status, 200);
    assert!(response.ok);
    assert!(!response.body.is_empty());

    mock.shutdown().await;
}

#[tokio::test]
async fn test_fetch_post_with_json() {
    let mock = MockServer::start()
        .await
        .expect("Failed to start mock server");
    let url = mock.url("/post");

    let result = tokio::task::spawn_blocking(move || {
        let client = HttpClient::new_for_tests().expect("Failed to create client");
        let mut headers = HashMap::new();
        headers.insert("Content-Type".to_string(), "application/json".to_string());
        let body = r#"{"test": "data", "number": 42}"#;

        client.fetch(
            url,
            FetchOptions {
                method: "POST".to_string(),
                headers: Some(headers),
                body: Some(body.to_string()),
                timeout_ms: None,
                ..Default::default()
            },
            None,
            None,
        )
    })
    .await
    .expect("Task panicked");

    assert!(result.is_ok(), "POST request should succeed");
    let response = result.unwrap();
    assert_eq!(response.status, 200);
    assert!(response.ok);

    // Mock server echoes back the JSON we sent
    assert!(response.body.contains("test"));
    assert!(response.body.contains("data"));

    mock.shutdown().await;
}

#[tokio::test]
async fn test_fetch_custom_headers() {
    let mock = MockServer::start()
        .await
        .expect("Failed to start mock server");
    let url = mock.url("/headers");

    let result = tokio::task::spawn_blocking(move || {
        let client = HttpClient::new_for_tests().expect("Failed to create client");
        let mut headers = HashMap::new();
        headers.insert("x-custom-header".to_string(), "test-value".to_string());
        headers.insert("user-agent".to_string(), "aiwebengine/test".to_string());

        client.fetch(
            url,
            FetchOptions {
                method: "GET".to_string(),
                headers: Some(headers),
                body: None,
                timeout_ms: None,
                ..Default::default()
            },
            None,
            None,
        )
    })
    .await
    .expect("Task panicked");

    assert!(result.is_ok(), "Request with custom headers should succeed");
    let response = result.unwrap();
    assert_eq!(response.status, 200);

    // Mock server echoes back our headers (case-insensitive)
    assert!(
        response.body.to_lowercase().contains("x-custom-header")
            || response.body.contains("X-Custom-Header")
    );
    assert!(response.body.contains("test-value"));

    mock.shutdown().await;
}

/// A gzipped answer reads as text, and the headers stop describing the body
/// that was sent in favour of the one the caller gets.
#[tokio::test]
async fn test_fetch_decodes_gzip() {
    let mock = MockServer::start()
        .await
        .expect("Failed to start mock server");
    let url = mock.url("/compressed/gzip");

    let response = tokio::task::spawn_blocking(move || {
        let client = HttpClient::new_for_tests().expect("Failed to create client");
        client.fetch(url, FetchOptions::default(), None, None)
    })
    .await
    .expect("Task panicked")
    .expect("gzip response should be decoded");

    assert_eq!(response.status, 200);
    assert!(
        response.body.contains("compressed payload"),
        "body should be inflated, got: {}",
        response.body
    );
    // The request offered gzip without being asked to.
    assert!(
        response.body.contains("gzip"),
        "client should advertise gzip: {}",
        response.body
    );
    assert!(
        !response.headers.contains_key("content-encoding"),
        "content-encoding describes a body the caller no longer has"
    );
    assert!(
        !response.headers.contains_key("content-length"),
        "content-length counted the compressed bytes"
    );

    mock.shutdown().await;
}

/// Both shapes of `deflate` in the wild: zlib-wrapped, and the raw stream.
#[tokio::test]
async fn test_fetch_decodes_deflate() {
    let mock = MockServer::start()
        .await
        .expect("Failed to start mock server");

    for path in ["/compressed/deflate", "/compressed/raw-deflate"] {
        let url = mock.url(path);
        let response = tokio::task::spawn_blocking(move || {
            let client = HttpClient::new_for_tests().expect("Failed to create client");
            client.fetch(url, FetchOptions::default(), None, None)
        })
        .await
        .expect("Task panicked")
        .unwrap_or_else(|e| panic!("{} should be decoded: {}", path, e));

        assert!(
            response.body.contains("compressed payload"),
            "{} should be inflated, got: {}",
            path,
            response.body
        );
    }

    mock.shutdown().await;
}

/// The case this was built for: an API that answers only when the request
/// carries its own `Accept-Encoding`. The caller's header is sent as written,
/// and the answer is still decoded — decoding follows the response's
/// `Content-Encoding`, not what the client happened to ask for.
#[tokio::test]
async fn test_fetch_decodes_gzip_with_caller_supplied_accept_encoding() {
    let mock = MockServer::start()
        .await
        .expect("Failed to start mock server");
    let url = mock.url("/compressed/gzip");

    let response = tokio::task::spawn_blocking(move || {
        let client = HttpClient::new_for_tests().expect("Failed to create client");
        let mut headers = HashMap::new();
        headers.insert("Accept-Encoding".to_string(), "gzip".to_string());
        client.fetch(
            url,
            FetchOptions {
                method: "GET".to_string(),
                headers: Some(headers),
                body: None,
                timeout_ms: None,
                ..Default::default()
            },
            None,
            None,
        )
    })
    .await
    .expect("Task panicked")
    .expect("gzip response should be decoded");

    assert!(response.body.contains("compressed payload"));
    assert!(
        response.body.contains(r#""accept_encoding":"gzip""#),
        "the caller's header should reach the server unchanged: {}",
        response.body
    );

    mock.shutdown().await;
}

/// A coding this client never offered and cannot undo is named as such,
/// rather than surfacing as an unreadable body.
#[tokio::test]
async fn test_fetch_reports_unsupported_encoding() {
    let mock = MockServer::start()
        .await
        .expect("Failed to start mock server");
    let url = mock.url("/compressed/br");

    let result = tokio::task::spawn_blocking(move || {
        let client = HttpClient::new_for_tests().expect("Failed to create client");
        client.fetch(url, FetchOptions::default(), None, None)
    })
    .await
    .expect("Task panicked");

    let error = result.expect_err("brotli should be refused").to_string();
    assert!(
        error.contains("Unsupported Content-Encoding") && error.contains("br"),
        "error should name the coding: {}",
        error
    );

    mock.shutdown().await;
}

/// The response ceiling applies to what comes out of the decompressor, not
/// only to what arrived: a few kilobytes of gzip can inflate past it.
#[tokio::test]
async fn test_fetch_bounds_decompressed_size() {
    let mock = MockServer::start()
        .await
        .expect("Failed to start mock server");
    let url = mock.url("/compressed/bomb");

    let result = tokio::task::spawn_blocking(move || {
        let client = HttpClient::new_for_tests().expect("Failed to create client");
        client.fetch(url, FetchOptions::default(), None, None)
    })
    .await
    .expect("Task panicked");

    let error = result
        .expect_err("an over-sized inflated body should be refused")
        .to_string();
    assert!(
        error.contains("Response too large"),
        "error should name the size limit: {}",
        error
    );

    mock.shutdown().await;
}

#[test]
fn test_fetch_blocks_localhost() {
    let client = HttpClient::new().expect("Failed to create client");

    let result = client.fetch(
        "http://localhost:8080/api".to_string(),
        FetchOptions::default(),
        None,
        None,
    );

    assert!(result.is_err(), "Localhost should be blocked");
    let error = result.unwrap_err();
    assert!(error.to_string().contains("Localhost"));
}

#[test]
fn test_fetch_blocks_private_ip() {
    let client = HttpClient::new().expect("Failed to create client");

    let result = client.fetch(
        "http://192.168.1.1/api".to_string(),
        FetchOptions::default(),
        None,
        None,
    );

    assert!(result.is_err(), "Private IP should be blocked");
    let error = result.unwrap_err();
    assert!(error.to_string().contains("not allowed") || error.to_string().contains("Blocked"));
}

#[test]
fn test_fetch_blocks_127_0_0_1() {
    let client = HttpClient::new().expect("Failed to create client");

    let result = client.fetch(
        "http://127.0.0.1:3000/api".to_string(),
        FetchOptions::default(),
        None,
        None,
    );

    assert!(result.is_err(), "127.0.0.1 should be blocked");
    let error = result.unwrap_err();
    assert!(error.to_string().contains("not allowed") || error.to_string().contains("Blocked"));
}

#[test]
fn test_fetch_invalid_url_scheme() {
    let client = HttpClient::new().expect("Failed to create client");

    let result = client.fetch(
        "ftp://example.com/file".to_string(),
        FetchOptions::default(),
        None,
        None,
    );

    assert!(result.is_err(), "FTP scheme should be rejected");
    let error = result.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("only http and https are allowed")
    );
}

#[test]
fn test_fetch_file_scheme_blocked() {
    let client = HttpClient::new().expect("Failed to create client");

    let result = client.fetch(
        "file:///etc/passwd".to_string(),
        FetchOptions::default(),
        None,
        None,
    );

    assert!(result.is_err(), "File scheme should be rejected");
}

#[test]
fn test_fetch_invalid_url() {
    let client = HttpClient::new().expect("Failed to create client");

    let result = client.fetch(
        "not-a-valid-url".to_string(),
        FetchOptions::default(),
        None,
        None,
    );

    assert!(result.is_err(), "Invalid URL should be rejected");
}

#[tokio::test]
async fn test_fetch_404_not_found() {
    let mock = MockServer::start()
        .await
        .expect("Failed to start mock server");
    let url = mock.url("/status/404");

    let result = tokio::task::spawn_blocking(move || {
        let client = HttpClient::new_for_tests().expect("Failed to create client");
        client.fetch(url, FetchOptions::default(), None, None)
    })
    .await
    .expect("Task panicked");

    assert!(
        result.is_ok(),
        "Request should succeed even with 404 status"
    );
    let response = result.unwrap();
    assert_eq!(response.status, 404);
    assert!(!response.ok); // ok should be false for 4xx/5xx

    mock.shutdown().await;
}

#[tokio::test]
async fn test_fetch_different_methods() {
    let mock = MockServer::start()
        .await
        .expect("Failed to start mock server");
    let base_url = format!("http://127.0.0.1:{}", mock.port);

    tokio::task::spawn_blocking(move || {
        let client = HttpClient::new_for_tests().expect("Failed to create client");

        // Test PUT
        let result = client.fetch(
            format!("{}/put", base_url),
            FetchOptions {
                method: "PUT".to_string(),
                headers: None,
                body: Some("test data".to_string()),
                timeout_ms: Some(10000),
                ..Default::default()
            },
            None,
            None,
        );
        assert!(result.is_ok(), "PUT request failed: {:?}", result.err());
        let response = result.unwrap();
        assert_eq!(
            response.status, 200,
            "PUT request returned unexpected status"
        );
        assert!(response.ok, "PUT request ok flag should be true");

        // Test DELETE
        let result = client.fetch(
            format!("{}/delete", base_url),
            FetchOptions {
                method: "DELETE".to_string(),
                headers: None,
                body: None,
                timeout_ms: Some(10000),
                ..Default::default()
            },
            None,
            None,
        );
        assert!(result.is_ok(), "DELETE request failed: {:?}", result.err());
        let response = result.unwrap();
        assert_eq!(
            response.status, 200,
            "DELETE request returned unexpected status"
        );
        assert!(response.ok, "DELETE request ok flag should be true");

        // Test PATCH
        let result = client.fetch(
            format!("{}/patch", base_url),
            FetchOptions {
                method: "PATCH".to_string(),
                headers: None,
                body: Some("patch data".to_string()),
                timeout_ms: Some(10000),
                ..Default::default()
            },
            None,
            None,
        );
        assert!(result.is_ok(), "PATCH request failed: {:?}", result.err());
        let response = result.unwrap();
        assert_eq!(
            response.status, 200,
            "PATCH request returned unexpected status"
        );
        assert!(response.ok, "PATCH request ok flag should be true");
    })
    .await
    .expect("Task panicked");

    mock.shutdown().await;
}

#[tokio::test]
async fn test_fetch_response_headers() {
    let mock = MockServer::start()
        .await
        .expect("Failed to start mock server");
    let url = mock.url("/response-headers?custom-header=test-value");

    let result = tokio::task::spawn_blocking(move || {
        let client = HttpClient::new_for_tests().expect("Failed to create client");
        client.fetch(url, FetchOptions::default(), None, None)
    })
    .await
    .expect("Task panicked");

    assert!(result.is_ok());
    let response = result.unwrap();
    assert!(!response.headers.is_empty());

    // Mock server should return our custom header
    assert!(response.headers.contains_key("custom-header"));

    mock.shutdown().await;
}

// Test secret injection using database-backed secrets
#[tokio::test(flavor = "multi_thread")]
async fn test_fetch_secret_template_syntax() {
    let mock = MockServer::start()
        .await
        .expect("Failed to start mock server");
    let url = mock.url("/headers");

    // Secrets are read from the database, so this one test needs it standing.
    use aiwebengine::repository;
    setup_env().await;

    // Store the secret in the script_secrets table
    let script_uri = "test://http-fetch";
    let _ = repository::set_script_secret_item(script_uri, "test_api_key", "secret-key-12345");

    let url_clone = url.clone();
    let result = tokio::task::spawn_blocking(move || {
        let client = HttpClient::new_for_tests().expect("Failed to create client");
        let mut headers = HashMap::new();
        headers.insert(
            "authorization".to_string(),
            "{{secret:test_api_key}}".to_string(),
        );

        client.fetch(
            url_clone,
            FetchOptions {
                method: "GET".to_string(),
                headers: Some(headers),
                body: None,
                timeout_ms: None,
                ..Default::default()
            },
            Some(script_uri),
            None,
        )
    })
    .await
    .expect("Task panicked");

    // Clean up
    let _ = repository::remove_script_secret_item(script_uri, "test_api_key");

    assert!(
        result.is_ok(),
        "Request with secret injection should succeed: {:?}",
        result.err()
    );
    let response = result.unwrap();
    assert_eq!(response.status, 200);

    // The secret value should have been injected
    assert!(response.body.contains("secret-key-12345"));
    assert!(response.body.to_lowercase().contains("authorization"));

    mock.shutdown().await;
}

/// The bearer-token shape: a prefix and a secret in one value. Matching only a
/// value that was nothing but a template put most APIs out of reach of a
/// stored key, and the script that hit it got a 401 explaining nothing.
#[tokio::test(flavor = "multi_thread")]
async fn test_fetch_secret_inside_a_header_value() {
    let mock = MockServer::start()
        .await
        .expect("Failed to start mock server");
    let url = mock.url("/headers");

    use aiwebengine::repository;
    setup_env().await;

    let script_uri = "test://http-fetch-inline";
    let _ = repository::set_script_secret_item(script_uri, "inline_token", "secret-key-12345");

    let result = tokio::task::spawn_blocking(move || {
        let client = HttpClient::new_for_tests().expect("Failed to create client");
        let mut headers = HashMap::new();
        headers.insert(
            "authorization".to_string(),
            "Bearer {{secret:inline_token}}".to_string(),
        );

        client.fetch(
            url,
            FetchOptions {
                method: "GET".to_string(),
                headers: Some(headers),
                body: None,
                timeout_ms: None,
                ..Default::default()
            },
            Some(script_uri),
            None,
        )
    })
    .await
    .expect("Task panicked");

    let _ = repository::remove_script_secret_item(script_uri, "inline_token");

    let response = result.expect("Request with an inline secret should succeed");
    assert_eq!(response.status, 200);
    assert!(
        response.body.contains("Bearer secret-key-12345"),
        "the prefix and the secret should both arrive: {}",
        response.body
    );
    assert!(
        !response.body.contains("{{secret:"),
        "no template text should reach the far end: {}",
        response.body
    );

    mock.shutdown().await;
}

/// The whole path, for a secret that lives in the URL rather than a header.
///
/// The Telegram Bot API is why this is allowed at all: it takes its token in
/// the path and offers no header to carry it, so "headers only" put that whole
/// class of API out of reach. What keeps it safe is that the template carries
/// the scheme and host, so every check runs against a string with no
/// credential in it — and that is what the unit tests beside `resolve_url`
/// pin. This one proves the byte actually arrives.
#[tokio::test(flavor = "multi_thread")]
async fn test_fetch_secret_inside_a_url_path() {
    let mock = MockServer::start()
        .await
        .expect("Failed to start mock server");

    use aiwebengine::repository;
    setup_env().await;

    let script_uri = "test://http-fetch-url-secret";
    let _ = repository::set_script_secret_item(script_uri, "bot_token", "12345:AAbbCC");

    // Shaped like Telegram's: the token sits between two fixed segments.
    let url = mock.url("/path/bot{{secret:bot_token}}");

    let result = tokio::task::spawn_blocking(move || {
        let client = HttpClient::new_for_tests().expect("Failed to create client");
        client.fetch(
            url,
            FetchOptions {
                method: "GET".to_string(),
                headers: None,
                body: None,
                timeout_ms: None,
                ..Default::default()
            },
            Some(script_uri),
            None,
        )
    })
    .await
    .expect("Task panicked");

    let _ = repository::remove_script_secret_item(script_uri, "bot_token");

    let response = result.expect("a secret in the path should resolve");
    assert_eq!(response.status, 200);
    assert_eq!(
        response.body, "bot12345:AAbbCC",
        "the secret should arrive in the path, with its surrounding text intact"
    );
    assert!(
        !response.body.contains("{{secret:"),
        "no template text should reach the far end: {}",
        response.body
    );

    mock.shutdown().await;
}

/// A URL naming a secret is refused when the execution may not read one.
///
/// The capability check reads the URL as well as the headers. Forgetting that
/// would be the whole of the hole: a context holding `use_network` and not
/// `read_secrets` would have its URL resolved because nothing asked.
#[tokio::test]
async fn test_a_url_template_is_refused_without_read_secrets() {
    use aiwebengine::http_client::names_a_secret;

    let bare = FetchOptions {
        method: "GET".to_string(),
        headers: None,
        body: None,
        timeout_ms: None,
        ..Default::default()
    };

    assert!(names_a_secret(
        "https://api.telegram.org/bot{{secret:t}}/sendMessage",
        &bare
    ));
    assert!(!names_a_secret("https://example.com/plain", &bare));
}

#[tokio::test]
async fn test_fetch_missing_secret_error() {
    let mock = MockServer::start()
        .await
        .expect("Failed to start mock server");
    let url = mock.url("/headers");

    let result = tokio::task::spawn_blocking(move || {
        let client = HttpClient::new_for_tests().expect("Failed to create client");
        let mut headers = HashMap::new();
        headers.insert(
            "Authorization".to_string(),
            "{{secret:nonexistent_key}}".to_string(),
        );

        client.fetch(
            url,
            FetchOptions {
                method: "GET".to_string(),
                headers: Some(headers),
                body: None,
                timeout_ms: None,
                ..Default::default()
            },
            None,
            None,
        )
    })
    .await
    .expect("Task panicked");

    assert!(result.is_err(), "Missing secret should cause error");
    let error = result.unwrap_err();
    assert!(
        error.to_string().contains("Secret not found"),
        "Expected 'Secret not found' error, got: {}",
        error
    );

    mock.shutdown().await;
}

// ============================================================================
// Manual Redirect Loop Tests (production fetch path)
// ============================================================================

#[tokio::test]
async fn test_manual_redirects_are_followed() {
    let mock = MockServer::start()
        .await
        .expect("Failed to start mock server");
    let url = mock.url("/redirect/3");

    let result = tokio::task::spawn_blocking(move || {
        let client = HttpClient::new_for_redirect_tests().expect("Failed to create client");
        client.fetch(url, FetchOptions::default(), None, None)
    })
    .await
    .expect("Task panicked");

    let response = result.expect("Redirects within the limit should be followed");
    assert_eq!(response.status, 200, "should land on /get after 3 hops");
    assert!(response.ok);

    mock.shutdown().await;
}

#[tokio::test]
async fn test_manual_redirects_reject_redirect_loop() {
    let mock = MockServer::start()
        .await
        .expect("Failed to start mock server");
    let url = mock.url("/redirect-loop");

    let result = tokio::task::spawn_blocking(move || {
        let client = HttpClient::new_for_redirect_tests().expect("Failed to create client");
        client.fetch(url, FetchOptions::default(), None, None)
    })
    .await
    .expect("Task panicked");

    let error = result.expect_err("A redirect loop must be rejected");
    assert!(
        error.to_string().contains("Too many redirects"),
        "unexpected error: {}",
        error
    );

    mock.shutdown().await;
}

#[tokio::test]
async fn test_manual_redirect_switches_post_to_get() {
    let mock = MockServer::start()
        .await
        .expect("Failed to start mock server");
    // 302 from a POST must be retried as GET (browser/fetch semantics);
    // /redirect/1 redirects to /get, which only accepts GET
    let url = mock.url("/redirect/1");

    let result = tokio::task::spawn_blocking(move || {
        let client = HttpClient::new_for_redirect_tests().expect("Failed to create client");
        client.fetch(
            url,
            FetchOptions {
                method: "POST".to_string(),
                headers: None,
                body: Some("payload".to_string()),
                timeout_ms: None,
                ..Default::default()
            },
            None,
            None,
        )
    })
    .await
    .expect("Task panicked");

    let response = result.expect("POST through a 302 should succeed as GET");
    assert_eq!(response.status, 200);

    mock.shutdown().await;
}

/// Bytes that are not text, which `fetch` could not return at all.
///
/// A response body is a `String`, and a body that is not UTF-8 was an error
/// rather than a lossy decode — the right call for the JSON and HTML nearly
/// every request fetches, and the reason a photo somebody sends a bot was
/// unreachable: the bytes arrived, and there was no way to ask for them.
///
/// `{ binary: true }` is an option rather than a second field on every
/// response, because base64 is a third again the size and every other caller
/// would have paid for it; and rather than a silent fallback, because a binary
/// body answering as an empty string reads exactly like a server that sent
/// nothing.
#[tokio::test]
async fn a_binary_body_comes_back_as_base64_when_asked_for() {
    let mock = MockServer::start()
        .await
        .expect("Failed to start mock server");
    let url = mock.url("/binary");

    let response = tokio::task::spawn_blocking(move || {
        let client = HttpClient::new_for_tests().expect("Failed to create client");
        client.fetch(
            url,
            FetchOptions {
                binary: true,
                ..Default::default()
            },
            None,
            None,
        )
    })
    .await
    .expect("Task panicked")
    .expect("a binary body should be retrievable");

    assert_eq!(response.status, 200);
    assert!(
        response.body.is_empty(),
        "exactly one of the two carries the answer: {}",
        response.body
    );

    use base64::Engine as _;
    let encoded = response
        .body_base64
        .expect("binary was asked for, so the bytes should be here");
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(&encoded)
        .expect("the client should answer with valid base64");
    assert_eq!(
        decoded,
        vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0xff, 0xfe],
        "the bytes should survive exactly"
    );

    mock.shutdown().await;
}

/// Without the option, the same body is still refused — and the refusal says
/// how to get it.
///
/// The behaviour is deliberately unchanged: a script that believed it was
/// fetching JSON and got something else should hear about it rather than
/// receive an empty string. What is new is that the message names the way out,
/// because this failure is recoverable and a caller cannot guess how.
#[tokio::test]
async fn a_binary_body_without_the_option_says_how_to_ask_for_it() {
    let mock = MockServer::start()
        .await
        .expect("Failed to start mock server");
    let url = mock.url("/binary");

    let failure = tokio::task::spawn_blocking(move || {
        let client = HttpClient::new_for_tests().expect("Failed to create client");
        client.fetch(url, FetchOptions::default(), None, None)
    })
    .await
    .expect("Task panicked")
    .expect_err("a non-UTF-8 body should still be refused by default");

    let message = failure.to_string();
    assert!(
        message.contains("binary"),
        "the refusal should name the option that fixes it: {}",
        message
    );
}

/// The timeout option the type declarations have always documented.
///
/// They said `timeout` and serde read `timeout_ms`, so every script that
/// followed the documentation got the default and no sign its own value had
/// been dropped. The alias makes the documented name work; the real name goes
/// on working, which is why this asserts both.
#[test]
fn the_documented_timeout_name_is_accepted() {
    let documented: FetchOptions =
        serde_json::from_str(r#"{"timeout": 1234}"#).expect("options should parse");
    assert_eq!(documented.timeout_ms, Some(1234));

    let real: FetchOptions =
        serde_json::from_str(r#"{"timeout_ms": 1234}"#).expect("options should parse");
    assert_eq!(real.timeout_ms, Some(1234));
}
