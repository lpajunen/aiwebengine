/// OAuth 2.0 Dynamic Client Registration (RFC 7591)
///
/// Implements automated client registration for OAuth 2.0 authorization servers
/// allowing clients to register themselves without manual administrator intervention.
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use std::sync::Arc;
use uuid::Uuid;

use crate::auth::error::AuthError;
use crate::auth::security::AuthSecurityContext;
use crate::security::client_ip;

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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret: Option<String>,

    /// Time when client_secret expires (Unix timestamp)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_secret_expires_at: Option<i64>,

    /// All registered metadata
    #[serde(flatten)]
    pub metadata: RegisteredClientMetadata,
}

/// Registered client metadata (returned in response)
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RegisteredClientMetadata {
    pub redirect_uris: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logo_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contacts: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tos_uri: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub policy_uri: Option<String>,
    pub token_endpoint_auth_method: String,
    pub grant_types: Vec<String>,
    pub response_types: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,

    /// Time at which the client was registered (Unix timestamp, seconds)
    pub client_id_issued_at: i64,
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

impl RegisteredClient {
    /// Whether this client registered the exact redirect URI being asked for.
    ///
    /// Byte-for-byte comparison, deliberately. Normalising first — resolving
    /// `.` segments, folding a trailing slash, lower-casing a path — is how a
    /// redirect allowlist becomes a way to reach a URI the client never
    /// registered, and RFC 6749 §3.1.2.3 asks for simple string comparison for
    /// exactly that reason.
    pub fn redirect_uri_registered(&self, redirect_uri: &str) -> bool {
        self.metadata
            .redirect_uris
            .iter()
            .any(|registered| registered == redirect_uri)
    }

    /// Whether this client is allowed to use a grant type at all.
    pub fn allows_grant(&self, grant_type: &str) -> bool {
        self.metadata.grant_types.iter().any(|g| g == grant_type)
    }

    /// A name to show a person on the consent page. Falls back to the
    /// identifier, which is at least honest about there being nothing else.
    pub fn display_name(&self) -> &str {
        match self.metadata.client_name.as_deref() {
            Some(name) if !name.trim().is_empty() => name,
            _ => &self.client_id,
        }
    }
}

/// Whether a presented client secret matches the stored hash.
///
/// A free function rather than a method, because the token endpoint
/// authenticates a client without holding a registration manager. Compared in
/// constant time: the hash is not a password, but a comparison that returns
/// early on the first differing byte is a habit worth not having.
pub fn client_secret_matches(presented: &str, stored_hash: &str) -> bool {
    let computed = hash_client_secret(presented);

    computed.len() == stored_hash.len()
        && computed
            .bytes()
            .zip(stored_hash.bytes())
            .fold(0u8, |acc, (a, b)| acc | (a ^ b))
            == 0
}

/// Look up a client by the identifier an authorization request presented.
///
/// `Ok(None)` means no such client. The caller must treat that as a refusal
/// rather than a reason to fall back on whatever the request supplied — the
/// absence of this lookup is what let the authorization endpoint accept any
/// `client_id` and redirect to any URI.
pub async fn lookup_client(
    pool: &PgPool,
    client_id: &str,
) -> Result<Option<RegisteredClient>, AuthError> {
    let row = sqlx::query(
        "SELECT client_id, client_secret_hash, client_secret_expires_at, metadata, created_at
         FROM oauth_clients WHERE client_id = $1",
    )
    .bind(client_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| AuthError::Internal(format!("client lookup failed: {}", e)))?;

    let Some(row) = row else {
        return Ok(None);
    };

    let metadata: serde_json::Value = row
        .try_get("metadata")
        .map_err(|e| AuthError::Internal(format!("client row is malformed: {}", e)))?;
    let metadata: RegisteredClientMetadata = serde_json::from_value(metadata)
        .map_err(|e| AuthError::Internal(format!("client metadata is malformed: {}", e)))?;

    Ok(Some(RegisteredClient {
        client_id: row
            .try_get("client_id")
            .map_err(|e| AuthError::Internal(format!("client row is malformed: {}", e)))?,
        client_secret_hash: row.try_get("client_secret_hash").ok(),
        client_secret_expires_at: row.try_get("client_secret_expires_at").ok(),
        metadata,
        created_at: row
            .try_get("created_at")
            .map_err(|e| AuthError::Internal(format!("client row is malformed: {}", e)))?,
    }))
}

/// Dynamic Client Registration Manager
pub struct ClientRegistrationManager {
    secret_expiry_days: i64,
    pool: PgPool,
}

impl ClientRegistrationManager {
    /// Create a new client registration manager
    ///
    /// # Arguments
    /// * `secret_expiry_days` - Number of days until client secrets expire (0 = never)
    /// * `pool` - Where registered clients are stored. Registration that does
    ///   not persist is registration the authorization endpoint cannot check
    ///   anything against.
    pub fn new(secret_expiry_days: i64, pool: PgPool) -> Self {
        Self {
            secret_expiry_days,
            pool,
        }
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
        let client_secret = generate_client_secret();
        let client_secret_hash = hash_client_secret(&client_secret);

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
            client_id_issued_at: Utc::now().timestamp(),
        };

        // A public client holds no secret. Storing one anyway would be a
        // credential nobody can present and nothing checks, and handing it back
        // invites a client to treat itself as confidential when it is not.
        let is_public = metadata.token_endpoint_auth_method == "none";
        let stored_secret_hash = if is_public {
            None
        } else {
            Some(client_secret_hash)
        };

        let metadata_json = serde_json::to_value(&metadata)
            .map_err(|e| AuthError::Internal(format!("client metadata is not encodable: {}", e)))?;

        sqlx::query(
            "INSERT INTO oauth_clients (
                 client_id, client_secret_hash, client_secret_expires_at, client_name,
                 redirect_uris, grant_types, response_types, token_endpoint_auth_method,
                 scope, metadata
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(&client_id)
        .bind(&stored_secret_hash)
        .bind(if is_public { None } else { expires_at })
        .bind(&metadata.client_name)
        .bind(&metadata.redirect_uris)
        .bind(&metadata.grant_types)
        .bind(&metadata.response_types)
        .bind(&metadata.token_endpoint_auth_method)
        .bind(&metadata.scope)
        .bind(&metadata_json)
        .execute(&self.pool)
        .await
        .map_err(|e| AuthError::Internal(format!("storing client failed: {}", e)))?;

        // Return response
        Ok(ClientRegistrationResponse {
            client_id,
            client_secret: if is_public { None } else { Some(client_secret) },
            client_secret_expires_at: if is_public {
                None
            } else {
                expires_at_timestamp
            },
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
}

/// Generate a cryptographically secure client secret.
fn generate_client_secret() -> String {
    let bytes: Vec<u8> = (0..32).map(|_| rand::random::<u8>()).collect();
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    STANDARD.encode(&bytes)
}

/// Hash a client secret for storage.
fn hash_client_secret(secret: &str) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(secret.as_bytes()))
}

/// What the registration endpoint needs: somewhere to write the client, and
/// the budget that says how many a caller may write.
#[derive(Clone)]
pub struct ClientRegistrationState {
    pub manager: Arc<ClientRegistrationManager>,
    pub security: Arc<AuthSecurityContext>,
}

/// Axum handler for client registration endpoint
/// POST /auth/oauth2/register
///
/// Unauthenticated, per RFC 7591 — a client has no credential to present
/// before it has one, and this is how an MCP client onboards. What consent
/// bounds is what a registered client can then *do*; what bounds registration
/// itself is the per-address budget checked here, so `oauth_clients` cannot be
/// filled by a caller in a loop.
#[utoipa::path(
    post,
    path = "/auth/oauth2/register",
    tags = ["Authentication"],
    request_body = ClientRegistrationRequest,
    responses(
        (status = 200, description = "Client successfully registered", body = ClientRegistrationResponse),
        (status = 400, description = "Invalid client metadata"),
        (status = 429, description = "Too many registrations from this address"),
    )
)]
pub async fn register_client_handler(
    State(state): State<ClientRegistrationState>,
    headers: HeaderMap,
    Json(request): Json<ClientRegistrationRequest>,
) -> Result<Json<ClientRegistrationResponse>, Response> {
    let ip_addr = client_ip::from_headers(&headers);
    if !state.security.client_registration_allowed(&ip_addr).await {
        tracing::warn!("Client registration rate limit reached for {}", ip_addr);
        return Err((
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({
                "error": "invalid_client_metadata",
                "error_description": "Too many client registrations from this address; try again later",
            })),
        )
            .into_response());
    }

    match state.manager.register_client(request).await {
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

    /// Registration writes to Postgres, so these exercise the real insert the
    /// same way the rest of the suite does.
    fn test_pool() -> PgPool {
        PgPool::connect_lazy("postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine")
            .expect("lazy pool should be constructible")
    }

    #[tokio::test]
    async fn test_client_registration_basic() {
        let manager = ClientRegistrationManager::new(90, test_pool());

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
        let manager = ClientRegistrationManager::new(90, test_pool());

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

    #[tokio::test]
    async fn test_registration_response_is_rfc7591_compliant() {
        let manager = ClientRegistrationManager::new(90, test_pool());

        let request = ClientRegistrationRequest {
            redirect_uris: vec!["http://localhost:33418/callback".to_string()],
            client_name: Some("Claude Code".to_string()),
            logo_uri: None,
            client_uri: None,
            contacts: None,
            tos_uri: None,
            policy_uri: None,
            token_endpoint_auth_method: Some("none".to_string()),
            grant_types: vec![
                "authorization_code".to_string(),
                "refresh_token".to_string(),
            ],
            response_types: vec!["code".to_string()],
            scope: None,
        };

        let response = manager.register_client(request).await.unwrap();
        let json = serde_json::to_value(&response).unwrap();

        // RFC 7591: omitted optional metadata must be absent, not null
        for field in [
            "logo_uri",
            "client_uri",
            "contacts",
            "tos_uri",
            "policy_uri",
            "scope",
        ] {
            assert!(
                json.get(field).is_none(),
                "field `{}` must be omitted when not provided, got {:?}",
                field,
                json.get(field)
            );
        }

        // RFC 7591: client_id_issued_at is a numeric Unix timestamp
        assert!(json["client_id_issued_at"].is_i64());

        // A public client holds no secret, so none is minted or returned.
        assert!(
            response.client_secret.is_none(),
            "token_endpoint_auth_method=none must not be handed a client secret"
        );
    }

    #[test]
    fn test_client_secret_generation() {
        let secret1 = generate_client_secret();
        let secret2 = generate_client_secret();

        assert_ne!(secret1, secret2);
        assert!(secret1.len() > 32);
    }

    #[test]
    fn test_client_secret_verification() {
        let secret = "test_secret_12345";
        let hash = hash_client_secret(secret);

        assert!(client_secret_matches(secret, &hash));
        assert!(!client_secret_matches("wrong_secret", &hash));
    }
}
