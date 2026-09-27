//! Exposure belongs to the tree, not to `init()`.
//!
//! A file used to become world-readable because some line of `init()` named
//! it. Three things followed: you could not look at a file and know whether
//! it was public, the default was private while the *failure mode* was
//! public, and script data — prompts, skill definitions, few-shot examples —
//! was private only because nobody happened to name it.
//!
//! `public/` is served, `resources/` is an MCP resource, everything else is
//! reachable only to the linker and the script itself. Nothing is enforced
//! yet: these tests are about the convention and about the report that says
//! what enforcing it would cost.

mod common;

use aiwebengine::exposure::{self, Exposure};
use aiwebengine::js_engine::execute_script_secure;
use aiwebengine::repository;
use aiwebengine::security::UserContext;
use common::{setup_env, test_mutex};

/// The `scripts` row has to exist before any of its files: every file
/// references it.
fn ensure_script(script_uri: &str) {
    repository::upsert_script(script_uri, "function init() {}\n").expect("script should store");
}

fn store(script_uri: &str, path: &str, body: &str) {
    let now = std::time::SystemTime::now();
    repository::upsert_asset(repository::Asset {
        uri: path.to_string(),
        name: Some(path.to_string()),
        mimetype: "text/css".to_string(),
        content: body.as_bytes().to_vec(),
        created_at: now,
        updated_at: now,
        script_uri: script_uri.to_string(),
    })
    .expect("the file should be stored");
}

#[test]
fn the_directory_is_the_decision() {
    assert_eq!(exposure::of("public/app.js"), Exposure::Public);
    assert_eq!(exposure::of("resources/schema.json"), Exposure::Resource);
    // The category with no positive marker before this: it is private
    // because of where it is, not because nobody named it.
    assert_eq!(exposure::of("skills/refund.md"), Exposure::Private);
    assert_eq!(exposure::of("credentials.json"), Exposure::Private);
}

/// The report is what an operator reads before anything is enforced, so what
/// it must get right is the difference between "clean" and "we could not
/// look".
#[tokio::test(flavor = "multi_thread")]
async fn the_report_names_a_file_published_from_outside_public() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let admin = UserContext::admin("exposure-admin".to_string());
    let uri = "test://exposure/misplaced";
    ensure_script(uri);

    store(uri, "branding/logo.css", "body { color: red; }");
    store(uri, "public/ok.css", "body { color: blue; }");

    let script = r#"
        routeRegistry.registerAssetRoute("/exposure-misplaced.css", "branding/logo.css");
        routeRegistry.registerAssetRoute("/exposure-ok.css", "public/ok.css");
    "#;
    let result = execute_script_secure(uri, script, admin.clone());
    assert!(
        result.success,
        "registration should run: {:?}",
        result.error
    );

    let report = aiwebengine::engine_api::exposure_report_authorized(&admin)
        .expect("an administrator may read the report");

    let mine = report
        .scripts
        .iter()
        .find(|script| script.script_uri == uri)
        .expect("the script should appear in the report");

    let paths: Vec<&str> = mine.routes.iter().map(|r| r.path.as_str()).collect();
    assert_eq!(
        paths,
        vec!["branding/logo.css"],
        "only the file outside public/ is a problem; the one inside it is not"
    );
    assert_eq!(mine.routes[0].published_as, "/exposure-misplaced.css");
    assert_eq!(
        mine.routes[0].should_be, "public/branding/logo.css",
        "the report says where the file would have to move"
    );
}

/// Nothing is enforced yet, and the point of the report is that the operator
/// finds out before anything 404s rather than after.
#[tokio::test(flavor = "multi_thread")]
async fn a_misplaced_file_is_still_served() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let admin = UserContext::admin("exposure-admin-2".to_string());
    let uri = "test://exposure/still-served";
    ensure_script(uri);
    store(uri, "branding/still.css", "body { color: green; }");

    let script = r#"
        routeRegistry.registerAssetRoute("/exposure-still.css", "branding/still.css");
    "#;
    let result = execute_script_secure(uri, script, admin);
    assert!(
        result.success,
        "registration should run: {:?}",
        result.error
    );

    assert!(
        aiwebengine::asset_registry::get_global_registry()
            .is_path_registered("/exposure-still.css"),
        "the registration still stands — this lands as a warning and a report entry, not a refusal"
    );
}

/// The report is a map of exactly the mistakes worth exploiting: every file a
/// deployment serves from a directory that says it is private.
#[tokio::test(flavor = "multi_thread")]
async fn the_report_is_an_administrators_to_read() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    assert!(
        aiwebengine::engine_api::exposure_report_authorized(&UserContext::anonymous()).is_err(),
        "anonymous callers must not read it"
    );
    assert!(
        aiwebengine::engine_api::exposure_report_authorized(&UserContext::editor(
            "exposure-editor".to_string()
        ))
        .is_err(),
        "an editor owns scripts; this lists every script in the engine"
    );
    assert!(
        aiwebengine::engine_api::exposure_report_authorized(&UserContext::admin(
            "exposure-admin-3".to_string()
        ))
        .is_ok()
    );
}

/// A script with no `init()` at all never reaches
/// `update_script_init_status`, so it sits at `initialized = false` for ever
/// — and it is the clearest case there is, since a script that registers
/// nothing publishes nothing. Reading that flag reported almost every script
/// on a real deployment as unclassifiable, which is the same as reporting
/// nothing at all. A *failed* init is the uncertain one.
#[tokio::test(flavor = "multi_thread")]
async fn a_script_with_no_init_is_not_unclassifiable() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let admin = UserContext::admin("exposure-admin-4".to_string());
    let quiet = "test://exposure/no-init";
    repository::upsert_script(quiet, "const nothing = 1;\n").expect("script should store");

    let report = aiwebengine::engine_api::exposure_report_authorized(&admin)
        .expect("an administrator may read the report");

    assert!(
        !report
            .scripts
            .iter()
            .any(|script| script.script_uri == quiet),
        "a script that registers nothing has nothing to report, so it is not listed at all"
    );
}

/// An asset route was the one surface where the engine served data with no
/// way to ask who was asking. A route's handler *is* the hook, a stream has
/// its customization callback, and this had nothing — so "signed-in users
/// only" was inexpressible and "the person this file belongs to" doubly so.
///
/// Driven over real HTTP, because the point is that the bytes do not leave
/// the engine: an assertion against the registry would pass whether or not
/// the check runs.
#[tokio::test(flavor = "multi_thread")]
async fn an_asset_route_can_refuse_to_serve_its_file() {
    let engine = common::AdminServer::start()
        .await
        .expect("server failed to start");

    let script_uri = "https://example.com/authorized_asset_test";
    engine.deploy_script(script_uri, "function init() {}").await;
    store(script_uri, "public/secret.css", "body { color: red; }");
    store(script_uri, "public/open.css", "body { color: blue; }");

    let script = r#"
        function mayRead(context) {
          if (context.request.query.pass === "yes") {
            return {};
          }
          return { deny: 403, reason: "not yours" };
        }

        function broken(context) {
          throw new Error("the guard itself is wrong");
        }

        function init(context) {
          routeRegistry.registerAssetRoute("/authz-secret.css", "public/secret.css", {
            authorize: "mayRead",
          });
          routeRegistry.registerAssetRoute("/authz-broken.css", "public/secret.css", {
            authorize: "broken",
          });
          routeRegistry.registerAssetRoute("/authz-open.css", "public/open.css");
          return { success: true };
        }
    "#;
    engine.deploy_script(script_uri, script).await;

    let port = engine.port();
    let client = engine.client();
    let get = |path: String| {
        let client = client.clone();
        async move {
            let response = client
                .get(format!("http://127.0.0.1:{}{}", port, path))
                .send()
                .await
                .expect("request failed");
            let status = response.status().as_u16();
            let body = response.text().await.unwrap_or_default();
            (status, body)
        }
    };

    let (status, body) = get("/authz-secret.css".to_string()).await;
    assert_eq!(status, 403, "{}", body);
    assert_eq!(body, "not yours", "the reason reaches whoever was refused");
    assert!(
        !body.contains("color: red"),
        "the file must not be in the body of its own refusal"
    );

    let (status, body) = get("/authz-secret.css?pass=yes".to_string()).await;
    assert_eq!(status, 200, "{}", body);
    assert_eq!(body, "body { color: red; }");

    // A guard that fails is not a guard that allowed. Falling through to the
    // bytes would serve the file precisely when the thing protecting it is
    // broken.
    let (status, body) = get("/authz-broken.css".to_string()).await;
    assert_eq!(status, 500, "{}", body);
    assert!(
        !body.contains("color: red"),
        "a broken guard must not serve the file: {}",
        body
    );

    // And a registration with no `authorize` is what an asset route has
    // always been: open to anyone who can reach the host.
    let (status, body) = get("/authz-open.css".to_string()).await;
    assert_eq!(status, 200, "{}", body);
    assert_eq!(body, "body { color: blue; }");

    engine.shutdown().await;
}
