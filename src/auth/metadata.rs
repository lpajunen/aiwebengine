/// OAuth 2.0 Authorization Server Metadata (RFC 8414)
///
/// Implements the .well-known/oauth-authorization-server endpoint
/// for automatic discovery of authorization server capabilities.
use axum::{Json, extract::State, http::StatusCode, response::IntoResponse};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// OAuth 2.0 Authorization Server Metadata (RFC 8414 Section 2)
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone)]
pub struct MetadataConfig {
    /// Base URL of the authorization server (e.g., "https://auth.example.com")
    pub issuer: String,

    /// Whether dynamic client registration is enabled
    pub enable_registration: bool,

    /// Whether PKCE is required
    pub require_pkce: bool,

    /// Whether resource indicators are supported
    pub resource_indicators_supported: bool,
}

impl MetadataConfig {
    /// Create OAuth 2.0 authorization server metadata
    pub fn to_metadata(&self) -> AuthorizationServerMetadata {
        let issuer = self.issuer.trim_end_matches('/').to_string();

        AuthorizationServerMetadata {
            issuer: issuer.clone(),
            authorization_endpoint: format!("{}/oauth2/authorize", issuer),
            token_endpoint: format!("{}/oauth2/token", issuer),
            registration_endpoint: if self.enable_registration {
                Some(format!("{}/oauth2/register", issuer))
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
pub async fn metadata_handler(State(config): State<Arc<MetadataConfig>>) -> impl IntoResponse {
    let metadata = config.to_metadata();
    (StatusCode::OK, Json(metadata))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metadata_generation() {
        let config = MetadataConfig {
            issuer: "https://auth.example.com".to_string(),
            enable_registration: true,
            require_pkce: true,
            resource_indicators_supported: true,
        };

        let metadata = config.to_metadata();

        assert_eq!(metadata.issuer, "https://auth.example.com");
        assert_eq!(
            metadata.authorization_endpoint,
            "https://auth.example.com/oauth2/authorize"
        );
        assert_eq!(
            metadata.token_endpoint,
            "https://auth.example.com/oauth2/token"
        );
        assert_eq!(
            metadata.registration_endpoint,
            Some("https://auth.example.com/oauth2/register".to_string())
        );
        assert_eq!(
            metadata.code_challenge_methods_supported,
            Some(vec!["S256".to_string()])
        );
        assert_eq!(metadata.resource_indicators_supported, Some(true));
    }

    #[test]
    fn test_metadata_without_registration() {
        let config = MetadataConfig {
            issuer: "https://auth.example.com/".to_string(),
            enable_registration: false,
            require_pkce: false,
            resource_indicators_supported: false,
        };

        let metadata = config.to_metadata();

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
        let config = MetadataConfig {
            issuer: "https://auth.example.com///".to_string(),
            enable_registration: false,
            require_pkce: true,
            resource_indicators_supported: true,
        };

        let metadata = config.to_metadata();
        assert_eq!(metadata.issuer, "https://auth.example.com");
    }
}
