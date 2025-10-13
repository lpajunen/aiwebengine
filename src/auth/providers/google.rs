/// Google OAuth2/OIDC Provider Implementation
///
/// Implements OAuth2 authentication with Google using OpenID Connect.
/// Supports ID token verification and userinfo endpoint.
use super::{OAuth2Provider, OAuth2ProviderConfig, OAuth2TokenResponse, OAuth2UserInfo};
use crate::auth::error::AuthError;
use async_trait::async_trait;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

const GOOGLE_AUTH_URL: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_URL: &str = "https://oauth2.googleapis.com/token";
const GOOGLE_USERINFO_URL: &str = "https://www.googleapis.com/oauth2/v3/userinfo";
const GOOGLE_JWKS_URL: &str = "https://www.googleapis.com/oauth2/v3/certs";
const GOOGLE_ISSUER: &str = "https://accounts.google.com";

/// Google ID token claims (from JWT)
#[derive(Debug, Deserialize, Serialize)]
struct GoogleIdTokenClaims {
    /// Issuer (should be accounts.google.com)
    iss: String,

    /// Subject (user ID)
    sub: String,

    /// Audience (client ID)
    aud: String,

    /// Expiration time
    exp: u64,

    /// Issued at time
    iat: u64,

    /// Email address
    email: Option<String>,

    /// Email verified flag
    email_verified: Option<bool>,

    /// User's full name
    name: Option<String>,

    /// Given name
    given_name: Option<String>,

    /// Family name
    family_name: Option<String>,

    /// Profile picture URL
    picture: Option<String>,

    /// Locale
    locale: Option<String>,

    /// Hosted domain (for G Suite)
    hd: Option<String>,
}

/// Google userinfo response
#[derive(Debug, Deserialize, Serialize)]
struct GoogleUserInfoResponse {
    sub: String,
    email: Option<String>,
    email_verified: Option<bool>,
    name: Option<String>,
    given_name: Option<String>,
    family_name: Option<String>,
    picture: Option<String>,
    locale: Option<String>,
    hd: Option<String>,
}

/// Google OAuth2 token request
#[derive(Debug, Serialize)]
struct GoogleTokenRequest {
    code: String,
    client_id: String,
    client_secret: String,
    redirect_uri: String,
    grant_type: String,
}

/// Google OAuth2 token response
#[derive(Debug, Deserialize)]
struct GoogleTokenResponseRaw {
    access_token: String,
    token_type: String,
    expires_in: Option<u64>,
    refresh_token: Option<String>,
    id_token: Option<String>,
    scope: Option<String>,
}

/// Google OAuth2 refresh token request
#[derive(Debug, Serialize)]
struct GoogleRefreshRequest {
    refresh_token: String,
    client_id: String,
    client_secret: String,
    grant_type: String,
}

/// Google token revocation request
#[derive(Debug, Serialize)]
struct GoogleRevokeRequest {
    token: String,
}

/// Google OAuth2/OIDC Provider
pub struct GoogleProvider {
    config: OAuth2ProviderConfig,
    http_client: reqwest::Client,
}

impl GoogleProvider {
    /// Create a new Google OAuth2 provider
    pub fn new(config: OAuth2ProviderConfig) -> Result<Self, AuthError> {
        // Set default scopes if none provided
        let mut config = config;
        if config.scopes.is_empty() {
            config.scopes = vec![
                "openid".to_string(),
                "email".to_string(),
                "profile".to_string(),
            ];
        }

        // Ensure openid scope is included for OIDC
        if !config.scopes.contains(&"openid".to_string()) {
            config.scopes.push("openid".to_string());
        }

        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| AuthError::OAuth2Error(format!("Failed to create HTTP client: {}", e)))?;

        Ok(Self {
            config,
            http_client,
        })
    }

    /// Verify Google ID token
    async fn verify_id_token(&self, id_token: &str) -> Result<GoogleIdTokenClaims, AuthError> {
        // Decode header to get key ID
        let header = decode_header(id_token)
            .map_err(|e| AuthError::JwtError(format!("Failed to decode ID token header: {}", e)))?;

        let kid = header
            .kid
            .ok_or_else(|| AuthError::JwtError("ID token missing key ID (kid)".to_string()))?;

        // Fetch Google's public keys (JWKS)
        let jwks_response = self
            .http_client
            .get(GOOGLE_JWKS_URL)
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
        validation.set_issuer(&[GOOGLE_ISSUER, "accounts.google.com"]);

        // Decode and validate token
        let token_data = decode::<GoogleIdTokenClaims>(id_token, &decoding_key, &validation)
            .map_err(|e| AuthError::JwtError(format!("ID token validation failed: {}", e)))?;

        Ok(token_data.claims)
    }

    /// Convert Google userinfo to OAuth2UserInfo
    fn convert_userinfo(&self, info: GoogleUserInfoResponse) -> OAuth2UserInfo {
        let mut raw_data = HashMap::new();

        if let Some(hd) = &info.hd {
            raw_data.insert("hd".to_string(), serde_json::json!(hd));
        }

        OAuth2UserInfo {
            provider_user_id: info.sub,
            email: info.email.unwrap_or_default(),
            email_verified: info.email_verified.unwrap_or(false),
            name: info.name,
            given_name: info.given_name,
            family_name: info.family_name,
            picture: info.picture,
            locale: info.locale,
            raw_data,
        }
    }

    /// Convert ID token claims to OAuth2UserInfo
    fn convert_id_token_claims(&self, claims: GoogleIdTokenClaims) -> OAuth2UserInfo {
        let mut raw_data = HashMap::new();

        if let Some(hd) = &claims.hd {
            raw_data.insert("hd".to_string(), serde_json::json!(hd));
        }
        raw_data.insert("iss".to_string(), serde_json::json!(claims.iss));
        raw_data.insert("iat".to_string(), serde_json::json!(claims.iat));
        raw_data.insert("exp".to_string(), serde_json::json!(claims.exp));

        OAuth2UserInfo {
            provider_user_id: claims.sub,
            email: claims.email.unwrap_or_default(),
            email_verified: claims.email_verified.unwrap_or(false),
            name: claims.name,
            given_name: claims.given_name,
            family_name: claims.family_name,
            picture: claims.picture,
            locale: claims.locale,
            raw_data,
        }
    }
}

#[async_trait]
impl OAuth2Provider for GoogleProvider {
    fn name(&self) -> &str {
        "google"
    }

    fn authorization_url(&self, state: &str, nonce: Option<&str>) -> Result<String, AuthError> {
        let auth_url = self.config.auth_url.as_deref().unwrap_or(GOOGLE_AUTH_URL);

        let mut url = url::Url::parse(auth_url)
            .map_err(|e| AuthError::ConfigError(format!("Invalid auth URL: {}", e)))?;

        {
            let mut query = url.query_pairs_mut();
            query.append_pair("client_id", &self.config.client_id);
            query.append_pair("redirect_uri", &self.config.redirect_uri);
            query.append_pair("response_type", "code");
            query.append_pair("scope", &self.config.scopes.join(" "));
            query.append_pair("state", state);
            query.append_pair("access_type", "offline"); // Request refresh token
            query.append_pair("prompt", "consent"); // Force consent to get refresh token

            if let Some(nonce) = nonce {
                query.append_pair("nonce", nonce);
            }

            // Add extra parameters
            for (key, value) in &self.config.extra_params {
                query.append_pair(key, value);
            }
        }

        Ok(url.to_string())
    }

    async fn exchange_code(
        &self,
        code: &str,
        _state: &str,
    ) -> Result<OAuth2TokenResponse, AuthError> {
        let token_url = self.config.token_url.as_deref().unwrap_or(GOOGLE_TOKEN_URL);

        let token_request = GoogleTokenRequest {
            code: code.to_string(),
            client_id: self.config.client_id.clone(),
            client_secret: self.config.client_secret.clone(),
            redirect_uri: self.config.redirect_uri.clone(),
            grant_type: "authorization_code".to_string(),
        };

        let response = self
            .http_client
            .post(token_url)
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

        let token_response: GoogleTokenResponseRaw = response.json().await.map_err(|e| {
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
        // Prefer ID token for user info (more reliable and doesn't require extra API call)
        if let Some(id_token) = id_token {
            let claims = self.verify_id_token(id_token).await?;
            return Ok(self.convert_id_token_claims(claims));
        }

        // Fallback to userinfo endpoint
        let userinfo_url = self
            .config
            .userinfo_url
            .as_deref()
            .unwrap_or(GOOGLE_USERINFO_URL);

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

        let userinfo: GoogleUserInfoResponse = response
            .json()
            .await
            .map_err(|e| AuthError::OAuth2Error(format!("Failed to parse userinfo: {}", e)))?;

        Ok(self.convert_userinfo(userinfo))
    }

    async fn refresh_token(&self, refresh_token: &str) -> Result<OAuth2TokenResponse, AuthError> {
        let token_url = self.config.token_url.as_deref().unwrap_or(GOOGLE_TOKEN_URL);

        let refresh_request = GoogleRefreshRequest {
            refresh_token: refresh_token.to_string(),
            client_id: self.config.client_id.clone(),
            client_secret: self.config.client_secret.clone(),
            grant_type: "refresh_token".to_string(),
        };

        let response = self
            .http_client
            .post(token_url)
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

        let token_response: GoogleTokenResponseRaw = response.json().await.map_err(|e| {
            AuthError::OAuth2Error(format!("Failed to parse refresh response: {}", e))
        })?;

        Ok(OAuth2TokenResponse {
            access_token: token_response.access_token,
            token_type: token_response.token_type,
            expires_in: token_response.expires_in,
            refresh_token: Some(refresh_token.to_string()), // Keep the same refresh token
            id_token: token_response.id_token,
            scope: token_response.scope,
        })
    }

    async fn revoke_token(&self, token: &str) -> Result<(), AuthError> {
        let revoke_url = "https://oauth2.googleapis.com/revoke";

        let revoke_request = GoogleRevokeRequest {
            token: token.to_string(),
        };

        let response = self
            .http_client
            .post(revoke_url)
            .form(&revoke_request)
            .send()
            .await
            .map_err(|e| AuthError::OAuth2Error(format!("Token revocation failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            return Err(AuthError::OAuth2Error(format!(
                "Token revocation failed with status {}: {}",
                status, error_text
            )));
        }

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
            ],
            redirect_uri: "https://example.com/auth/callback".to_string(),
            auth_url: None,
            token_url: None,
            userinfo_url: None,
            extra_params: HashMap::new(),
        }
    }

    #[test]
    fn test_google_provider_creation() {
        let config = create_test_config();
        let provider = GoogleProvider::new(config);
        assert!(provider.is_ok());
    }

    #[test]
    fn test_authorization_url_generation() {
        let config = create_test_config();
        let provider = GoogleProvider::new(config).unwrap();

        let auth_url = provider
            .authorization_url("test-state", Some("test-nonce"))
            .unwrap();

        assert!(auth_url.contains("client_id=test-client-id"));
        assert!(auth_url.contains("state=test-state"));
        assert!(auth_url.contains("nonce=test-nonce"));
        assert!(auth_url.contains("scope=openid+email+profile"));
        assert!(auth_url.contains("response_type=code"));
    }

    #[test]
    fn test_provider_name() {
        let config = create_test_config();
        let provider = GoogleProvider::new(config).unwrap();
        assert_eq!(provider.name(), "google");
    }
}
