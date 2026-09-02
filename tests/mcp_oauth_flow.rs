//! The flow an MCP client actually walks, end to end.
//!
//! Every step of it had unit coverage, and the flow still did not work: the
//! protected-resource document named the engine's origin, the client asked for
//! a token for the origin, and `/mcp` refused that token because an audience is
//! matched on host *and* path. Nothing was wrong in isolation — the two ends
//! disagreed about what the resource was called, and only walking the whole
//! thing shows that.
//!
//! So these tests are deliberately literal about being a client: they read the
//! resource identifier out of the discovery document rather than knowing it,
//! and then insist the token that comes back opens the endpoint the document
//! was describing.

mod common;

use aiwebengine::auth::pkce::PkcePair;
use common::{TestServer, should_skip_integration_tests, wait_for_server};
use serde_json::Value;

/// A redirect URI the tests register and never listen on: the code comes back
/// in the response body, so nothing has to receive it.
const REDIRECT_URI: &str = "http://localhost:59999/callback";

struct Client {
    http: reqwest::Client,
    base: String,
}

impl Client {
    fn new(port: u16) -> anyhow::Result<Self> {
        Ok(Self {
            http: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(std::time::Duration::from_secs(10))
                .build()?,
            base: format!("http://127.0.0.1:{}", port),
        })
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }
}

/// The resource identifier this engine publishes for its MCP endpoint, read
/// the way RFC 9728 §3.1 has a client read it: from the document whose URL is
/// built out of the MCP server's own URL.
async fn discover_resource(client: &Client) -> anyhow::Result<String> {
    let document: Value = client
        .http
        .get(client.url("/.well-known/oauth-protected-resource/mcp"))
        .send()
        .await?
        .json()
        .await?;

    Ok(document["resource"]
        .as_str()
        .unwrap_or_default()
        .to_string())
}

/// Registration is bounded per address, and the bucket lives in the shared
/// `rate_limits` table — so a fixed key carries its drained state from one run
/// into the next. Every test here registers from the loopback address, so each
/// starts by handing itself a full budget.
async fn clear_registration_limit() {
    let Some(db) = aiwebengine::database::get_global_database() else {
        return;
    };
    let _ = sqlx::query("DELETE FROM rate_limits WHERE key LIKE 'client_registration:%'")
        .execute(db.pool())
        .await;
}

async fn register_client(client: &Client) -> anyhow::Result<String> {
    let registration: Value = client
        .http
        .post(client.url("/auth/oauth2/register"))
        .json(&serde_json::json!({
            "client_name": "flow-test",
            "redirect_uris": [REDIRECT_URI],
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        }))
        .send()
        .await?
        .json()
        .await?;

    Ok(registration["client_id"]
        .as_str()
        .unwrap_or_default()
        .to_string())
}

/// Sign someone in. A local account, because it needs no OAuth provider and
/// these tests are about what happens after a person is signed in.
async fn sign_in(client: &Client) -> anyhow::Result<String> {
    let response = client
        .http
        .post(client.url("/auth/local/register"))
        .json(&serde_json::json!({
            "username": format!("flow{}", &uuid::Uuid::new_v4().simple().to_string()[..12]),
            "password": "a-password-long-enough",
        }))
        .send()
        .await?;

    let status = response.status();
    let cookie = response
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::to_string);

    match cookie {
        Some(cookie) => Ok(cookie),
        None => Err(anyhow::anyhow!(
            "registration issued no session cookie: {} {}",
            status,
            response.text().await.unwrap_or_default()
        )),
    }
}

/// The one value the consent form carries that a test cannot make up: it is
/// bound to the signed-in user, and the endpoint checks that binding.
fn csrf_token_from(page: &str) -> Option<String> {
    let marker = r#"name="csrf_token" value=""#;
    let start = page.find(marker)? + marker.len();
    let rest = &page[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn authorization_code_from(page: &str) -> Option<String> {
    let start = page.find("code_")?;
    let rest = &page[start..];
    let end = rest
        .find(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-')
        .unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

/// The parameters of an authorization request, spelled out the way a client
/// writes them into a URL and then again into the consent form.
fn request_params<'a>(
    client_id: &'a str,
    challenge: &'a str,
    resource: Option<&'a str>,
) -> Vec<(&'static str, &'a str)> {
    let mut params = vec![
        ("response_type", "code"),
        ("client_id", client_id),
        ("redirect_uri", REDIRECT_URI),
        ("state", "opaque-state"),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
    ];

    if let Some(resource) = resource {
        params.push(("resource", resource));
    }

    params
}

/// Walk register → sign in → authorize → consent → token, and hand back what a
/// client would be holding at the end of it.
async fn token_for(client: &Client, resource: Option<&str>) -> anyhow::Result<String> {
    clear_registration_limit().await;
    let client_id = register_client(client).await?;
    let session_cookie = sign_in(client).await?;
    let pkce = PkcePair::generate();
    let params = request_params(&client_id, &pkce.code_challenge, resource);

    let query = params
        .iter()
        .map(|(key, value)| format!("{}={}", key, urlencoding::encode(value)))
        .collect::<Vec<_>>()
        .join("&");

    let authorize = client
        .http
        .get(format!(
            "{}?{}",
            client.url("/auth/oauth2/authorize"),
            query
        ))
        .header(reqwest::header::COOKIE, &session_cookie)
        .send()
        .await?;

    anyhow::ensure!(
        authorize.status() == reqwest::StatusCode::OK,
        "a signed-in person should be shown the consent page, got {}",
        authorize.status()
    );

    let consent_page = authorize.text().await?;
    let csrf_token = csrf_token_from(&consent_page)
        .ok_or_else(|| anyhow::anyhow!("consent page carried no CSRF token"))?;

    let mut form = vec![("csrf_token", csrf_token.as_str()), ("decision", "allow")];
    form.extend(params.iter().copied());

    let approved = client
        .http
        .post(client.url("/auth/oauth2/consent"))
        .header(reqwest::header::COOKIE, &session_cookie)
        .form(&form)
        .send()
        .await?
        .text()
        .await?;

    let code = authorization_code_from(&approved)
        .ok_or_else(|| anyhow::anyhow!("approving consent returned no code: {}", approved))?;

    let token: Value = client
        .http
        .post(client.url("/auth/oauth2/token"))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", REDIRECT_URI),
            ("client_id", client_id.as_str()),
            ("code_verifier", pkce.code_verifier.as_str()),
        ])
        .send()
        .await?
        .json()
        .await?;

    token["access_token"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("token endpoint returned no access token: {}", token))
}

/// Present the token the way an MCP client does, at the endpoint it was minted
/// for.
async fn call_mcp(client: &Client, access_token: &str) -> anyhow::Result<reqwest::Response> {
    Ok(client
        .http
        .post(client.url("/mcp"))
        .bearer_auth(access_token)
        .header(
            reqwest::header::ACCEPT,
            "application/json, text/event-stream",
        )
        .json(&serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "flow-test", "version": "0"},
            },
        }))
        .send()
        .await?)
}

/// A client discovers this engine, is authorized by a signed-in person, and
/// uses the token it gets at the endpoint the document described.
#[tokio::test(flavor = "multi_thread")]
async fn a_token_minted_through_discovery_opens_the_mcp_endpoint() -> anyhow::Result<()> {
    if should_skip_integration_tests() {
        return Ok(());
    }

    let server = TestServer::start_with_auth().await?;
    let client = Client::new(server.port())?;
    wait_for_server(server.port(), 30).await?;

    // The bug was here: the document said `http://host`, and a token for
    // `http://host` is a token for nothing, because `/mcp` is matched on path
    // as well as host.
    let resource = discover_resource(&client).await?;
    assert_eq!(
        resource,
        client.url("/mcp"),
        "the document has to name the endpoint it was asked about"
    );

    let token = token_for(&client, Some(&resource)).await?;
    let mcp = call_mcp(&client, &token).await?;

    assert_eq!(
        mcp.status(),
        reqwest::StatusCode::OK,
        "the token the flow issued must open the endpoint it was issued for: {}",
        mcp.text().await.unwrap_or_default()
    );

    server.shutdown().await;
    Ok(())
}

/// What Claude Code sends: the origin with a trailing slash, rather than the
/// endpoint. Clients derive a resource indicator in more ways than one, and a
/// token minted for a whole site would otherwise authorize nothing at all.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_asks_for_the_whole_origin_still_gets_a_usable_token() -> anyhow::Result<()> {
    if should_skip_integration_tests() {
        return Ok(());
    }

    let server = TestServer::start_with_auth().await?;
    let client = Client::new(server.port())?;
    wait_for_server(server.port(), 30).await?;

    let origin = format!("{}/", client.base);
    let token = token_for(&client, Some(&origin)).await?;
    let mcp = call_mcp(&client, &token).await?;

    assert_eq!(
        mcp.status(),
        reqwest::StatusCode::OK,
        "a resource naming the origin must still reach this engine's one \
         audience-checked endpoint: {}",
        mcp.text().await.unwrap_or_default()
    );

    server.shutdown().await;
    Ok(())
}

/// A client that names no resource at all: the token endpoint names the MCP
/// endpoint on the host the exchange happened on, and that has to match too.
#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_names_no_resource_still_gets_a_usable_token() -> anyhow::Result<()> {
    if should_skip_integration_tests() {
        return Ok(());
    }

    let server = TestServer::start_with_auth().await?;
    let client = Client::new(server.port())?;
    wait_for_server(server.port(), 30).await?;

    let token = token_for(&client, None).await?;
    let mcp = call_mcp(&client, &token).await?;

    assert_eq!(mcp.status(), reqwest::StatusCode::OK);

    server.shutdown().await;
    Ok(())
}

/// Narrowing an origin to `/mcp` must not also loosen which host a token
/// reaches: a resource naming somewhere this engine does not serve is still
/// refused before any code is issued.
#[tokio::test(flavor = "multi_thread")]
async fn a_resource_naming_another_host_is_still_refused() -> anyhow::Result<()> {
    if should_skip_integration_tests() {
        return Ok(());
    }

    let server = TestServer::start_with_auth().await?;
    let client = Client::new(server.port())?;
    wait_for_server(server.port(), 30).await?;

    assert!(
        token_for(&client, Some("https://elsewhere.example.com/"))
            .await
            .is_err(),
        "a resource naming a host this engine does not serve must be refused"
    );

    server.shutdown().await;
    Ok(())
}

/// Registration is unauthenticated — a client has no credential to present
/// before it has one — so nothing but a per-address budget stops a caller
/// writing rows into `oauth_clients` in a loop. Consent bounds what a
/// registered client may *do*; it does not bound how many of them exist.
#[tokio::test(flavor = "multi_thread")]
async fn registering_clients_in_a_loop_is_cut_off() -> anyhow::Result<()> {
    if should_skip_integration_tests() {
        return Ok(());
    }

    let server = TestServer::start_with_auth().await?;
    let client = Client::new(server.port())?;
    wait_for_server(server.port(), 30).await?;
    clear_registration_limit().await;

    // The budget is ten, refilling one every ten minutes: a developer wiring up
    // an MCP client registers once, and a handful while getting it right.
    //
    // The cap is far above ten because the bucket is keyed by address and every
    // test in this file registers from the loopback one, each topping the
    // budget up as it starts. Enough attempts that a few concurrent top-ups
    // cannot keep this loop fed.
    let mut refused = None;
    for attempt in 1..=80 {
        let status = client
            .http
            .post(client.url("/auth/oauth2/register"))
            .json(&serde_json::json!({
                "client_name": format!("flood-{}", attempt),
                "redirect_uris": [REDIRECT_URI],
                "token_endpoint_auth_method": "none",
            }))
            .send()
            .await?
            .status();

        if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
            refused = Some(attempt);
            break;
        }

        assert!(
            status.is_success(),
            "registration {} failed for some other reason: {}",
            attempt,
            status
        );
    }

    let refused = refused.expect("registering in a loop must eventually be refused");
    assert!(
        refused > 1,
        "and the first registration must still go through — this endpoint is how \
         a client onboards"
    );

    // The budget is spent, so leave a full one behind for whatever runs next.
    clear_registration_limit().await;
    server.shutdown().await;
    Ok(())
}
