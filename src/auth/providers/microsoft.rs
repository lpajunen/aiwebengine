/// Microsoft Azure AD OAuth2/OIDC Provider Implementation
///
/// Implements OAuth2 authentication with Microsoft Azure Active Directory
/// using OpenID Connect. Supports both personal Microsoft accounts and
/// organizational/work accounts.
use super::{OAuth2Provider, OAuth2ProviderConfig, OAuth2TokenResponse, OAuth2UserInfo};
use crate::auth::error::AuthError;
use async_trait::async_trait;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const _MICROSOFT_AUTH_URL: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/authorize";
const _MICROSOFT_TOKEN_URL: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/token";
const MICROSOFT_USERINFO_URL: &str = "https://graph.microsoft.com/v1.0/me";
const _MICROSOFT_JWKS_URL: &str = "https://login.microsoftonline.com/common/discovery/v2.0/keys";
const _MICROSOFT_ISSUER: &str = "https://login.microsoftonline.com";

/// Microsoft ID token claims
#[derive(Debug, Deserialize, Serialize, Clone)]
struct MicrosoftIdTokenClaims {
    /// Issuer
    iss: String,

    /// Subject (user ID)
    sub: String,

    /// Audience (client ID)
    aud: String,

    /// Expiration time
    exp: u64,

    /// Issued at time
    iat: u64,

    /// Not before time
    nbf: Option<u64>,

    /// Email address
    email: Option<String>,

    /// Preferred username
    preferred_username: Option<String>,

    /// Name
    name: Option<String>,

    /// Object ID (Azure AD)
    oid: Option<String>,

    /// Tenant ID
    tid: Option<String>,

    /// Nonce
    nonce: Option<String>,
}

/// Microsoft Graph API user response
#[derive(Debug, Deserialize, Serialize)]
struct MicrosoftGraphUser {
    id: String,
    #[serde(rename = "userPrincipalName")]
    user_principal_name: Option<String>,
    #[serde(rename = "displayName")]
    display_name: Option<String>,
    #[serde(rename = "givenName")]
    given_name: Option<String>,
    surname: Option<String>,
    mail: Option<String>,
    #[serde(rename = "preferredLanguage")]
    preferred_language: Option<String>,
}

/// Microsoft OAuth2 token request
#[derive(Debug, Serialize)]
struct MicrosoftTokenRequest {
    code: String,
    client_id: String,
    client_secret: String,
    redirect_uri: String,
    grant_type: String,
    scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    code_verifier: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resource: Option<String>,
}

/// Microsoft OAuth2 token response
#[derive(Debug, Deserialize)]
struct MicrosoftTokenResponseRaw {
    access_token: String,
    token_type: String,
    expires_in: Option<u64>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    scope: Option<String>,
}

/// Microsoft OAuth2 refresh token request
#[derive(Debug, Serialize)]
struct MicrosoftRefreshRequest {
    refresh_token: String,
    client_id: String,
    client_secret: String,
    grant_type: String,
    scope: Option<String>,
}

/// Microsoft Azure AD OAuth2/OIDC Provider
pub struct MicrosoftProvider {
    config: OAuth2ProviderConfig,
    http_client: reqwest::Client,
    tenant_id: String,
}

impl MicrosoftProvider {
    /// Create a new Microsoft OAuth2 provider
    ///
    /// # Arguments
    /// * `config` - Provider configuration
    ///
    /// The tenant ID can be specified in extra_params as "tenant_id".
    /// If not specified, "common" is used (allows any Microsoft account).
    pub fn new(config: OAuth2ProviderConfig) -> Result<Self, AuthError> {
        // Set default scopes if none provided
        let mut config = config;
        if config.scopes.is_empty() {
            config.scopes = vec![
                "openid".to_string(),
                "email".to_string(),
                "profile".to_string(),
                "User.Read".to_string(), // Required for Graph API access
            ];
        }

        // Ensure openid scope is included for OIDC
        if !config.scopes.contains(&"openid".to_string()) {
            config.scopes.push("openid".to_string());
        }

        // Ensure User.Read scope is included for Graph API access
        if !config.scopes.contains(&"User.Read".to_string()) {
            config.scopes.push("User.Read".to_string());
        }

        // Extract tenant ID from extra params (default to "common")
        let tenant_id = config
            .extra_params
            .get("tenant_id")
            .cloned()
            .unwrap_or_else(|| "common".to_string());

        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| AuthError::OAuth2Error(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Self {
            config,
            http_client,
            tenant_id,
        })
    }

    /// Get authorization URL with tenant ID
    fn get_auth_url(&self) -> String {
        self.config.auth_url.clone().unwrap_or_else(|| {
            format!(
                "https://login.microsoftonline.com/{}/oauth2/v2.0/authorize",
                self.tenant_id
            )
        })
    }

    /// Get token URL with tenant ID
    fn get_token_url(&self) -> String {
        self.config.token_url.clone().unwrap_or_else(|| {
            format!(
                "https://login.microsoftonline.com/{}/oauth2/v2.0/token",
                self.tenant_id
            )
        })
    }

    /// Verify Microsoft ID token
    async fn verify_id_token(&self, id_token: &str) -> Result<MicrosoftIdTokenClaims, AuthError> {
        // Decode header to get key ID
        let header = decode_header(id_token)
            .map_err(|e| AuthError::JwtError(format!("Failed to decode ID token header: {}", e)))?;

        let kid = header
            .kid
            .ok_or_else(|| AuthError::JwtError("ID token missing key ID (kid)".to_string()))?;

        // Fetch Microsoft's public keys (JWKS)
        let jwks_url = format!(
            "https://login.microsoftonline.com/{}/discovery/v2.0/keys",
            self.tenant_id
        );

        let jwks_response = self
            .http_client
            .get(&jwks_url)
            .send()
            .await
            .map_err(|e| AuthError::OAuth2Error(format!("Failed to fetch JWKS: {}", e)))?
            .json::<serde_json::Value>()
            .await
            .map_err(|e| AuthError::OAuth2Error(format!("Failed to parse JWKS: {}", e)))?;

        // Find the matching key
        let keys = jwks_response["keys"]
            .as_array()
            .ok_or_else(|| AuthError::JwtError("Invalid JWKS format".to_string()))?;

        let matching_key = keys
            .iter()
            .find(|k| k["kid"].as_str() == Some(&kid))
            .ok_or_else(|| AuthError::JwtError(format!("Key ID {} not found in JWKS", kid)))?;

        // Extract RSA components
        let n = matching_key["n"]
            .as_str()
            .ok_or_else(|| AuthError::JwtError("Missing 'n' in JWK".to_string()))?;
        let e = matching_key["e"]
            .as_str()
            .ok_or_else(|| AuthError::JwtError("Missing 'e' in JWK".to_string()))?;

        // Create decoding key from RSA components
        let decoding_key = DecodingKey::from_rsa_components(n, e)
            .map_err(|e| AuthError::JwtError(format!("Failed to create decoding key: {}", e)))?;

        // Set up validation
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&[&self.config.client_id]);

        // Microsoft uses tenant-specific issuer URLs
        let issuer = format!("https://login.microsoftonline.com/{}/v2.0", self.tenant_id);
        validation.set_issuer(&[&issuer]);

        // Decode and validate token
        let token_data = decode::<MicrosoftIdTokenClaims>(id_token, &decoding_key, &validation)
            .map_err(|e| AuthError::JwtError(format!("ID token validation failed: {}", e)))?;

        Ok(token_data.claims)
    }

    /// Convert Microsoft Graph user to OAuth2UserInfo
    fn convert_graph_user(&self, user: MicrosoftGraphUser) -> OAuth2UserInfo {
        let mut raw_data = HashMap::new();
        raw_data.insert("id".to_string(), serde_json::json!(user.id));

        if let Some(upn) = &user.user_principal_name {
            raw_data.insert("userPrincipalName".to_string(), serde_json::json!(upn));
        }

        // Microsoft doesn't provide email_verified flag in Graph API
        // Assume verified for organizational accounts
        let email_verified = user.mail.is_some() || user.user_principal_name.is_some();

        OAuth2UserInfo {
            provider_user_id: user.id,
            email: user.mail.or(user.user_principal_name).unwrap_or_default(),
            email_verified,
            name: user.display_name,
            given_name: user.given_name,
            family_name: user.surname,
            picture: None, // Graph API requires separate photo endpoint
            locale: user.preferred_language,
            raw_data,
        }
    }

    /// Convert ID token claims to OAuth2UserInfo
    fn convert_id_token_claims(&self, claims: MicrosoftIdTokenClaims) -> OAuth2UserInfo {
        let mut raw_data = HashMap::new();
        raw_data.insert("iss".to_string(), serde_json::json!(claims.iss));
        raw_data.insert("iat".to_string(), serde_json::json!(claims.iat));
        raw_data.insert("exp".to_string(), serde_json::json!(claims.exp));

        if let Some(oid) = &claims.oid {
            raw_data.insert("oid".to_string(), serde_json::json!(oid));
        }
        if let Some(tid) = &claims.tid {
            raw_data.insert("tid".to_string(), serde_json::json!(tid));
        }

        let email = claims
            .email
            .or(claims.preferred_username)
            .unwrap_or_default();

        OAuth2UserInfo {
            provider_user_id: claims.sub,
            email,
            email_verified: true, // Microsoft validates emails
            name: claims.name,
            given_name: None,
            family_name: None,
            picture: None,
            locale: None,
            raw_data,
        }
    }
}

#[async_trait]
impl OAuth2Provider for MicrosoftProvider {
    fn name(&self) -> &str {
        "microsoft"
    }

    fn authorization_url(
        &self,
        state: &str,
        nonce: Option<&str>,
        code_challenge: Option<&str>,
        resource: Option<&str>,
    ) -> Result<String, AuthError> {
        let auth_url = self.get_auth_url();

        let mut url = url::Url::parse(&auth_url)
            .map_err(|e| AuthError::ConfigError(format!("Invalid auth URL: {}", e)))?;

        {
            let mut query = url.query_pairs_mut();
            query.append_pair("client_id", &self.config.client_id);
            query.append_pair("redirect_uri", &self.config.redirect_uri);
            query.append_pair("response_type", "code");
            query.append_pair("scope", &self.config.scopes.join(" "));
            query.append_pair("state", state);
            query.append_pair("response_mode", "query");

            if let Some(nonce) = nonce {
                query.append_pair("nonce", nonce);
            }

            // PKCE support (RFC 7636)
            if let Some(challenge) = code_challenge {
                query.append_pair("code_challenge", challenge);
                query.append_pair("code_challenge_method", "S256");
            }

            // Resource indicator (RFC 8707)
            if let Some(res) = resource {
                query.append_pair("resource", res);
            }

            // Add extra parameters (excluding tenant_id which is in URL)
            for (key, value) in &self.config.extra_params {
                if key != "tenant_id" {
                    query.append_pair(key, value);
                }
            }
        }

        Ok(url.to_string())
    }

    async fn exchange_code(
        &self,
        code: &str,
        _state: &str,
        code_verifier: Option<&str>,
        resource: Option<&str>,
    ) -> Result<OAuth2TokenResponse, AuthError> {
        let token_url = self.get_token_url();

        let token_request = MicrosoftTokenRequest {
            code: code.to_string(),
            client_id: self.config.client_id.clone(),
            client_secret: self.config.client_secret.clone(),
            redirect_uri: self.config.redirect_uri.clone(),
            grant_type: "authorization_code".to_string(),
            scope: Some(self.config.scopes.join(" ")),
            code_verifier: code_verifier.map(|s| s.to_string()),
            resource: resource.map(|s| s.to_string()),
        };

        let response = self
            .http_client
            .post(&token_url)
            .form(&token_request)
            .send()
            .await
            .map_err(|e| AuthError::OAuth2Error(format!("Token request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(AuthError::OAuth2Error(format!(
                "Token request failed with status {}: {}",
                status, error_text
            )));
        }

        let token_response: MicrosoftTokenResponseRaw = response.json().await.map_err(|e| {
            AuthError::OAuth2Error(format!("Failed to parse token response: {}", e))
        })?;

        Ok(OAuth2TokenResponse {
            access_token: token_response.access_token,
            token_type: token_response.token_type,
            expires_in: token_response.expires_in,
            refresh_token: token_response.refresh_token,
            id_token: token_response.id_token,
            scope: token_response.scope,
        })
    }

    async fn get_user_info(
        &self,
        access_token: &str,
        id_token: Option<&str>,
    ) -> Result<OAuth2UserInfo, AuthError> {
        // Prefer ID token for user info when available
        if let Some(id_token) = id_token
            && let Ok(claims) = self.verify_id_token(id_token).await
        {
            return Ok(self.convert_id_token_claims(claims));
        }
        // Fall through to Graph API if ID token validation fails

        // Use Microsoft Graph API for detailed user info
        let userinfo_url = self
            .config
            .userinfo_url
            .as_deref()
            .unwrap_or(MICROSOFT_USERINFO_URL);

        let response = self
            .http_client
            .get(userinfo_url)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|e| AuthError::OAuth2Error(format!("UserInfo request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(AuthError::OAuth2Error(format!(
                "UserInfo request failed with status {}: {}",
                status, error_text
            )));
        }

        let graph_user: MicrosoftGraphUser = response
            .json()
            .await
            .map_err(|e| AuthError::OAuth2Error(format!("Failed to parse userinfo: {}", e)))?;

        Ok(self.convert_graph_user(graph_user))
    }

    async fn refresh_token(&self, refresh_token: &str) -> Result<OAuth2TokenResponse, AuthError> {
        let token_url = self.get_token_url();

        let refresh_request = MicrosoftRefreshRequest {
            refresh_token: refresh_token.to_string(),
            client_id: self.config.client_id.clone(),
            client_secret: self.config.client_secret.clone(),
            grant_type: "refresh_token".to_string(),
            scope: Some(self.config.scopes.join(" ")),
        };

        let response = self
            .http_client
            .post(&token_url)
            .form(&refresh_request)
            .send()
            .await
            .map_err(|e| AuthError::OAuth2Error(format!("Refresh token request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(AuthError::OAuth2Error(format!(
                "Refresh token request failed with status {}: {}",
                status, error_text
            )));
        }

        let token_response: MicrosoftTokenResponseRaw = response.json().await.map_err(|e| {
            AuthError::OAuth2Error(format!("Failed to parse refresh response: {}", e))
        })?;

        Ok(OAuth2TokenResponse {
            access_token: token_response.access_token,
            token_type: token_response.token_type,
            expires_in: token_response.expires_in,
            refresh_token: token_response
                .refresh_token
                .or(Some(refresh_token.to_string())),
            id_token: token_response.id_token,
            scope: token_response.scope,
        })
    }

    async fn revoke_token(&self, _token: &str) -> Result<(), AuthError> {
        // Microsoft doesn't provide a token revocation endpoint
        // Tokens expire naturally or can be invalidated through Azure AD admin
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_config() -> OAuth2ProviderConfig {
        OAuth2ProviderConfig {
            client_id: "test-client-id".to_string(),
            client_secret: "test-client-secret".to_string(),
            scopes: vec![
                "openid".to_string(),
                "email".to_string(),
                "profile".to_string(),
                "User.Read".to_string(),
            ],
            redirect_uri: "https://example.com/auth/callback".to_string(),
            auth_url: None,
            token_url: None,
            userinfo_url: None,
            extra_params: HashMap::new(),
        }
    }

    #[test]
    fn test_microsoft_provider_creation() {
        let config = create_test_config();
        let provider = MicrosoftProvider::new(config);
        assert!(provider.is_ok());
    }

    #[test]
    fn test_authorization_url_generation() {
        let config = create_test_config();
        let provider = MicrosoftProvider::new(config).unwrap();

        let auth_url = provider
            .authorization_url("test-state", Some("test-nonce"), None, None)
            .unwrap();

        assert!(auth_url.contains("client_id=test-client-id"));
        assert!(auth_url.contains("state=test-state"));
        assert!(auth_url.contains("nonce=test-nonce"));
        assert!(auth_url.contains("response_type=code"));
        assert!(auth_url.contains("/common/oauth2/v2.0/authorize"));
    }

    #[test]
    fn test_custom_tenant_id() {
        let mut config = create_test_config();
        config
            .extra_params
            .insert("tenant_id".to_string(), "custom-tenant".to_string());

        let provider = MicrosoftProvider::new(config).unwrap();
        assert_eq!(provider.tenant_id, "custom-tenant");

        let auth_url = provider
            .authorization_url("test-state", None, None, None)
            .unwrap();
        assert!(auth_url.contains("/custom-tenant/oauth2/v2.0/authorize"));
    }

    #[test]
    fn test_provider_name() {
        let config = create_test_config();
        let provider = MicrosoftProvider::new(config).unwrap();
        assert_eq!(provider.name(), "microsoft");
    }
}
