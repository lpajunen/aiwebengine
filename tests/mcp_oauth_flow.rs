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
async fn sign_in(client: &Client) -> anyhow::Result<(String, String)> {
    let username = format!("flow{}", &uuid::Uuid::new_v4().simple().to_string()[..12]);
    let response = client
        .http
        .post(client.url("/auth/local/register"))
        .json(&serde_json::json!({
            "username": username,
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
        Some(cookie) => Ok((username, cookie)),
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
    let token = token_response_for(client, resource).await?.token;

    token["access_token"]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("token endpoint returned no access token: {}", token))
}

/// What one walk of the flow leaves a test holding.
struct Flow {
    client_id: String,
    /// The local account that approved the client. Named so a test can revoke
    /// exactly the identity this flow signed in as, rather than guessing at the
    /// shared database.
    username: String,
    token: Value,
}

/// The same walk, handing back everything a test might want to look at rather
/// than one field of the token response.
async fn token_response_for(client: &Client, resource: Option<&str>) -> anyhow::Result<Flow> {
    clear_registration_limit().await;
    let client_id = register_client(client).await?;
    let (username, session_cookie) = sign_in(client).await?;
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

    Ok(Flow {
        client_id,
        username,
        token,
    })
}

/// Redeem a refresh token the way a client does when its access token ages out.
async fn refresh_with(
    client: &Client,
    client_id: &str,
    refresh_token: &str,
) -> anyhow::Result<(reqwest::StatusCode, Value)> {
    let response = client
        .http
        .post(client.url("/auth/oauth2/token"))
        .form(&[
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", client_id),
        ])
        .send()
        .await?;

    let status = response.status();
    let body: Value = response.json().await.unwrap_or(Value::Null);
    Ok((status, body))
}

fn field(token: &Value, name: &str) -> anyhow::Result<String> {
    token[name]
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("token response carried no {}: {}", name, token))
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

// ============================================================================
// Refresh tokens
// ============================================================================

/// The finding this closes: the token endpoint returned the session token in
/// both fields, so the "refresh token" was the access token. Rotation was
/// impossible, and a leaked refresh token was a leaked access token carrying
/// the same audience and the same roles.
#[tokio::test(flavor = "multi_thread")]
async fn a_refresh_token_is_not_an_access_token() -> anyhow::Result<()> {
    if should_skip_integration_tests() {
        return Ok(());
    }

    let server = TestServer::start_with_auth().await?;
    let client = Client::new(server.port())?;
    wait_for_server(server.port(), 30).await?;

    let flow = token_response_for(&client, None).await?;
    let access_token = field(&flow.token, "access_token")?;
    let refresh_token = field(&flow.token, "refresh_token")?;

    assert_ne!(
        access_token, refresh_token,
        "the two must be different credentials"
    );

    // And the difference has to mean something: a refresh token authenticates
    // nothing. It is redeemed at the token endpoint, never presented as one.
    let refused = call_mcp(&client, &refresh_token).await?;
    assert_eq!(
        refused.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "a refresh token must not open an API endpoint"
    );

    server.shutdown().await;
    Ok(())
}

/// Redeeming rotates. The client gets a new access token and a new refresh
/// token, and the one it spent stops working — which is what makes a stolen
/// copy detectable rather than permanently useful.
#[tokio::test(flavor = "multi_thread")]
async fn redeeming_a_refresh_token_rotates_it() -> anyhow::Result<()> {
    if should_skip_integration_tests() {
        return Ok(());
    }

    let server = TestServer::start_with_auth().await?;
    let client = Client::new(server.port())?;
    wait_for_server(server.port(), 30).await?;

    let resource = discover_resource(&client).await?;
    let flow = token_response_for(&client, Some(&resource)).await?;
    let first_refresh = field(&flow.token, "refresh_token")?;

    let (status, refreshed) = refresh_with(&client, &flow.client_id, &first_refresh).await?;
    assert_eq!(
        status,
        reqwest::StatusCode::OK,
        "refreshing should succeed: {}",
        refreshed
    );

    let second_access = field(&refreshed, "access_token")?;
    let second_refresh = field(&refreshed, "refresh_token")?;
    assert_ne!(
        first_refresh, second_refresh,
        "the spent token must be replaced, not handed back"
    );
    assert_ne!(
        second_access, second_refresh,
        "and the replacement must still not be the access token"
    );

    // The session it minted is a working one, for the audience the original
    // authorization named — refreshing must not narrow or widen that.
    let mcp = call_mcp(&client, &second_access).await?;
    assert_eq!(
        mcp.status(),
        reqwest::StatusCode::OK,
        "the refreshed token must open the endpoint the first one did: {}",
        mcp.text().await.unwrap_or_default()
    );

    server.shutdown().await;
    Ok(())
}

/// Presenting a spent token cannot be told apart from a replay, so the whole
/// rotation chain goes — including the successor, which is the copy the
/// legitimate client is holding. Losing a session is the right outcome when the
/// alternative is not knowing who else has one.
#[tokio::test(flavor = "multi_thread")]
async fn replaying_a_spent_refresh_token_revokes_the_chain() -> anyhow::Result<()> {
    if should_skip_integration_tests() {
        return Ok(());
    }

    let server = TestServer::start_with_auth().await?;
    let client = Client::new(server.port())?;
    wait_for_server(server.port(), 30).await?;

    let flow = token_response_for(&client, None).await?;
    let first_refresh = field(&flow.token, "refresh_token")?;

    let (_, refreshed) = refresh_with(&client, &flow.client_id, &first_refresh).await?;
    let second_refresh = field(&refreshed, "refresh_token")?;

    let (replayed, _) = refresh_with(&client, &flow.client_id, &first_refresh).await?;
    assert_eq!(
        replayed,
        reqwest::StatusCode::BAD_REQUEST,
        "a token that was already spent must not be redeemable again"
    );

    let (successor, _) = refresh_with(&client, &flow.client_id, &second_refresh).await?;
    assert_eq!(
        successor,
        reqwest::StatusCode::BAD_REQUEST,
        "and the replay must take the rest of the family with it"
    );

    server.shutdown().await;
    Ok(())
}

/// A refresh token is bound to the client it was issued to. Registration is
/// open, so anyone can hold a `client_id` — holding someone else's refresh
/// token must not be enough to redeem it.
#[tokio::test(flavor = "multi_thread")]
async fn another_client_cannot_redeem_a_refresh_token() -> anyhow::Result<()> {
    if should_skip_integration_tests() {
        return Ok(());
    }

    let server = TestServer::start_with_auth().await?;
    let client = Client::new(server.port())?;
    wait_for_server(server.port(), 30).await?;

    let flow = token_response_for(&client, None).await?;
    let refresh_token = field(&flow.token, "refresh_token")?;

    clear_registration_limit().await;
    let other_client_id = register_client(&client).await?;

    let (status, _) = refresh_with(&client, &other_client_id, &refresh_token).await?;
    assert_eq!(
        status,
        reqwest::StatusCode::BAD_REQUEST,
        "a refresh token presented by a client it was not issued to must be refused"
    );

    server.shutdown().await;
    Ok(())
}

/// Ending someone's sessions has to end what could mint another one. Otherwise
/// revoking a role revokes nothing: the client refreshes, and the role it just
/// lost comes back with the new session.
#[tokio::test(flavor = "multi_thread")]
async fn revoking_sessions_revokes_the_refresh_tokens_too() -> anyhow::Result<()> {
    if should_skip_integration_tests() {
        return Ok(());
    }

    let server = TestServer::start_with_auth().await?;
    let client = Client::new(server.port())?;
    wait_for_server(server.port(), 30).await?;

    let flow = token_response_for(&client, None).await?;
    let access_token = field(&flow.token, "access_token")?;
    let refresh_token = field(&flow.token, "refresh_token")?;

    // Exactly the account this flow signed in as. The database is shared with
    // whatever else the suite is running, so "the newest session" would be a
    // race rather than an identity.
    let db =
        aiwebengine::database::get_global_database().expect("the suite's database should be up");
    let user = aiwebengine::user_repository::find_user_by_provider(
        aiwebengine::auth::local::LOCAL_PROVIDER,
        &aiwebengine::auth::local::normalize_username(&flow.username),
    )?
    .expect("the account the flow registered should exist");

    aiwebengine::security::delete_sessions_for_user(db.pool(), &user.id).await?;

    let refused = call_mcp(&client, &access_token).await?;
    assert_eq!(
        refused.status(),
        reqwest::StatusCode::UNAUTHORIZED,
        "the session is gone"
    );

    let (status, _) = refresh_with(&client, &flow.client_id, &refresh_token).await?;
    assert_eq!(
        status,
        reqwest::StatusCode::BAD_REQUEST,
        "and the refresh token must not be a way back in"
    );

    server.shutdown().await;
    Ok(())
}
