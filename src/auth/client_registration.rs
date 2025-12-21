/// OAuth 2.0 Dynamic Client Registration (RFC 7591)
///
/// Implements automated client registration for OAuth 2.0 authorization servers
/// allowing clients to register themselves without manual administrator intervention.
use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::error::AuthError;

/// Client metadata submitted during registration (RFC 7591 Section 2)
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
pub struct ClientRegistrationRequest {
    /// Array of redirection URIs for use in redirect-based flows
    #[serde(default)]
    pub redirect_uris: Vec<String>,

    /// Client name (human-readable)
    pub client_name: Option<String>,

    /// URL of client logo
    pub logo_uri: Option<String>,

    /// URL of client homepage
    pub client_uri: Option<String>,

    /// Email addresses of people responsible for this client
    pub contacts: Option<Vec<String>>,

    /// URL for the client's terms of service
    pub tos_uri: Option<String>,

    /// URL for the client's privacy policy
    pub policy_uri: Option<String>,

    /// Requested authentication method for the token endpoint
    /// Default: client_secret_basic
    pub token_endpoint_auth_method: Option<String>,

    /// Grant types the client will use
    /// Default: ["authorization_code"]
    #[serde(default)]
    pub grant_types: Vec<String>,

    /// Response types the client will use
    /// Default: ["code"]
    #[serde(default)]
    pub response_types: Vec<String>,

    /// OAuth 2.0 scopes the client may request
    #[serde(default)]
    pub scope: Option<String>,
}

/// Successful client registration response (RFC 7591 Section 3.2.1)
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct ClientRegistrationResponse {
    /// Unique client identifier
    pub client_id: String,

    /// Client secret (for confidential clients)
    pub client_secret: Option<String>,

    /// Time when client_secret expires (Unix timestamp)
    pub client_secret_expires_at: Option<i64>,

    /// All registered metadata
    #[serde(flatten)]
    pub metadata: RegisteredClientMetadata,
}

/// Registered client metadata (returned in response)
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RegisteredClientMetadata {
    pub redirect_uris: Vec<String>,
    pub client_name: Option<String>,
    pub logo_uri: Option<String>,
    pub client_uri: Option<String>,
    pub contacts: Option<Vec<String>>,
    pub tos_uri: Option<String>,
    pub policy_uri: Option<String>,
    pub token_endpoint_auth_method: String,
    pub grant_types: Vec<String>,
    pub response_types: Vec<String>,
    pub scope: Option<String>,

    /// Time at which the client was registered (ISO 8601)
    pub client_id_issued_at: String,
}

/// Stored client information in the database
#[derive(Debug, Clone)]
pub struct RegisteredClient {
    pub client_id: String,
    pub client_secret_hash: Option<String>,
    pub client_secret_expires_at: Option<DateTime<Utc>>,
    pub metadata: RegisteredClientMetadata,
    pub created_at: DateTime<Utc>,
}

/// Dynamic Client Registration Manager
pub struct ClientRegistrationManager {
    // TODO: Add database connection for persistent storage
    secret_expiry_days: i64,
}

impl ClientRegistrationManager {
    /// Create a new client registration manager
    ///
    /// # Arguments
    /// * `secret_expiry_days` - Number of days until client secrets expire (0 = never)
    pub fn new(secret_expiry_days: i64) -> Self {
        Self { secret_expiry_days }
    }

    /// Register a new OAuth2 client (RFC 7591 Section 3)
    ///
    /// # Arguments
    /// * `request` - Client registration metadata
    ///
    /// # Returns
    /// Client credentials and registered metadata
    pub async fn register_client(
        &self,
        request: ClientRegistrationRequest,
    ) -> Result<ClientRegistrationResponse, AuthError> {
        // Validate request
        self.validate_registration_request(&request)?;

        // Generate client_id
        let client_id = format!("client_{}", Uuid::new_v4());

        // Generate client_secret for confidential clients
        let client_secret = self.generate_client_secret();
        let client_secret_hash = self.hash_client_secret(&client_secret)?;

        // Calculate expiration
        let (expires_at, expires_at_timestamp) = if self.secret_expiry_days > 0 {
            let exp = Utc::now() + Duration::days(self.secret_expiry_days);
            (Some(exp), Some(exp.timestamp()))
        } else {
            (None, None)
        };

        // Set defaults for optional fields
        let token_endpoint_auth_method = request
            .token_endpoint_auth_method
            .unwrap_or_else(|| "client_secret_basic".to_string());

        let grant_types = if request.grant_types.is_empty() {
            vec!["authorization_code".to_string()]
        } else {
            request.grant_types
        };

        let response_types = if request.response_types.is_empty() {
            vec!["code".to_string()]
        } else {
            request.response_types
        };

        // Create registered metadata
        let metadata = RegisteredClientMetadata {
            redirect_uris: request.redirect_uris,
            client_name: request.client_name,
            logo_uri: request.logo_uri,
            client_uri: request.client_uri,
            contacts: request.contacts,
            tos_uri: request.tos_uri,
            policy_uri: request.policy_uri,
            token_endpoint_auth_method,
            grant_types,
            response_types,
            scope: request.scope,
            client_id_issued_at: Utc::now().to_rfc3339(),
        };

        // TODO: Store in database
        let _registered_client = RegisteredClient {
            client_id: client_id.clone(),
            client_secret_hash: Some(client_secret_hash),
            client_secret_expires_at: expires_at,
            metadata: metadata.clone(),
            created_at: Utc::now(),
        };

        // Return response
        Ok(ClientRegistrationResponse {
            client_id,
            client_secret: Some(client_secret),
            client_secret_expires_at: expires_at_timestamp,
            metadata,
        })
    }

    /// Validate client registration request
    fn validate_registration_request(
        &self,
        request: &ClientRegistrationRequest,
    ) -> Result<(), AuthError> {
        // Validate redirect_uris for authorization_code grant
        if request.grant_types.is_empty()
            || request
                .grant_types
                .contains(&"authorization_code".to_string())
        {
            if request.redirect_uris.is_empty() {
                return Err(AuthError::ConfigError(
                    "redirect_uris required for authorization_code grant".to_string(),
                ));
            }

            // Validate URI format
            for uri in &request.redirect_uris {
                if !uri.starts_with("http://") && !uri.starts_with("https://") {
                    return Err(AuthError::ConfigError(format!(
                        "Invalid redirect_uri: {}",
                        uri
                    )));
                }
            }
        }

        // Validate token_endpoint_auth_method
        if let Some(ref method) = request.token_endpoint_auth_method
            && !matches!(
                method.as_str(),
                "client_secret_basic" | "client_secret_post" | "none"
            )
        {
            return Err(AuthError::ConfigError(format!(
                "Unsupported token_endpoint_auth_method: {}",
                method
            )));
        }

        // Validate grant_types
        for grant_type in &request.grant_types {
            if !matches!(
                grant_type.as_str(),
                "authorization_code" | "refresh_token" | "client_credentials"
            ) {
                return Err(AuthError::ConfigError(format!(
                    "Unsupported grant_type: {}",
                    grant_type
                )));
            }
        }

        Ok(())
    }

    /// Generate a cryptographically secure client secret
    fn generate_client_secret(&self) -> String {
        use rand::Rng;
        let mut rng = rand::rng();

        // Generate 32 random bytes
        let bytes: Vec<u8> = (0..32).map(|_| rng.random()).collect();

        // Base64 encode
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        STANDARD.encode(&bytes)
    }

    /// Hash client secret for storage
    fn hash_client_secret(&self, secret: &str) -> Result<String, AuthError> {
        use sha2::{Digest, Sha256};

        let mut hasher = Sha256::new();
        hasher.update(secret.as_bytes());
        let result = hasher.finalize();

        Ok(format!("{:x}", result))
    }

    /// Verify client credentials
    pub fn verify_client_secret(&self, secret: &str, hash: &str) -> Result<bool, AuthError> {
        let computed_hash = self.hash_client_secret(secret)?;
        Ok(computed_hash == hash)
    }
}

/// Axum handler for client registration endpoint
/// POST /oauth2/register
#[utoipa::path(
    post,
    path = "/oauth2/register",
    tags = ["Authentication"],
    request_body = ClientRegistrationRequest,
    responses(
        (status = 200, description = "Client successfully registered", body = ClientRegistrationResponse),
        (status = 400, description = "Invalid client metadata"),
    )
)]
pub async fn register_client_handler(
    State(manager): State<Arc<ClientRegistrationManager>>,
    Json(request): Json<ClientRegistrationRequest>,
) -> Result<Json<ClientRegistrationResponse>, Response> {
    match manager.register_client(request).await {
        Ok(response) => Ok(Json(response)),
        Err(err) => Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "invalid_client_metadata",
                "error_description": err.to_string(),
            })),
        )
            .into_response()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_client_registration_basic() {
        let manager = ClientRegistrationManager::new(90);

        let request = ClientRegistrationRequest {
            redirect_uris: vec!["https://example.com/callback".to_string()],
            client_name: Some("Test Client".to_string()),
            logo_uri: None,
            client_uri: None,
            contacts: None,
            tos_uri: None,
            policy_uri: None,
            token_endpoint_auth_method: None,
            grant_types: vec![],
            response_types: vec![],
            scope: Some("read write".to_string()),
        };

        let response = manager.register_client(request).await.unwrap();

        assert!(response.client_id.starts_with("client_"));
        assert!(response.client_secret.is_some());
        assert_eq!(
            response.metadata.token_endpoint_auth_method,
            "client_secret_basic"
        );
        assert_eq!(response.metadata.grant_types, vec!["authorization_code"]);
        assert_eq!(response.metadata.response_types, vec!["code"]);
    }

    #[tokio::test]
    async fn test_client_registration_validation() {
        let manager = ClientRegistrationManager::new(90);

        // Missing redirect_uris
        let request = ClientRegistrationRequest {
            redirect_uris: vec![],
            client_name: Some("Test Client".to_string()),
            logo_uri: None,
            client_uri: None,
            contacts: None,
            tos_uri: None,
            policy_uri: None,
            token_endpoint_auth_method: None,
            grant_types: vec![],
            response_types: vec![],
            scope: None,
        };

        let result = manager.register_client(request).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_client_secret_generation() {
        let manager = ClientRegistrationManager::new(0);

        let secret1 = manager.generate_client_secret();
        let secret2 = manager.generate_client_secret();

        assert_ne!(secret1, secret2);
        assert!(secret1.len() > 32);
    }

    #[test]
    fn test_client_secret_verification() {
        let manager = ClientRegistrationManager::new(0);

        let secret = "test_secret_12345";
        let hash = manager.hash_client_secret(secret).unwrap();

        assert!(manager.verify_client_secret(secret, &hash).unwrap());
        assert!(!manager.verify_client_secret("wrong_secret", &hash).unwrap());
    }
}
