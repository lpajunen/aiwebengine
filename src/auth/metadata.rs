/// OAuth 2.0 Authorization Server Metadata (RFC 8414)
///
/// Implements the .well-known/oauth-authorization-server endpoint
/// for automatic discovery of authorization server capabilities.
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// OAuth 2.0 Authorization Server Metadata (RFC 8414 Section 2)
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct AuthorizationServerMetadata {
    /// The authorization server's issuer identifier (URL)
    pub issuer: String,

    /// URL of the authorization endpoint
    pub authorization_endpoint: String,

    /// URL of the token endpoint
    pub token_endpoint: String,

    /// URL of the dynamic client registration endpoint (RFC 7591)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registration_endpoint: Option<String>,

    /// URL of the JSON Web Key Set document
    #[serde(skip_serializing_if = "Option::is_none")]
    pub jwks_uri: Option<String>,

    /// OAuth 2.0 scopes supported
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scopes_supported: Option<Vec<String>>,

    /// Response types supported
    pub response_types_supported: Vec<String>,

    /// Response modes supported
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_modes_supported: Option<Vec<String>>,

    /// Grant types supported
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_types_supported: Option<Vec<String>>,

    /// Token endpoint authentication methods supported
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_endpoint_auth_methods_supported: Option<Vec<String>>,

    /// PKCE code challenge methods supported (RFC 7636)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code_challenge_methods_supported: Option<Vec<String>>,

    /// Whether authorization server supports RFC 8707 resource indicators
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_indicators_supported: Option<bool>,

    /// Service documentation URL
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_documentation: Option<String>,

    /// UI locales supported
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui_locales_supported: Option<Vec<String>>,

    /// Token introspection endpoint
    #[serde(skip_serializing_if = "Option::is_none")]
    pub introspection_endpoint: Option<String>,

    /// Token revocation endpoint
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revocation_endpoint: Option<String>,

    /// Whether TLS client certificate bound access tokens are supported
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tls_client_certificate_bound_access_tokens: Option<bool>,
}

/// Metadata configuration for the authorization server
///
/// A multi-host deployment is one authorization server per host rather than
/// one shared with the default host: RFC 8414 §3.3 requires the `issuer` in a
/// discovery document to be identical to the URL the document was fetched
/// from, and a client that sees otherwise must discard the response. Every
/// host serves the same `/auth/oauth2/*` endpoints and completes logins on
/// itself, so each one advertises itself.
#[derive(Debug, Clone)]
pub struct MetadataConfig {
    /// Base URL of the authorization server (e.g., "https://auth.example.com").
    /// Also what a request to an unconfigured host is answered with.
    default_issuer: String,

    /// Issuer per configured host, keyed the way request Host headers arrive.
    issuers_by_host: HashMap<String, String>,

    /// Whether dynamic client registration is enabled
    pub enable_registration: bool,

    /// Whether PKCE is required
    pub require_pkce: bool,

    /// Whether resource indicators are supported
    pub resource_indicators_supported: bool,
}

impl MetadataConfig {
    /// Build from every base URL the engine answers to, primary first.
    pub fn new(
        base_urls: &[String],
        enable_registration: bool,
        require_pkce: bool,
        resource_indicators_supported: bool,
    ) -> Self {
        let mut default_issuer = String::new();
        let mut issuers_by_host = HashMap::new();

        for base_url in base_urls {
            let issuer = base_url.trim().trim_end_matches('/').to_string();
            if issuer.is_empty() {
                continue;
            }
            if default_issuer.is_empty() {
                default_issuer = issuer.clone();
            }
            if let Some(host) = crate::config::base_url_authority(&issuer) {
                issuers_by_host.entry(host).or_insert(issuer);
            }
        }

        Self {
            default_issuer,
            issuers_by_host,
            enable_registration,
            require_pkce,
            resource_indicators_supported,
        }
    }

    /// The issuer a request addressed to `request_host` should be told about.
    ///
    /// The Host header is only ever a lookup key into the issuers built at
    /// startup, never interpolated into the URLs handed back, so a spoofed or
    /// unconfigured value falls back to the primary base URL instead of
    /// pointing a client at an endpoint of the caller's choosing.
    pub fn issuer_for_host(&self, request_host: Option<&str>) -> &str {
        request_host
            .map(|host| host.trim().to_lowercase())
            .and_then(|host| self.issuers_by_host.get(&host))
            .map(String::as_str)
            .unwrap_or(&self.default_issuer)
    }

    /// Create OAuth 2.0 authorization server metadata for the host a request
    /// was addressed to.
    pub fn to_metadata(&self, request_host: Option<&str>) -> AuthorizationServerMetadata {
        let issuer = self.issuer_for_host(request_host).to_string();

        AuthorizationServerMetadata {
            issuer: issuer.clone(),
            // The OAuth2 endpoints live under the reserved `/auth` prefix.
            // These are the only paths served; clients discover them here.
            authorization_endpoint: format!("{}/auth/oauth2/authorize", issuer),
            token_endpoint: format!("{}/auth/oauth2/token", issuer),
            registration_endpoint: if self.enable_registration {
                Some(format!("{}/auth/oauth2/register", issuer))
            } else {
                None
            },
            jwks_uri: None, // TODO: Add JWKS endpoint when implemented
            scopes_supported: Some(vec![
                "openid".to_string(),
                "profile".to_string(),
                "email".to_string(),
            ]),
            response_types_supported: vec!["code".to_string()],
            response_modes_supported: Some(vec!["query".to_string(), "fragment".to_string()]),
            grant_types_supported: Some(vec![
                "authorization_code".to_string(),
                "refresh_token".to_string(),
            ]),
            token_endpoint_auth_methods_supported: Some(vec![
                "client_secret_basic".to_string(),
                "client_secret_post".to_string(),
                "none".to_string(),
            ]),
            code_challenge_methods_supported: if self.require_pkce {
                Some(vec!["S256".to_string()])
            } else {
                Some(vec!["S256".to_string(), "plain".to_string()])
            },
            resource_indicators_supported: Some(self.resource_indicators_supported),
            service_documentation: None,
            ui_locales_supported: Some(vec!["en".to_string()]),
            introspection_endpoint: None, // TODO: Add when implemented
            revocation_endpoint: None,    // TODO: Add when implemented
            tls_client_certificate_bound_access_tokens: Some(false),
        }
    }
}

/// Axum handler for OAuth 2.0 authorization server metadata endpoint
/// GET /.well-known/oauth-authorization-server
#[utoipa::path(
    get,
    path = "/.well-known/oauth-authorization-server",
    tags = ["Authentication"],
    responses(
        (status = 200, description = "OAuth 2.0 authorization server metadata (RFC 8414)", body = AuthorizationServerMetadata),
    )
)]
pub async fn metadata_handler(
    State(config): State<Arc<MetadataConfig>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    let host = crate::auth::routes::get_request_host(&headers);
    let metadata = config.to_metadata(host.as_deref());
    (StatusCode::OK, Json(metadata))
}

/// OAuth 2.0 Protected Resource Metadata (RFC 8414 Section 5)
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ProtectedResourceMetadata {
    /// The protected resource's identifier
    pub resource: String,

    /// Authorization servers that can issue tokens for this resource
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authorization_servers: Option<Vec<String>>,

    /// OAuth 2.0 Bearer Token Usage endpoint
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bearer_methods_supported: Option<Vec<String>>,

    /// Resource indicators supported
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_signing_alg_values_supported: Option<Vec<String>>,
}

/// Handler for /.well-known/oauth-protected-resource
#[utoipa::path(
    get,
    path = "/.well-known/oauth-protected-resource",
    tags = ["Authentication"],
    responses(
        (status = 200, description = "OAuth 2.0 protected resource metadata (RFC 8414)", body = ProtectedResourceMetadata),
    )
)]
pub async fn protected_resource_metadata_handler(
    State(config): State<Arc<MetadataConfig>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // The resource identifier is the host the client actually addressed, not
    // the engine's default one, or a client validating the document against
    // the URL it requested rejects it (RFC 9728 §3.3).
    let host = crate::auth::routes::get_request_host(&headers);
    let issuer = config.issuer_for_host(host.as_deref()).to_string();

    let metadata = ProtectedResourceMetadata {
        resource: issuer.clone(),
        authorization_servers: Some(vec![issuer]),
        bearer_methods_supported: Some(vec!["header".to_string(), "body".to_string()]),
        resource_signing_alg_values_supported: None,
    };

    (StatusCode::OK, Json(metadata))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(base_urls: &[&str], enable_registration: bool, require_pkce: bool) -> MetadataConfig {
        let urls: Vec<String> = base_urls.iter().map(|u| u.to_string()).collect();
        MetadataConfig::new(&urls, enable_registration, require_pkce, true)
    }

    #[test]
    fn test_metadata_generation() {
        let config = config(&["https://auth.example.com"], true, true);

        let metadata = config.to_metadata(None);

        assert_eq!(metadata.issuer, "https://auth.example.com");
        assert_eq!(
            metadata.authorization_endpoint,
            "https://auth.example.com/auth/oauth2/authorize"
        );
        assert_eq!(
            metadata.token_endpoint,
            "https://auth.example.com/auth/oauth2/token"
        );
        assert_eq!(
            metadata.registration_endpoint,
            Some("https://auth.example.com/auth/oauth2/register".to_string())
        );
        assert_eq!(
            metadata.code_challenge_methods_supported,
            Some(vec!["S256".to_string()])
        );
        assert_eq!(metadata.resource_indicators_supported, Some(true));
    }

    #[test]
    fn test_metadata_without_registration() {
        let mut config = config(&["https://auth.example.com/"], false, false);
        config.resource_indicators_supported = false;

        let metadata = config.to_metadata(None);

        assert_eq!(metadata.issuer, "https://auth.example.com");
        assert_eq!(metadata.registration_endpoint, None);
        assert_eq!(
            metadata.code_challenge_methods_supported,
            Some(vec!["S256".to_string(), "plain".to_string()])
        );
        assert_eq!(metadata.resource_indicators_supported, Some(false));
    }

    #[test]
    fn test_issuer_normalization() {
        let config = config(&["https://auth.example.com///"], false, true);

        let metadata = config.to_metadata(None);
        assert_eq!(metadata.issuer, "https://auth.example.com");
    }

    /// Every configured host is its own issuer, so the document a client
    /// fetches matches the URL it fetched it from (RFC 8414 §3.3).
    #[test]
    fn additional_host_issues_for_itself() {
        let config = config(
            &["https://softagen.com", "https://manage.softagen.com"],
            true,
            true,
        );

        let metadata = config.to_metadata(Some("manage.softagen.com"));

        assert_eq!(metadata.issuer, "https://manage.softagen.com");
        assert_eq!(
            metadata.authorization_endpoint,
            "https://manage.softagen.com/auth/oauth2/authorize"
        );
        assert_eq!(
            metadata.token_endpoint,
            "https://manage.softagen.com/auth/oauth2/token"
        );
        assert_eq!(
            metadata.registration_endpoint,
            Some("https://manage.softagen.com/auth/oauth2/register".to_string())
        );
    }

    #[test]
    fn host_lookup_ignores_case_and_whitespace() {
        let config = config(
            &["https://softagen.com", "https://manage.softagen.com"],
            true,
            true,
        );

        assert_eq!(
            config.issuer_for_host(Some(" Manage.Softagen.com ")),
            "https://manage.softagen.com"
        );
    }

    /// A Host header naming somewhere the engine was not configured for gets
    /// the primary base URL rather than a URL built from the header itself.
    #[test]
    fn unconfigured_host_falls_back_to_the_primary_base_url() {
        let config = config(
            &["https://softagen.com", "https://manage.softagen.com"],
            true,
            true,
        );

        for host in [Some("attacker.example.com"), Some("127.0.0.1:3000"), None] {
            assert_eq!(config.issuer_for_host(host), "https://softagen.com");
        }
    }

    /// A base URL with a port keeps it, since that is how the Host header
    /// arrives on a local or non-standard-port deployment.
    #[test]
    fn host_with_port_is_matched() {
        let config = config(&["http://localhost:3000"], true, true);

        assert_eq!(
            config.issuer_for_host(Some("localhost:3000")),
            "http://localhost:3000"
        );
    }
}
