//! The consent page, walked the way a browser walks it.
//!
//! `tests/delegation.rs` covers the store and the JavaScript surface. What
//! is left over is the seam between them: a script mints an invitation, a
//! person opens it, the page shows them which sender they are about to
//! link, and pressing the button records the grant and the link together.
//!
//! That seam is worth its own file because every part of it is a place a
//! mistake is invisible. A page that dropped the hidden field would record
//! the grant and silently no link; one that spent the token on display would
//! break the sign-in redirect; one that did not compare the token's script
//! against the form's would let an invitation minted by a chatty solution be
//! redeemed against a different one.

mod common;

use common::AdminServer;

const PASSWORD: &str = "a-perfectly-fine-password";

fn unique(label: &str) -> String {
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    format!("{}{}", label, &suffix[..12])
}

async fn register(
    engine: &AdminServer,
    http: &reqwest::Client,
) -> anyhow::Result<(String, String)> {
    let username = unique("consent");
    let response = http
        .post(engine.url("/auth/local/register"))
        .json(&serde_json::json!({ "username": username, "password": PASSWORD }))
        .send()
        .await?;

    anyhow::ensure!(
        response.status().is_success(),
        "registering {} failed: {}",
        username,
        response.status()
    );

    let cookie = response
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("registration issued no session"))?;

    Ok((username, cookie))
}

fn csrf_token_from(html: &str) -> String {
    let marker = r#"name="csrf_token" value=""#;
    let start = html.find(marker).expect("the page should carry a token") + marker.len();
    let rest = &html[start..];
    let end = rest.find('"').expect("the token should be quoted");
    rest[..end].to_string()
}

fn hidden_link_from(html: &str) -> Option<String> {
    let marker = r#"name="link" value=""#;
    let start = html.find(marker)? + marker.len();
    let rest = &html[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn token_of(url: &str) -> &str {
    url.strip_prefix("/auth/delegate?link=")
        .expect("an invitation URL carries its token")
}

/// The whole round trip: mint, open, consent, and the sender is linked to
/// the person who opened it.
#[tokio::test(flavor = "multi_thread")]
async fn opening_an_invitation_and_consenting_links_the_sender() {
    let engine = AdminServer::start().await.expect("server failed to start");
    let http = engine.anonymous().clone();
    let (_, cookie) = register(&engine, &http).await.expect("registration");

    let script = "test://consent/telegram-bot";
    let invitation = aiwebengine::delegation::invite_link(script, "telegram", "12345")
        .await
        .expect("a script mints this in reply to a message");

    let page = http
        .get(engine.url(&invitation))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .expect("the consent page should answer");
    assert_eq!(page.status(), 200);
    let html = page.text().await.expect("a body");

    // The page names the sender, so the person knows what they are agreeing
    // to rather than approving an opaque token.
    assert!(
        html.contains("12345") && html.contains("telegram"),
        "the page should say which sender it is about"
    );

    let token = csrf_token_from(&html);
    let carried = hidden_link_from(&html).expect("the form should carry the invitation");

    let response = http
        .post(engine.url("/auth/delegate"))
        .header(reqwest::header::COOKIE, &cookie)
        .form(&[
            ("csrf_token", token.as_str()),
            ("script", script),
            ("scope", "personal_storage"),
            ("scope", "write"),
            ("link", carried.as_str()),
            ("days", "30"),
        ])
        .send()
        .await
        .expect("the form should be accepted");
    assert!(response.status().is_redirection());

    // And the sender now reaches exactly that person.
    let linked = aiwebengine::delegation::resolve_channel(script, "telegram", "12345")
        .await
        .expect("lookup");
    assert!(linked.is_some(), "the sender should be linked");

    // Both halves landed: the link is useless without the grant.
    let grant = aiwebengine::delegation::get(linked.as_deref().unwrap(), script)
        .await
        .expect("lookup");
    assert!(grant.is_some(), "the grant should have been recorded too");

    // Spent. A link forwarded on after somebody used it links nothing.
    assert_eq!(
        aiwebengine::delegation::peek_invite(token_of(&invitation)).await,
        None,
    );

    engine.shutdown().await;
}

/// Reading the page does not spend the invitation, which is what lets a
/// signed-out person be sent to sign in and come back to it.
#[tokio::test(flavor = "multi_thread")]
async fn a_signed_out_visitor_keeps_the_invitation_through_signing_in() {
    let engine = AdminServer::start().await.expect("server failed to start");
    let http = engine.anonymous().clone();

    let script = "test://consent/signed-out";
    let invitation = aiwebengine::delegation::invite_link(script, "telegram", "12345")
        .await
        .expect("minted");

    let response = http
        .get(engine.url(&invitation))
        .send()
        .await
        .expect("the page should answer");

    assert!(response.status().is_redirection());
    let location = response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    assert!(
        location.starts_with("/auth/login?redirect=") && location.contains("link%3Dlnk_"),
        "signing in should come back to the same invitation: {}",
        location
    );

    // And it still works afterwards, which it would not if being shown the
    // sign-in page had spent it.
    assert!(
        aiwebengine::delegation::peek_invite(token_of(&invitation))
            .await
            .is_some()
    );

    engine.shutdown().await;
}

/// An invitation minted by one script cannot be redeemed against another.
/// The token names its own script precisely so that editing the form's
/// hidden `script` field changes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn an_invitation_cannot_be_redeemed_against_another_script() {
    let engine = AdminServer::start().await.expect("server failed to start");
    let http = engine.anonymous().clone();
    let (_, cookie) = register(&engine, &http).await.expect("registration");

    let minted_by = "test://consent/mine";
    let target = "test://consent/theirs";
    let invitation = aiwebengine::delegation::invite_link(minted_by, "telegram", "12345")
        .await
        .expect("minted");

    // The page for the *other* script shows no sender at all, because the
    // token does not name it.
    let html = http
        .get(engine.url(&format!(
            "/auth/delegate?script={}&link={}",
            urlencoding::encode(target),
            urlencoding::encode(token_of(&invitation)),
        )))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .expect("the page should answer")
        .text()
        .await
        .expect("a body");
    assert!(
        hidden_link_from(&html).is_none(),
        "an invitation for another script must not be offered here"
    );

    // And posting it by hand records the grant and no link.
    let response = http
        .post(engine.url("/auth/delegate"))
        .header(reqwest::header::COOKIE, &cookie)
        .form(&[
            ("csrf_token", csrf_token_from(&html).as_str()),
            ("script", target),
            ("scope", "personal_storage"),
            ("link", token_of(&invitation)),
            ("days", "30"),
        ])
        .send()
        .await
        .expect("the form should be accepted");
    assert!(response.status().is_redirection());

    assert_eq!(
        aiwebengine::delegation::resolve_channel(target, "telegram", "12345")
            .await
            .expect("lookup"),
        None,
        "the sender must not have been linked to the script it was not minted for"
    );

    engine.shutdown().await;
}

/// Without an invitation the page offers no way to name a sender at all —
/// which is the property that makes linking somebody else's chat impossible
/// rather than merely discouraged.
#[tokio::test(flavor = "multi_thread")]
async fn the_page_offers_no_way_to_name_a_sender_by_hand() {
    let engine = AdminServer::start().await.expect("server failed to start");
    let http = engine.anonymous().clone();
    let (_, cookie) = register(&engine, &http).await.expect("registration");

    let script = "test://consent/no-invitation";

    // The shape somebody would try: the sender straight in the query string.
    let html = http
        .get(engine.url(&format!(
            "/auth/delegate?script={}&channel=telegram&identity=12345",
            urlencoding::encode(script),
        )))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .expect("the page should answer")
        .text()
        .await
        .expect("a body");

    assert!(
        hidden_link_from(&html).is_none(),
        "there should be nothing to link"
    );
    assert!(
        !html.contains("12345"),
        "the page must not echo a sender nobody vouched for: it would read as \
         though consenting would link it"
    );

    // Posting the pair directly does nothing either: the endpoint reads an
    // invitation and not a sender.
    let response = http
        .post(engine.url("/auth/delegate"))
        .header(reqwest::header::COOKIE, &cookie)
        .form(&[
            ("csrf_token", csrf_token_from(&html).as_str()),
            ("script", script),
            ("scope", "personal_storage"),
            ("channel", "telegram"),
            ("identity", "12345"),
            ("days", "30"),
        ])
        .send()
        .await
        .expect("the form should be accepted");
    assert!(response.status().is_redirection());

    assert_eq!(
        aiwebengine::delegation::resolve_channel(script, "telegram", "12345")
            .await
            .expect("lookup"),
        None,
        "naming a sender by hand must link nothing"
    );

    engine.shutdown().await;
}
