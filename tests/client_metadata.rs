//! Clients identified by the HTTPS URL of a metadata document, and the redirect
//! URIs a registered client may name.
//!
//! The fetch is supplied, so what is under test is the engine's handling —
//! storing, refreshing, falling back, refusing — rather than an HTTPS origin.

mod common;

use aiwebengine::auth::client_metadata::{ResolveError, resolve_client_with};
use aiwebengine::auth::client_registration::{
    ClientRegistrationManager, ClientRegistrationRequest,
};
use common::{setup_env, test_pool};

fn document(id: &str, name: &str) -> String {
    format!(
        r#"{{"client_id":"{id}","client_name":"{name}","redirect_uris":["http://localhost:33418/callback"]}}"#
    )
}

fn unique_id() -> String {
    format!(
        "https://app.example.com/clients/{}.json",
        uuid::Uuid::new_v4().simple()
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn a_document_is_fetched_once_kept_and_refreshed_when_stale() {
    setup_env().await;
    let pool = test_pool().await;
    let id = unique_id();

    let client = resolve_client_with(&pool, &id, |_| async { Ok(document(&id, "First")) })
        .await
        .expect("a good document resolves")
        .expect("and names a client");
    assert_eq!(client.display_name(), "First");
    assert_eq!(client.publisher().as_deref(), Some("app.example.com"));
    assert!(
        client.client_secret_hash.is_none(),
        "a document client is public"
    );
    assert!(client.redirect_uri_registered("http://localhost:33418/callback"));

    // Fresh: the stored copy answers and nothing is fetched.
    let again = resolve_client_with(&pool, &id, |_| async {
        Err::<String, String>("must not be fetched".to_string())
    })
    .await
    .expect("a fresh copy answers")
    .expect("still a client");
    assert_eq!(again.display_name(), "First");

    // Stale but under a day old, with the origin down: the stored copy stands.
    sqlx::query(
        "UPDATE oauth_clients SET metadata = jsonb_set(metadata, '{document_fetched_at}', \
         to_jsonb(EXTRACT(EPOCH FROM NOW())::bigint - 3600)) WHERE client_id = $1",
    )
    .bind(&id)
    .execute(&pool)
    .await
    .expect("aged");
    let fallback = resolve_client_with(&pool, &id, |_| async {
        Err::<String, String>("origin down".to_string())
    })
    .await
    .expect("a stale copy stands in")
    .expect("still a client");
    assert_eq!(fallback.display_name(), "First");

    // Stale and reachable: the new document replaces it.
    let refreshed = resolve_client_with(&pool, &id, |_| async { Ok(document(&id, "Second")) })
        .await
        .expect("a refreshed document resolves")
        .expect("still a client");
    assert_eq!(refreshed.display_name(), "Second");

    // Older than a day, origin down: refused rather than trusted forever.
    sqlx::query(
        "UPDATE oauth_clients SET metadata = jsonb_set(metadata, '{document_fetched_at}', \
         to_jsonb(EXTRACT(EPOCH FROM NOW())::bigint - 200000)) WHERE client_id = $1",
    )
    .bind(&id)
    .execute(&pool)
    .await
    .expect("aged");
    let refused = resolve_client_with(&pool, &id, |_| async {
        Err::<String, String>("origin down".to_string())
    })
    .await;
    assert!(matches!(refused, Err(ResolveError::Document(_))));
}

/// The document must say it is the client it was fetched for, or anybody's
/// document could be presented under somebody else's identifier.
#[tokio::test(flavor = "multi_thread")]
async fn a_document_naming_another_client_is_refused() {
    setup_env().await;
    let pool = test_pool().await;
    let id = unique_id();

    let result = resolve_client_with(&pool, &id, |_| async {
        Ok(document("https://evil.example.com/client.json", "Imposter"))
    })
    .await;
    assert!(matches!(result, Err(ResolveError::Document(_))));
}

/// A registered client's redirect URIs say what kind of client it is, and a
/// set that is neither kind is refused — including plain HTTP to a public host.
#[tokio::test(flavor = "multi_thread")]
async fn registration_reads_the_application_type_from_the_redirect_uris() {
    setup_env().await;
    let manager = ClientRegistrationManager::new(0, test_pool().await);
    let request = |uris: &[&str], declared: Option<&str>| ClientRegistrationRequest {
        redirect_uris: uris.iter().map(|s| s.to_string()).collect(),
        client_name: Some("Test".to_string()),
        logo_uri: None,
        client_uri: None,
        contacts: None,
        tos_uri: None,
        policy_uri: None,
        token_endpoint_auth_method: Some("none".to_string()),
        grant_types: Vec::new(),
        response_types: Vec::new(),
        scope: None,
        application_type: declared.map(str::to_string),
    };

    let native = manager
        .register_client(request(&["http://127.0.0.1:5000/cb"], None))
        .await
        .expect("a loopback client registers");
    assert_eq!(native.metadata.application_type.as_deref(), Some("native"));

    let app = manager
        .register_client(request(&["com.example.app:/callback"], Some("native")))
        .await
        .expect("a private-use scheme is a native client's");
    assert_eq!(app.metadata.application_type.as_deref(), Some("native"));

    for refused in [
        request(&["http://app.example.com/cb"], None),
        request(&["https://app.example.com/cb", "http://localhost/cb"], None),
        request(&["http://localhost/cb"], Some("web")),
    ] {
        assert!(manager.register_client(refused).await.is_err());
    }
}
