//! The engine's secret page, walked the way a browser walks it.
//!
//! A script declares in `init()` what it asks people for and links them to
//! `secretStorage.entryUrl(name)`; the person stores, replaces or removes the
//! value there, and it reaches the script only as a stored secret. What is
//! tested here is the seam: the page asks only for a declared name, never
//! shows a stored value back, takes only a form post carrying the person's
//! own token, and returns only to a path on this engine.

mod common;

use common::AdminServer;

const PASSWORD: &str = "a-perfectly-fine-password";
const VALUE: &str = "sk-test-value-that-must-not-appear";

fn unique(label: &str) -> String {
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    format!("{}{}", label, &suffix[..12])
}

/// A signed-in person: their session cookie.
async fn register(engine: &AdminServer, http: &reqwest::Client) -> String {
    let response = http
        .post(engine.url("/auth/local/register"))
        .json(&serde_json::json!({ "username": unique("secret"), "password": PASSWORD }))
        .send()
        .await
        .expect("registering should answer");
    assert!(response.status().is_success(), "registering failed");
    response
        .headers()
        .get(reqwest::header::SET_COOKIE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(str::to_string)
        .expect("registration should issue a session")
}

fn csrf_token_from(html: &str) -> String {
    let marker = r#"name="csrf_token" value=""#;
    let start = html.find(marker).expect("the page should carry a token") + marker.len();
    let rest = &html[start..];
    rest[..rest.find('"').expect("quoted")].to_string()
}

fn location(response: &reqwest::Response) -> String {
    response
        .headers()
        .get(reqwest::header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

/// A script that declares `API_KEY` and answers, at `path`, the URL of its
/// page, whether the caller has one stored, and what declaring outside
/// `init()` does.
async fn deploy(engine: &AdminServer, script: &str, path: &str) {
    engine
        .deploy_script(
            script,
            &format!(
                r#"
                function about(context) {{
                    return ResponseBuilder.json({{
                        url: secretStorage.entryUrl("API_KEY"),
                        has: secretStorage.exists("API_KEY"),
                        late: secretStorage.declare("OTHER", {{ label: "Other" }}),
                    }});
                }}
                function init() {{
                    secretStorage.declare("API_KEY", {{ label: "Example API key" }});
                    routeRegistry.registerRoute("{path}", {{ handler: "about" }});
                }}
                "#
            ),
        )
        .await;
}

async fn about(
    engine: &AdminServer,
    http: &reqwest::Client,
    path: &str,
    cookie: &str,
) -> serde_json::Value {
    http.get(engine.url(path))
        .header(reqwest::header::COOKIE, cookie)
        .send()
        .await
        .expect("the script should answer")
        .json()
        .await
        .expect("as JSON")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_person_stores_replaces_and_removes_a_declared_secret() {
    let engine = AdminServer::start().await.expect("server failed to start");
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("client");
    let cookie = register(&engine, &http).await;
    let script = format!("test://secret-page/{}", unique("app"));
    let path = format!("/{}", unique("secret-page"));
    deploy(&engine, &script, &path).await;

    let first = about(&engine, &http, &path, &cookie).await;
    let url = first["url"]
        .as_str()
        .expect("entryUrl answers a path")
        .to_string();
    assert!(url.starts_with("/auth/secret?script="));
    assert_eq!(first["has"], false);
    // Outside init() a declaration changes nothing, and says so.
    assert_eq!(first["late"]["ok"], false);

    let page = http
        .get(engine.url(&url))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .expect("the page should answer");
    assert_eq!(page.status(), 200);
    let html = page.text().await.expect("a body");
    assert!(html.contains("Example API key"), "the script's own label");
    assert!(html.contains("None is stored yet."));
    let token = csrf_token_from(&html);

    let stored = http
        .post(engine.url("/auth/secret"))
        .header(reqwest::header::COOKIE, &cookie)
        .form(&[
            ("csrf_token", token.as_str()),
            ("script", script.as_str()),
            ("name", "API_KEY"),
            ("value", VALUE),
        ])
        .send()
        .await
        .expect("the form should be accepted");
    assert!(stored.status().is_redirection());
    assert!(location(&stored).ends_with("&notice=saved"));
    assert_eq!(about(&engine, &http, &path, &cookie).await["has"], true);

    // Stored, and never shown back.
    let html = http
        .get(engine.url(&location(&stored)))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .expect("the page should answer")
        .text()
        .await
        .expect("a body");
    assert!(html.contains("You have one stored."));
    assert!(html.contains("Stored."));
    assert!(!html.contains(VALUE), "the value must never reach the page");

    let removed = http
        .post(engine.url("/auth/secret"))
        .header(reqwest::header::COOKIE, &cookie)
        .form(&[
            ("csrf_token", csrf_token_from(&html).as_str()),
            ("script", script.as_str()),
            ("name", "API_KEY"),
            ("remove", "1"),
            ("redirect", "/somewhere?x=1"),
        ])
        .send()
        .await
        .expect("the form should be accepted");
    assert_eq!(location(&removed), "/somewhere?x=1");
    assert_eq!(about(&engine, &http, &path, &cookie).await["has"], false);

    engine.shutdown().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_page_refuses_what_it_was_not_asked_for() {
    let engine = AdminServer::start().await.expect("server failed to start");
    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("client");
    let cookie = register(&engine, &http).await;
    let script = format!("test://secret-page/{}", unique("app"));
    let path = format!("/{}", unique("secret-page"));
    deploy(&engine, &script, &path).await;
    let url = about(&engine, &http, &path, &cookie).await["url"]
        .as_str()
        .expect("a path")
        .to_string();

    // A name the script did not declare, and a script that declared nothing.
    for undeclared in [
        aiwebengine::declared_secrets::entry_url(&script, "OTHER"),
        aiwebengine::declared_secrets::entry_url("test://secret-page/nobody", "API_KEY"),
    ] {
        let page = http
            .get(engine.url(&undeclared))
            .header(reqwest::header::COOKIE, &cookie)
            .send()
            .await
            .expect("the page should answer");
        assert_eq!(page.status(), 404, "{undeclared}");
    }

    let html = http
        .get(engine.url(&url))
        .header(reqwest::header::COOKIE, &cookie)
        .send()
        .await
        .expect("the page should answer")
        .text()
        .await
        .expect("a body");
    let token = csrf_token_from(&html);

    // A post for an undeclared name stores nothing.
    let other = http
        .post(engine.url("/auth/secret"))
        .header(reqwest::header::COOKIE, &cookie)
        .form(&[
            ("csrf_token", token.as_str()),
            ("script", script.as_str()),
            ("name", "OTHER"),
            ("value", VALUE),
        ])
        .send()
        .await
        .expect("answered");
    assert_eq!(other.status(), 404);

    // Without the person's token, or as JSON, nothing is stored.
    let forged = http
        .post(engine.url("/auth/secret"))
        .header(reqwest::header::COOKIE, &cookie)
        .form(&[
            ("csrf_token", "forged"),
            ("script", script.as_str()),
            ("name", "API_KEY"),
            ("value", VALUE),
        ])
        .send()
        .await
        .expect("answered");
    assert!(location(&forged).ends_with("&notice=expired"));
    let json = http
        .post(engine.url("/auth/secret"))
        .header(reqwest::header::COOKIE, &cookie)
        .json(&serde_json::json!({ "script": script, "name": "API_KEY", "value": VALUE }))
        .send()
        .await
        .expect("answered");
    assert_eq!(json.status(), 400);
    assert_eq!(about(&engine, &http, &path, &cookie).await["has"], false);

    // Somewhere else is not a way back.
    let offsite = http
        .post(engine.url("/auth/secret"))
        .header(reqwest::header::COOKIE, &cookie)
        .form(&[
            ("csrf_token", token.as_str()),
            ("script", script.as_str()),
            ("name", "API_KEY"),
            ("value", VALUE),
            ("redirect", "//evil.example/steal"),
        ])
        .send()
        .await
        .expect("answered");
    assert_eq!(location(&offsite), "/");

    // Signed out, the page sends the person to sign in and back.
    let signed_out = http.get(engine.url(&url)).send().await.expect("answered");
    assert!(signed_out.status().is_redirection());
    assert!(location(&signed_out).starts_with("/auth/login?redirect=%2Fauth%2Fsecret"));

    engine.shutdown().await;
}
