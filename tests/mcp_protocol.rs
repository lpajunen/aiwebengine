//! What `/mcp` answers, across the two protocol eras it serves.
//!
//! `2026-07-28` removed the `initialize` handshake and the `Mcp-Session-Id`
//! header: a modern request carries its own protocol version and the client's
//! capabilities in `_meta`, and the server answers it without reference to
//! anything that came before. A legacy client still opens with `initialize`.
//!
//! The engine is dual-era, which the specification explicitly allows, and the
//! thing worth testing is that the two do not contaminate each other — a legacy
//! client must not be told about a revision that deleted the handshake it just
//! used, and a modern client must not be served under rules it did not name.
//! Every assertion here goes over HTTP, because the era is decided in the
//! handler and the unit tests in `mcp.rs` cannot see that wiring.

mod common;

use common::{TestServer, wait_for_server};
use serde_json::{Value, json};

/// Auth is off, so `/mcp` mounts without the bearer layer (`lib.rs`). That is
/// deliberate here: these tests are about the protocol, and the credential is
/// `tests/mcp_oauth_flow.rs`'s subject.
async fn server() -> anyhow::Result<(TestServer, reqwest::Client, String)> {
    let server = TestServer::start().await?;
    wait_for_server(server.port(), 30).await?;
    let http = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()?;
    let base = format!("http://127.0.0.1:{}/mcp", server.port());
    Ok((server, http, base))
}

/// A request the way a modern client sends one: version and capabilities in
/// `_meta`, no handshake before it.
fn modern(method: &str, version: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": {
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": version,
                "io.modelcontextprotocol/clientCapabilities": {},
                "io.modelcontextprotocol/clientInfo": { "name": "era-test", "version": "0" }
            }
        }
    })
}

async fn post(http: &reqwest::Client, url: &str, body: &Value) -> anyhow::Result<Value> {
    Ok(http.post(url).json(body).send().await?.json().await?)
}

/// `server/discover` is the one method a server MUST implement, and the one a
/// client may call before it knows anything.
#[tokio::test(flavor = "multi_thread")]
async fn discover_reports_what_the_engine_speaks() -> anyhow::Result<()> {
    let (server, http, url) = server().await?;

    let answer = post(&http, &url, &modern("server/discover", "2026-07-28")).await?;
    let result = &answer["result"];

    assert_eq!(result["resultType"], "complete");
    assert_eq!(
        result["supportedVersions"],
        json!(["2026-07-28"]),
        "only a version whose rules include _meta can be named for a modern request"
    );
    assert!(result["capabilities"]["tools"].is_object());
    assert_eq!(
        result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"], "aiwebengine",
        "with no handshake, this is the only place a client learns who answered"
    );
    assert!(
        result["ttlMs"].is_number(),
        "discovery is cacheable and has to say for how long"
    );

    server.shutdown().await;
    Ok(())
}

/// The point of discovery: a client may call it without already knowing what to
/// claim. Refusing to say what we speak because the asker guessed wrong would
/// make a client probe for the answer it came to be told.
#[tokio::test(flavor = "multi_thread")]
async fn discover_answers_a_client_that_guessed_the_version_wrong() -> anyhow::Result<()> {
    let (server, http, url) = server().await?;

    let guessed = post(&http, &url, &modern("server/discover", "1900-01-01")).await?;
    assert_eq!(
        guessed["result"]["supportedVersions"],
        json!(["2026-07-28"]),
        "discovery answers regardless of the version named"
    );

    // And with no `_meta` at all, which is what a bare probe looks like.
    let bare = post(
        &http,
        &url,
        &json!({ "jsonrpc": "2.0", "id": 1, "method": "server/discover" }),
    )
    .await?;
    assert_eq!(bare["result"]["supportedVersions"], json!(["2026-07-28"]));

    server.shutdown().await;
    Ok(())
}

/// Every other method is held to the version it named, and the refusal carries
/// enough for the client to choose again in one round trip.
#[tokio::test(flavor = "multi_thread")]
async fn a_version_we_do_not_implement_is_refused_with_what_we_do() -> anyhow::Result<()> {
    let (server, http, url) = server().await?;

    let refused = post(&http, &url, &modern("tools/list", "1900-01-01")).await?;
    assert_eq!(
        refused["error"]["code"], -32022,
        "UnsupportedProtocolVersion, which is also how a dual-era client knows \
         it is not talking to a legacy server"
    );
    assert_eq!(refused["error"]["data"]["requested"], "1900-01-01");
    assert_eq!(refused["error"]["data"]["supported"], json!(["2026-07-28"]));
    assert!(
        refused.get("result").is_none(),
        "a refusal is not also an answer"
    );

    server.shutdown().await;
    Ok(())
}

/// A legacy revision cannot be named on a modern request: `2025-11-25` has no
/// per-request `_meta`, so a request in that shape is not that revision.
#[tokio::test(flavor = "multi_thread")]
async fn a_legacy_version_cannot_be_claimed_by_a_modern_request() -> anyhow::Result<()> {
    let (server, http, url) = server().await?;

    let refused = post(&http, &url, &modern("tools/list", "2025-11-25")).await?;
    assert_eq!(refused["error"]["code"], -32022);

    server.shutdown().await;
    Ok(())
}

/// A server must not rely on a capability the client never declared, so the
/// declaration is required — and "I have none" is a declaration.
#[tokio::test(flavor = "multi_thread")]
async fn a_modern_request_must_declare_its_capabilities() -> anyhow::Result<()> {
    let (server, http, url) = server().await?;

    let silent = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {
            "_meta": { "io.modelcontextprotocol/protocolVersion": "2026-07-28" }
        }
    });
    let refused = post(&http, &url, &silent).await?;
    assert_eq!(
        refused["error"]["code"], -32602,
        "a missing required field is malformed params"
    );

    // Declaring none is not the same as not declaring.
    let declared = post(&http, &url, &modern("tools/list", "2026-07-28")).await?;
    assert!(
        declared["result"]["tools"].is_array(),
        "an empty capability object is a complete declaration: {declared}"
    );

    server.shutdown().await;
    Ok(())
}

/// A list result carries the freshness hint that replaced the `listChanged`
/// promise the engine could not keep.
#[tokio::test(flavor = "multi_thread")]
async fn a_list_says_how_long_it_stays_good_and_who_may_cache_it() -> anyhow::Result<()> {
    let (server, http, url) = server().await?;

    for method in ["tools/list", "prompts/list"] {
        let answer = post(&http, &url, &modern(method, "2026-07-28")).await?;
        let result = &answer["result"];
        assert_eq!(result["resultType"], "complete", "{method}");
        assert!(result["ttlMs"].is_number(), "{method} carried no ttlMs");
        assert_eq!(
            result["cacheScope"], "private",
            "{method} is filtered by host and by whether engine tools are \
             allowed there, so a shared cache would hand one caller another's list"
        );
    }

    server.shutdown().await;
    Ok(())
}

/// A gateway that routed on `Mcp-Method` and a body saying something else
/// cannot both be right, and guessing is how a request gets metered as one
/// thing and executed as another.
#[tokio::test(flavor = "multi_thread")]
async fn a_routing_header_that_disagrees_with_the_body_is_refused() -> anyhow::Result<()> {
    let (server, http, url) = server().await?;

    let mismatched: Value = http
        .post(&url)
        .header("Mcp-Method", "tools/call")
        .json(&modern("tools/list", "2026-07-28"))
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(mismatched["error"]["code"], -32020, "HeaderMismatch");

    // Agreeing is ordinary.
    let agreed: Value = http
        .post(&url)
        .header("Mcp-Method", "tools/list")
        .json(&modern("tools/list", "2026-07-28"))
        .send()
        .await?
        .json()
        .await?;
    assert!(agreed["result"]["tools"].is_array());

    server.shutdown().await;
    Ok(())
}

/// The legacy half still works, and is not told about a revision that deleted
/// the handshake it just used.
#[tokio::test(flavor = "multi_thread")]
async fn a_legacy_client_is_answered_in_its_own_era() -> anyhow::Result<()> {
    let (server, http, url) = server().await?;

    let handshake = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": { "name": "legacy-test", "version": "0" }
        }
    });
    let answer = post(&http, &url, &handshake).await?;
    assert_eq!(
        answer["result"]["protocolVersion"], "2025-06-18",
        "a version we speak is answered with itself"
    );

    // A legacy client naming something we do not speak gets the newest legacy
    // revision — never a modern one, which has no `initialize` to have arrived
    // through.
    let future = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": { "protocolVersion": "2099-01-01", "capabilities": {} }
    });
    let answered = post(&http, &url, &future).await?;
    assert_eq!(answered["result"]["protocolVersion"], "2025-11-25");

    // And a call inside that session, carrying no `_meta`, is still served.
    let listed = post(
        &http,
        &url,
        &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    )
    .await?;
    assert!(
        listed["result"]["tools"].is_array(),
        "a legacy call must not be judged by modern rules: {listed}"
    );

    server.shutdown().await;
    Ok(())
}

/// Listing is stable across calls, which the caching hint above depends on: a
/// client comparing a cached list against the next one must not see a change
/// that is only iteration order.
#[tokio::test(flavor = "multi_thread")]
async fn a_tool_listing_comes_back_in_the_same_order() -> anyhow::Result<()> {
    let (server, http, url) = server().await?;

    let first = post(&http, &url, &modern("tools/list", "2026-07-28")).await?;
    let again = post(&http, &url, &modern("tools/list", "2026-07-28")).await?;
    assert_eq!(
        first["result"]["tools"], again["result"]["tools"],
        "the same listing twice must be the same listing"
    );

    let names: Vec<&str> = first["result"]["tools"]
        .as_array()
        .map(|tools| {
            tools
                .iter()
                .filter_map(|tool| tool["name"].as_str())
                .collect()
        })
        .unwrap_or_default();
    let mut sorted = names.clone();
    sorted.sort_unstable();
    assert_eq!(names, sorted, "and it is ordered rather than merely stable");

    server.shutdown().await;
    Ok(())
}
