//! A signed-in visitor's browser cannot be made to change something by a page
//! on another origin, without the script doing anything to prevent it.

mod common;

use common::AdminServer;

const SCRIPT: &str = r#"
function change(context) {
  return { status: 200, body: "changed", contentType: "text/plain" };
}

function init() {
  routeRegistry.registerRoute("/cross-origin/change", { handler: "change", method: "POST" });
}
"#;

#[tokio::test(flavor = "multi_thread")]
async fn a_signed_in_post_from_another_origin_is_refused_before_the_script_runs() {
    let server = AdminServer::start().await.expect("server should start");
    server.deploy_script("cross-origin-probe", SCRIPT).await;
    let path = "/cross-origin/change";

    let cross_site = server
        .post(path)
        .header("sec-fetch-site", "cross-site")
        .send()
        .await
        .expect("request should be answered");
    assert_eq!(cross_site.status(), 403, "a cross-site POST with a session");

    // Another of the engine's hosts is the same site to `SameSite=Lax`, which
    // is the case the cookie alone does not cover.
    let same_site = server
        .post(path)
        .header("origin", "https://other.example")
        .send()
        .await
        .expect("request should be answered");
    assert_eq!(
        same_site.status(),
        403,
        "a POST whose origin is another host"
    );

    let same_origin = server
        .post(path)
        .header("sec-fetch-site", "same-origin")
        .send()
        .await
        .expect("request should be answered");
    assert_eq!(same_origin.status(), 200, "the page's own POST");

    // Not from a page, and no ambient credential to abuse: a webhook.
    let anonymous = server
        .anonymous()
        .post(server.url(path))
        .header("sec-fetch-site", "cross-site")
        .send()
        .await
        .expect("request should be answered");
    assert_eq!(anonymous.status(), 200, "a POST carrying no session");

    server.shutdown().await;
}
