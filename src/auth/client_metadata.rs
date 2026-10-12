//! Clients identified by a metadata document, and the redirect URIs any client
//! may use.
//!
//! **Client ID Metadata Documents** (draft-ietf-oauth-client-id-metadata-document,
//! the MCP authorization spec's preferred registration). A `client_id` that is
//! an HTTPS URL names a JSON document at that URL describing the client — its
//! name and redirect URIs — and the document must name the same URL as its own
//! `client_id`. Whoever controls that origin is the client, so the identifier
//! carries provenance that open registration cannot: anybody can register a
//! client called "Claude", but only Anthropic can publish one at
//! `https://claude.ai/…`. The consent page shows the origin for that reason.
//!
//! The document is fetched through [`crate::http_client::HttpClient`] — the
//! same URL, DNS and redirect checks a script's `fetch` gets, since the URL is
//! attacker-chosen — and kept in `oauth_clients`, so grants and refresh tokens
//! keyed by `client_id` work as they do for a registered client. It is fetched
//! again after [`REFRESH_AFTER_SECS`], and a stale copy is used for at most
//! [`STALE_FOR_SECS`] while the origin cannot be reached.
//!
//! **`application_type`** (OpenID Connect Dynamic Client Registration §2). A
//! `web` client redirects to HTTPS on a public host; a `native` one to a
//! loopback address over HTTP or to a private-use scheme (RFC 8252 §7). The
//! two do not mix in one client, and plain HTTP to anywhere but loopback is
//! refused for either. A client that does not say is taken to be whichever its
//! redirect URIs describe.

use chrono::Utc;
use sqlx::PgPool;

use crate::auth::client_registration::{RegisteredClient, RegisteredClientMetadata};
use crate::auth::error::AuthError;

/// How long a fetched document is used before it is fetched again.
pub const REFRESH_AFTER_SECS: i64 = 600;

/// How long a stale document stands in while its origin is unreachable.
pub const STALE_FOR_SECS: i64 = 86_400;

/// The largest document read. The draft suggests 5 KB; a client description
/// needing more is not one.
const MAX_DOCUMENT_BYTES: usize = 5 * 1024;

/// How long the origin gets to answer.
const FETCH_TIMEOUT_MS: u64 = 5_000;

/// Whether `client_id` is a metadata document URL, and the URL if so.
///
/// HTTPS, a host, a path, and nothing that would make two spellings name one
/// client or let the identifier smuggle something into the fetch: no
/// credentials, fragment or dot segments, and no `{{` (the HTTP client
/// substitutes `{{secret:…}}` in URLs).
pub fn document_url(client_id: &str) -> Option<url::Url> {
    if !client_id.starts_with("https://") || client_id.contains("{{") || client_id.len() > 2048 {
        return None;
    }
    let url = url::Url::parse(client_id).ok()?;
    let path = url.path();
    let dot_segment = path
        .split('/')
        .any(|segment| segment == "." || segment == "..");
    let ok = url.scheme() == "https"
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
        && path.len() > 1
        && !dot_segment
        // The parsed form is the identifier; a client_id the parser would
        // rewrite is two names for one thing.
        && url.as_str() == client_id;
    ok.then_some(url)
}

/// What a client's redirect URIs say it is: `web` or `native`.
///
/// `declared` is the client's `application_type`, when it gave one; otherwise
/// the URIs decide, and a set that is neither is refused. A fragment is never
/// allowed (RFC 6749 §3.1.2), and plain HTTP only to loopback.
pub fn application_type_for(
    declared: Option<&str>,
    redirect_uris: &[String],
) -> Result<&'static str, String> {
    let mut web = 0;
    let mut native = 0;
    for uri in redirect_uris {
        match redirect_kind(uri) {
            Some(RedirectKind::Web) => web += 1,
            Some(RedirectKind::Native) => native += 1,
            None => {
                return Err(format!(
                    "redirect_uri {uri} is neither HTTPS on a public host, HTTP on a loopback \
                     address, nor a private-use scheme"
                ));
            }
        }
    }
    match declared {
        Some("web") if native == 0 => Ok("web"),
        Some("web") => Err(
            "a web client redirects to HTTPS on a public host, not to loopback or a custom scheme"
                .to_string(),
        ),
        Some("native") if web == 0 => Ok("native"),
        Some("native") => Err(
            "a native client redirects to a loopback address or a private-use scheme".to_string(),
        ),
        Some(other) => Err(format!("unknown application_type {other}")),
        None if web > 0 && native > 0 => Err(
            "redirect_uris mix a web client's and a native client's; register them as two clients"
                .to_string(),
        ),
        None if native > 0 => Ok("native"),
        None => Ok("web"),
    }
}

enum RedirectKind {
    Web,
    Native,
}

fn redirect_kind(uri: &str) -> Option<RedirectKind> {
    let url = url::Url::parse(uri).ok()?;
    if url.fragment().is_some() {
        return None;
    }
    let loopback = matches!(
        url.host_str(),
        Some("localhost") | Some("127.0.0.1") | Some("[::1]")
    );
    match url.scheme() {
        "https" if !loopback && url.host_str().is_some() => Some(RedirectKind::Web),
        "http" if loopback => Some(RedirectKind::Native),
        "http" | "https" => None,
        // RFC 8252 §7.1: a private-use scheme is a reverse domain name, so it
        // names its owner the way a host does — `com.example.app:/callback`.
        scheme if scheme.contains('.') => Some(RedirectKind::Native),
        _ => None,
    }
}

/// Read a fetched document into the metadata a registered client holds.
pub fn read_document(client_id: &str, text: &str) -> Result<RegisteredClientMetadata, String> {
    if text.len() > MAX_DOCUMENT_BYTES {
        return Err(format!(
            "the document is larger than {MAX_DOCUMENT_BYTES} bytes"
        ));
    }
    let doc: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("the document is not JSON: {e}"))?;
    let field = |name: &str| doc.get(name);
    let string = |name: &str| field(name).and_then(|v| v.as_str()).map(str::to_string);
    let strings = |name: &str| -> Result<Option<Vec<String>>, String> {
        match field(name) {
            None => Ok(None),
            Some(value) => value
                .as_array()
                .and_then(|items| {
                    items
                        .iter()
                        .map(|item| item.as_str().map(str::to_string))
                        .collect::<Option<Vec<_>>>()
                })
                .map(Some)
                .ok_or_else(|| format!("{name} must be an array of strings")),
        }
    };

    if string("client_id").as_deref() != Some(client_id) {
        return Err("the document's client_id is not the URL it was fetched from".to_string());
    }
    let redirect_uris = strings("redirect_uris")?.unwrap_or_default();
    if redirect_uris.is_empty() {
        return Err("the document names no redirect_uris".to_string());
    }
    let application_type =
        application_type_for(string("application_type").as_deref(), &redirect_uris)?;

    // There is no secret to authenticate with: the client is whoever serves
    // the document, and the token endpoint takes PKCE from a public client.
    let method = string("token_endpoint_auth_method").unwrap_or_else(|| "none".to_string());
    if method != "none" {
        return Err(format!(
            "token_endpoint_auth_method {method} is not supported for a metadata document; use none"
        ));
    }
    let grant_types = strings("grant_types")?.unwrap_or_else(|| {
        vec![
            "authorization_code".to_string(),
            "refresh_token".to_string(),
        ]
    });
    if let Some(grant) = grant_types
        .iter()
        .find(|grant| !matches!(grant.as_str(), "authorization_code" | "refresh_token"))
    {
        return Err(format!("grant_type {grant} is not supported"));
    }
    let response_types = strings("response_types")?.unwrap_or_else(|| vec!["code".to_string()]);
    if response_types.iter().any(|kind| kind != "code") {
        return Err("only the code response_type is supported".to_string());
    }
    let client_name = string("client_name").map(|name| name.chars().take(200).collect());

    Ok(RegisteredClientMetadata {
        redirect_uris,
        client_name,
        logo_uri: string("logo_uri"),
        client_uri: string("client_uri"),
        contacts: strings("contacts")?,
        tos_uri: string("tos_uri"),
        policy_uri: string("policy_uri"),
        token_endpoint_auth_method: method,
        grant_types,
        response_types,
        scope: string("scope"),
        client_id_issued_at: Utc::now().timestamp(),
        application_type: Some(application_type.to_string()),
        document_fetched_at: Some(Utc::now().timestamp()),
    })
}

/// The client a request names, fetching its metadata document when the
/// identifier is one.
///
/// `Ok(None)` is no such client, as [`crate::auth::client_registration::lookup_client`]
/// answers; an unreachable or invalid document is an error saying why, which
/// the endpoints report to the client.
pub async fn resolve_client(
    pool: &PgPool,
    client_id: &str,
) -> Result<Option<RegisteredClient>, ResolveError> {
    resolve_client_with(pool, client_id, fetch_document).await
}

/// [`resolve_client`] with the fetch supplied, so the caching and validation
/// can be tested without an HTTPS origin.
pub async fn resolve_client_with<F, Fut>(
    pool: &PgPool,
    client_id: &str,
    fetch: F,
) -> Result<Option<RegisteredClient>, ResolveError>
where
    F: FnOnce(url::Url) -> Fut,
    Fut: std::future::Future<Output = Result<String, String>>,
{
    let stored = crate::auth::client_registration::lookup_client(pool, client_id)
        .await
        .map_err(ResolveError::Storage)?;
    let Some(url) = document_url(client_id) else {
        return Ok(stored);
    };

    let now = Utc::now().timestamp();
    let age = |client: &RegisteredClient| {
        client
            .metadata
            .document_fetched_at
            .map_or(i64::MAX, |at| now - at)
    };
    if let Some(client) = &stored
        && age(client) < REFRESH_AFTER_SECS
    {
        return Ok(stored);
    }

    let text = match fetch(url).await {
        Ok(text) => text,
        Err(reason) => {
            return match stored {
                Some(client) if age(&client) < STALE_FOR_SECS => {
                    tracing::warn!(
                        "Using a stale metadata document for {}: {}",
                        client_id,
                        reason
                    );
                    Ok(Some(client))
                }
                _ => Err(ResolveError::Document(format!(
                    "the client's metadata document could not be fetched: {reason}"
                ))),
            };
        }
    };
    let metadata = read_document(client_id, &text).map_err(|reason| {
        ResolveError::Document(format!(
            "the client's metadata document is not usable: {reason}"
        ))
    })?;
    store(pool, client_id, &metadata)
        .await
        .map_err(ResolveError::Storage)?;
    crate::auth::client_registration::lookup_client(pool, client_id)
        .await
        .map_err(ResolveError::Storage)
}

/// Why a client could not be resolved.
#[derive(Debug)]
pub enum ResolveError {
    /// The metadata document was unreachable or did not describe a client.
    Document(String),
    /// The client registry could not be read or written.
    Storage(AuthError),
}

async fn fetch_document(url: url::Url) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let client = crate::http_client::HttpClient::new().map_err(|e| e.to_string())?;
        let mut headers = std::collections::HashMap::new();
        headers.insert("accept".to_string(), "application/json".to_string());
        let options = crate::http_client::FetchOptions {
            headers: Some(headers),
            timeout_ms: Some(FETCH_TIMEOUT_MS),
            ..Default::default()
        };
        let response = client
            .fetch(url.to_string(), options, None, None)
            .map_err(|e| e.to_string())?;
        if response.status != 200 {
            return Err(format!("the origin answered {}", response.status));
        }
        Ok(response.body)
    })
    .await
    .map_err(|e| e.to_string())?
}

async fn store(
    pool: &PgPool,
    client_id: &str,
    metadata: &RegisteredClientMetadata,
) -> Result<(), AuthError> {
    let metadata_json = serde_json::to_value(metadata)
        .map_err(|e| AuthError::Internal(format!("client metadata is not encodable: {e}")))?;
    sqlx::query(
        "INSERT INTO oauth_clients (
             client_id, client_secret_hash, client_secret_expires_at, client_name,
             redirect_uris, grant_types, response_types, token_endpoint_auth_method,
             scope, metadata
         ) VALUES ($1, NULL, NULL, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (client_id) DO UPDATE SET
             client_name = EXCLUDED.client_name,
             redirect_uris = EXCLUDED.redirect_uris,
             grant_types = EXCLUDED.grant_types,
             response_types = EXCLUDED.response_types,
             token_endpoint_auth_method = EXCLUDED.token_endpoint_auth_method,
             scope = EXCLUDED.scope,
             metadata = EXCLUDED.metadata",
    )
    .bind(client_id)
    .bind(&metadata.client_name)
    .bind(&metadata.redirect_uris)
    .bind(&metadata.grant_types)
    .bind(&metadata.response_types)
    .bind(&metadata.token_endpoint_auth_method)
    .bind(&metadata.scope)
    .bind(&metadata_json)
    .execute(pool)
    .await
    .map_err(|e| AuthError::Internal(format!("storing client failed: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_metadata_document_url_is_https_with_a_path_and_nothing_ambiguous() {
        assert!(document_url("https://app.example.com/oauth/client.json").is_some());
        for refused in [
            "http://app.example.com/client.json",
            "https://app.example.com",
            "https://app.example.com/",
            "https://user:pw@app.example.com/client.json",
            "https://app.example.com/client.json#x",
            "https://app.example.com/a/../client.json",
            "https://app.example.com/{{secret:KEY}}",
            "client_1234",
        ] {
            assert!(document_url(refused).is_none(), "{refused}");
        }
    }

    #[test]
    fn redirect_uris_decide_the_application_type_when_it_is_not_declared() {
        let uris = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            application_type_for(None, &uris(&["https://app.example.com/cb"])),
            Ok("web")
        );
        assert_eq!(
            application_type_for(None, &uris(&["http://localhost:33418/callback"])),
            Ok("native")
        );
        assert_eq!(
            application_type_for(None, &uris(&["com.example.app:/callback"])),
            Ok("native")
        );
        assert!(
            application_type_for(
                None,
                &uris(&["https://app.example.com/cb", "http://127.0.0.1/cb"])
            )
            .is_err(),
            "one client is one kind"
        );
    }

    #[test]
    fn a_declared_type_must_match_its_redirect_uris() {
        let uris = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(application_type_for(Some("web"), &uris(&["http://localhost/cb"])).is_err());
        assert!(
            application_type_for(Some("native"), &uris(&["https://app.example.com/cb"])).is_err()
        );
        assert!(application_type_for(Some("desktop"), &uris(&["http://localhost/cb"])).is_err());
    }

    #[test]
    fn plain_http_off_loopback_and_fragments_are_refused() {
        let uris = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(application_type_for(None, &uris(&["http://app.example.com/cb"])).is_err());
        assert!(application_type_for(None, &uris(&["https://app.example.com/cb#x"])).is_err());
        assert!(application_type_for(None, &uris(&["javascript:alert(1)"])).is_err());
    }

    #[test]
    fn a_document_must_name_the_url_it_was_fetched_from() {
        let id = "https://app.example.com/client.json";
        let good = format!(
            r#"{{"client_id":"{id}","client_name":"App","redirect_uris":["https://app.example.com/cb"]}}"#
        );
        let metadata = read_document(id, &good).expect("a good document reads");
        assert_eq!(metadata.client_name.as_deref(), Some("App"));
        assert_eq!(metadata.token_endpoint_auth_method, "none");
        assert_eq!(metadata.application_type.as_deref(), Some("web"));

        let elsewhere = good.replace(id, "https://evil.example.com/client.json");
        assert!(read_document(id, &elsewhere).is_err());

        let secret = good.replace(
            r#""client_name""#,
            r#""token_endpoint_auth_method":"client_secret_basic","client_name""#,
        );
        assert!(read_document(id, &secret).is_err());

        let huge = format!(r#"{{"client_id":"{id}","pad":"{}"}}"#, "x".repeat(6000));
        assert!(read_document(id, &huge).is_err());
    }
}
