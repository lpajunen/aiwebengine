//! Publishing a script's assets as MCP resources.
//!
//! `mcpRegistry.registerResource` is `routeRegistry.registerAssetRoute` aimed
//! at `/mcp` instead of at a path: the same asset, published under a name a
//! different protocol reaches. So what these assert is the part that differs
//! from an asset route — that a listing describes what is registered, that a
//! read answers with what the asset says *now* rather than at `init()`, and
//! that binary content survives the trip as base64 rather than as invalid
//! JSON or a lossy decode.
//!
//! The reason a resource is asset-backed rather than handler-backed is worth
//! keeping in view while reading these: a resource whose content came from a
//! handler would be a tool with a different spelling, free to answer
//! differently every time, which is not what a client caching by URI has any
//! reason to expect.

mod common;

use common::{TestServer, wait_for_server};
use serde_json::{Value, json};

use aiwebengine::repository;

const SCRIPT_URI: &str = "test://mcp/resources";

/// A script that publishes two of its assets and nothing else.
const RESOURCE_SCRIPT: &str = r#"
function init() {
  mcpRegistry.registerResource("docs://handbook", "handbook.md", {
    name: "Handbook",
    description: "How the team works",
    mimeType: "text/markdown"
  });
  mcpRegistry.registerResource("docs://logo", "logo.png");
}
"#;

fn store_asset(asset_uri: &str, mimetype: &str, content: impl Into<Vec<u8>>) {
    let now = std::time::SystemTime::now();
    repository::upsert_asset(repository::Asset {
        uri: asset_uri.to_string(),
        name: Some(asset_uri.to_string()),
        mimetype: mimetype.to_string(),
        content: content.into(),
        created_at: now,
        updated_at: now,
        script_uri: SCRIPT_URI.to_string(),
    })
    .expect("asset should be stored");
}

/// The assets have to exist before `init()` runs: registration verifies that
/// the asset is there and owned by the registering script, so that a listed
/// resource is never one a client cannot read.
async fn server() -> anyhow::Result<(TestServer, reqwest::Client, String)> {
    let server = TestServer::start().await?;
    wait_for_server(server.port(), 30).await?;

    // The script first: assets are keyed by their owning script, so there has
    // to be one to own them.
    repository::upsert_script(SCRIPT_URI, RESOURCE_SCRIPT).expect("script should store");

    store_asset("handbook.md", "text/markdown", "# Handbook\n\nBe kind.\n");
    // Deliberately not valid UTF-8: a PNG header, which is what forces the
    // `blob` arm below.
    store_asset(
        "logo.png",
        "image/png",
        vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0xff, 0xfe],
    );

    aiwebengine::script_init::ScriptInitializer::with_configured_timeout()
        .initialize_script(SCRIPT_URI, false)
        .await
        .ok();

    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()?;
    let url = format!("http://127.0.0.1:{}/mcp", server.port());
    Ok((server, http, url))
}

fn modern(method: &str, params: Value) -> Value {
    let mut params = params;
    if let Some(object) = params.as_object_mut() {
        object.insert(
            "_meta".to_string(),
            json!({
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {}
            }),
        );
    }
    json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params })
}

async fn post(http: &reqwest::Client, url: &str, body: &Value) -> anyhow::Result<Value> {
    Ok(http.post(url).json(body).send().await?.json().await?)
}

/// The listing describes what was registered, and carries the same caching
/// hint the other two list arms do — a client has no more reason to re-poll
/// resources than tools.
#[tokio::test(flavor = "multi_thread")]
async fn a_listing_describes_what_a_script_published() -> anyhow::Result<()> {
    let (server, http, url) = server().await?;

    let answer = post(&http, &url, &modern("resources/list", json!({}))).await?;
    let result = &answer["result"];

    assert_eq!(result["resultType"], "complete");
    assert!(
        result["ttlMs"].is_number(),
        "a resource listing should say how long it stays good: {}",
        answer
    );

    let resources = result["resources"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|entry| {
            entry["uri"]
                .as_str()
                .is_some_and(|uri| uri.starts_with("docs://"))
        })
        .collect::<Vec<_>>();

    assert_eq!(resources.len(), 2, "both resources should list: {}", answer);
    // Sorted by URI, so a client comparing one listing against the next does
    // not see a change that is only iteration order.
    assert_eq!(resources[0]["uri"], "docs://handbook");
    assert_eq!(resources[0]["name"], "Handbook");
    assert_eq!(resources[0]["description"], "How the team works");
    assert_eq!(resources[0]["mimeType"], "text/markdown");

    // The second registered no metadata at all, so the name falls back to the
    // asset's — the more readable of the two candidates — and `mimeType` is
    // absent rather than guessed.
    assert_eq!(resources[1]["uri"], "docs://logo");
    assert_eq!(resources[1]["name"], "logo.png");
    assert!(
        resources[1].get("mimeType").is_none(),
        "an unstated MIME type should be omitted, not invented: {}",
        answer
    );

    drop(server);
    Ok(())
}

/// Text comes back as `text`, and it is the asset's current content rather
/// than a copy taken when `init()` ran.
#[tokio::test(flavor = "multi_thread")]
async fn a_read_answers_with_what_the_asset_says_now() -> anyhow::Result<()> {
    let (server, http, url) = server().await?;

    let first = post(
        &http,
        &url,
        &modern("resources/read", json!({ "uri": "docs://handbook" })),
    )
    .await?;
    let contents = &first["result"]["contents"][0];
    assert_eq!(contents["uri"], "docs://handbook");
    assert_eq!(contents["mimeType"], "text/markdown");
    assert!(
        contents["text"]
            .as_str()
            .unwrap_or_default()
            .contains("Be kind"),
        "should answer with the asset's text: {}",
        first
    );

    // Rewrite the asset without re-running init(). A registration that had
    // copied the content would go on serving the old bytes, which is the
    // failure this asserts against — an asset written by the script itself,
    // or by an editor, reaches clients without a redeploy.
    store_asset(
        "handbook.md",
        "text/markdown",
        "# Handbook\n\nBe careful.\n",
    );

    let second = post(
        &http,
        &url,
        &modern("resources/read", json!({ "uri": "docs://handbook" })),
    )
    .await?;
    assert!(
        second["result"]["contents"][0]["text"]
            .as_str()
            .unwrap_or_default()
            .contains("Be careful"),
        "a read should not serve content captured at registration: {}",
        second
    );

    drop(server);
    Ok(())
}

/// Bytes that are not text travel as base64 in `blob`.
///
/// Decided by whether the content *is* text rather than by what the MIME type
/// claims, because a MIME type on an asset store anybody can write to is a
/// claim. Getting this wrong does not degrade gracefully: a lossy decode would
/// put `U+FFFD` through an image, and no decode at all would make the response
/// invalid JSON.
#[tokio::test(flavor = "multi_thread")]
async fn binary_content_comes_back_as_a_blob() -> anyhow::Result<()> {
    let (server, http, url) = server().await?;

    let answer = post(
        &http,
        &url,
        &modern("resources/read", json!({ "uri": "docs://logo" })),
    )
    .await?;
    let contents = &answer["result"]["contents"][0];

    assert!(
        contents.get("text").is_none(),
        "binary content must not be offered as text: {}",
        answer
    );
    let blob = contents["blob"]
        .as_str()
        .unwrap_or_else(|| panic!("binary content should answer as a blob: {}", answer));

    use base64::Engine as _;
    let decoded = base64::engine::general_purpose::STANDARD.decode(blob)?;
    assert_eq!(
        decoded,
        vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0xff, 0xfe],
        "the bytes should survive the round trip exactly"
    );
    // The asset's own type, since this registration stated none.
    assert_eq!(contents["mimeType"], "image/png");

    drop(server);
    Ok(())
}

/// A URI nobody registered is refused, and refused the same way a URI
/// published on another host would be.
#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_uri_is_refused() -> anyhow::Result<()> {
    let (server, http, url) = server().await?;

    let answer = post(
        &http,
        &url,
        &modern("resources/read", json!({ "uri": "docs://nothing-here" })),
    )
    .await?;

    assert_eq!(
        answer["error"]["code"], -32602,
        "an unknown resource is invalid params, per the code the revision \
         moved this to: {}",
        answer
    );
    assert!(answer["result"].is_null());

    drop(server);
    Ok(())
}

/// The capability is advertised, so a client knows to ask at all.
#[tokio::test(flavor = "multi_thread")]
async fn the_server_says_it_serves_resources() -> anyhow::Result<()> {
    let (server, http, url) = server().await?;

    let discovered = post(&http, &url, &modern("server/discover", json!({}))).await?;
    assert!(
        discovered["result"]["capabilities"]
            .get("resources")
            .is_some(),
        "server/discover should advertise resources: {}",
        discovered
    );

    // And the legacy handshake says so too, with the two flags it has to carry
    // there — both false, because a POST response has nothing to push on.
    let initialized = post(
        &http,
        &url,
        &json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2025-11-25" }
        }),
    )
    .await?;
    let resources = &initialized["result"]["capabilities"]["resources"];
    assert_eq!(resources["subscribe"], false, "got: {}", initialized);
    assert_eq!(resources["listChanged"], false, "got: {}", initialized);

    drop(server);
    Ok(())
}
