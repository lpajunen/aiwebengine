/// Apple Sign In OAuth2/OIDC Provider Implementation
///
/// Implements OAuth2 authentication with Apple Sign In using OpenID Connect.
/// Apple requires special handling including client secret generation using JWT.

use super::{OAuth2Provider, OAuth2ProviderConfig, OAuth2TokenResponse, OAuth2UserInfo};
use crate::auth::error::AuthError;
use async_trait::async_trait;
use jsonwebtoken::{decode, decode_header, encode, DecodingKey, EncodingKey, Header, Validation, Algorithm};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

const APPLE_AUTH_URL: &str = "https://appleid.apple.com/auth/authorize";
const APPLE_TOKEN_URL: &str = "https://appleid.apple.com/auth/token";
const APPLE_JWKS_URL: &str = "https://appleid.apple.com/auth/keys";
const APPLE_ISSUER: &str = "https://appleid.apple.com";

/// Apple ID token claims
#[derive(Debug, Deserialize, Serialize)]
struct AppleIdTokenClaims {
    /// Issuer (should be appleid.apple.com)
    iss: String,
    
    /// Subject (user ID)
    sub: String,
    
    /// Audience (client ID)
    aud: String,
    
    /// Expiration time
    exp: u64,
    
    /// Issued at time
    iat: u64,
    
    /// Nonce
    nonce: Option<String>,
    
    /// Email address
    email: Option<String>,
    
    /// Email verified flag ("true" or "false" as string)
    email_verified: Option<String>,
    
    /// Is private email (relay email)
    is_private_email: Option<String>,
    
    /// Real user status
    real_user_status: Option<i32>,
}

/// Apple client secret JWT claims
/// Apple requires generating a client secret as a signed JWT
#[derive(Debug, Serialize)]
struct AppleClientSecretClaims {
    iss: String, // Team ID
    iat: u64,
    exp: u64,
    aud: String, // https://appleid.apple.com
    sub: String, // Client ID (Service ID)
}

/// Apple OAuth2 token request
#[derive(Debug, Serialize)]
struct AppleTokenRequest {
    code: String,
    client_id: String,
    client_secret: String,
    redirect_uri: String,
    grant_type: String,
}

/// Apple OAuth2 token response
#[derive(Debug, Deserialize)]
struct AppleTokenResponseRaw {
    access_token: String,
    token_type: String,
    expires_in: Option<u64>,
    refresh_token: Option<String>,
    id_token: Option<String>,
}

/// Apple OAuth2 refresh token request
#[derive(Debug, Serialize)]
struct AppleRefreshRequest {
    refresh_token: String,
    client_id: String,
    client_secret: String,
    grant_type: String,
}

/// Apple OAuth2 revoke request
#[derive(Debug, Serialize)]
struct AppleRevokeRequest {
    token: String,
    client_id: String,
    client_secret: String,
    token_type_hint: String,
}

/// Apple user info from authorization response (sent with first login only)
#[derive(Debug, Deserialize, Serialize)]
pub struct AppleUserInfo {
    pub name: Option<AppleUserName>,
    pub email: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct AppleUserName {
    #[serde(rename = "firstName")]
    pub first_name: Option<String>,
    #[serde(rename = "lastName")]
    pub last_name: Option<String>,
}

/// Apple Sign In OAuth2/OIDC Provider
pub struct AppleProvider {
    config: OAuth2ProviderConfig,
    http_client: reqwest::Client,
    team_id: String,
    key_id: String,
    private_key: String,
}

impl AppleProvider {
    /// Create a new Apple OAuth2 provider
    ///
    /// # Arguments
    /// * `config` - Provider configuration
    ///
    /// Required extra_params:
    /// - team_id: Apple Developer Team ID
    /// - key_id: Apple Sign In Key ID
    /// - private_key: Apple Sign In Private Key (PEM format)
    pub fn new(config: OAuth2ProviderConfig) -> Result<Self, AuthError> {
        // Set default scopes if none provided
        let mut config = config;
        if config.scopes.is_empty() {
            config.scopes = vec![
                "openid".to_string(),
                "email".to_string(),
                "name".to_string(),
            ];
        }
        
        // Ensure openid scope is included for OIDC
        if !config.scopes.contains(&"openid".to_string()) {
            config.scopes.push("openid".to_string());
        }
        
        // Extract required parameters
        let team_id = config
            .extra_params
            .get("team_id")
            .ok_or_else(|| AuthError::ConfigError(
                "Apple provider requires 'team_id' in extra_params".to_string()
            ))?
            .clone();
        
        let key_id = config
            .extra_params
            .get("key_id")
            .ok_or_else(|| AuthError::ConfigError(
                "Apple provider requires 'key_id' in extra_params".to_string()
            ))?
            .clone();
        
        let private_key = config
            .extra_params
            .get("private_key")
            .ok_or_else(|| AuthError::ConfigError(
                "Apple provider requires 'private_key' in extra_params".to_string()
            ))?
            .clone();
        
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| AuthError::OAuth2Error(format!("Failed to create HTTP client: {}", e)))?;
        
        Ok(Self {
            config,
            http_client,
            team_id,
            key_id,
            private_key,
        })
    }
    
    /// Generate Apple client secret as a signed JWT
    /// Apple requires the client secret to be a JWT signed with the private key
    fn generate_client_secret(&self) -> Result<String, AuthError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| AuthError::SessionError(format!("Failed to get current time: {}", e)))?
            .as_secs();
        
        let claims = AppleClientSecretClaims {
            iss: self.team_id.clone(),
            iat: now,
            exp: now + 15777000, // 6 months (max allowed by Apple)
            aud: APPLE_ISSUER.to_string(),
            sub: self.config.client_id.clone(),
        };
        
        let mut header = Header::new(Algorithm::ES256);
        header.kid = Some(self.key_id.clone());
        
        let encoding_key = EncodingKey::from_ec_pem(self.private_key.as_bytes())
            .map_err(|e| AuthError::JwtError(format!("Failed to parse Apple private key: {}", e)))?;
        
        encode(&header, &claims, &encoding_key)
            .map_err(|e| AuthError::JwtError(format!("Failed to sign client secret: {}", e)))
    }
    
    /// Verify Apple ID token
    async fn verify_id_token(&self, id_token: &str) -> Result<AppleIdTokenClaims, AuthError> {
        // Decode header to get key ID
        let header = decode_header(id_token)
            .map_err(|e| AuthError::JwtError(format!("Failed to decode ID token header: {}", e)))?;
        
        let kid = header.kid.ok_or_else(|| {
            AuthError::JwtError("ID token missing key ID (kid)".to_string())
        })?;
        
        // Fetch Apple's public keys (JWKS)
        let jwks_response = self.http_client
            .get(APPLE_JWKS_URL)
            .send()
            .await
            .map_err(|e| AuthError::OAuth2Error(format!("Failed to fetch JWKS: {}", e)))?
            .json::<serde_json::Value>()
            .await
            .map_err(|e| AuthError::OAuth2Error(format!("Failed to parse JWKS: {}", e)))?;
        
        // Find the matching key
        let keys = jwks_response["keys"].as_array()
            .ok_or_else(|| AuthError::JwtError("Invalid JWKS format".to_string()))?;
        
        let matching_key = keys.iter()
            .find(|k| k["kid"].as_str() == Some(&kid))
            .ok_or_else(|| AuthError::JwtError(format!("Key ID {} not found in JWKS", kid)))?;
        
        // Extract RSA components (Apple uses RS256)
        let n = matching_key["n"].as_str()
            .ok_or_else(|| AuthError::JwtError("Missing 'n' in JWK".to_string()))?;
        let e = matching_key["e"].as_str()
            .ok_or_else(|| AuthError::JwtError("Missing 'e' in JWK".to_string()))?;
        
        // Create decoding key from RSA components
        let decoding_key = DecodingKey::from_rsa_components(n, e)
            .map_err(|e| AuthError::JwtError(format!("Failed to create decoding key: {}", e)))?;
        
        // Set up validation
        let mut validation = Validation::new(Algorithm::RS256);
        validation.set_audience(&[&self.config.client_id]);
        validation.set_issuer(&[APPLE_ISSUER]);
        
        // Decode and validate token
        let token_data = decode::<AppleIdTokenClaims>(id_token, &decoding_key, &validation)
            .map_err(|e| AuthError::JwtError(format!("ID token validation failed: {}", e)))?;
        
        Ok(token_data.claims)
    }
    
    /// Convert ID token claims to OAuth2UserInfo
    fn convert_id_token_claims(&self, claims: AppleIdTokenClaims) -> OAuth2UserInfo {
        let mut raw_data = HashMap::new();
        raw_data.insert("iss".to_string(), serde_json::json!(claims.iss));
        raw_data.insert("iat".to_string(), serde_json::json!(claims.iat));
        raw_data.insert("exp".to_string(), serde_json::json!(claims.exp));
        
        if let Some(is_private) = &claims.is_private_email {
            raw_data.insert("is_private_email".to_string(), serde_json::json!(is_private));
        }
        if let Some(real_user) = claims.real_user_status {
            raw_data.insert("real_user_status".to_string(), serde_json::json!(real_user));
        }
        
        // Apple's email_verified is a string "true" or "false"
        let email_verified = claims.email_verified
            .as_ref()
            .map(|s| s == "true")
            .unwrap_or(false);
        
        OAuth2UserInfo {
            provider_user_id: claims.sub,
            email: claims.email.unwrap_or_default(),
            email_verified,
            name: None, // Name only provided in first login
            given_name: None,
            family_name: None,
            picture: None,
            locale: None,
            raw_data,
        }
    }
}

#[async_trait]
impl OAuth2Provider for AppleProvider {
    fn name(&self) -> &str {
        "apple"
    }
    
    fn authorization_url(&self, state: &str, nonce: Option<&str>) -> Result<String, AuthError> {
        let auth_url = self.config.auth_url.as_deref().unwrap_or(APPLE_AUTH_URL);
        
        let mut url = url::Url::parse(auth_url)
            .map_err(|e| AuthError::ConfigError(format!("Invalid auth URL: {}", e)))?;
        
        {
            let mut query = url.query_pairs_mut();
            query.append_pair("client_id", &self.config.client_id);
            query.append_pair("redirect_uri", &self.config.redirect_uri);
            query.append_pair("response_type", "code");
            query.append_pair("scope", &self.config.scopes.join(" "));
            query.append_pair("state", state);
            query.append_pair("response_mode", "form_post"); // Apple recommends form_post
            
            if let Some(nonce) = nonce {
                query.append_pair("nonce", nonce);
            }
            
            // Add extra parameters (excluding private key related params)
            for (key, value) in &self.config.extra_params {
                if !matches!(key.as_str(), "team_id" | "key_id" | "private_key") {
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
    ) -> Result<OAuth2TokenResponse, AuthError> {
        let token_url = self.config.token_url.as_deref().unwrap_or(APPLE_TOKEN_URL);
        let client_secret = self.generate_client_secret()?;
        
        let token_request = AppleTokenRequest {
            code: code.to_string(),
            client_id: self.config.client_id.clone(),
            client_secret,
            redirect_uri: self.config.redirect_uri.clone(),
            grant_type: "authorization_code".to_string(),
        };
        
        let response = self.http_client
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
        
        let token_response: AppleTokenResponseRaw = response
            .json()
            .await
            .map_err(|e| AuthError::OAuth2Error(format!("Failed to parse token response: {}", e)))?;
        
        Ok(OAuth2TokenResponse {
            access_token: token_response.access_token,
            token_type: token_response.token_type,
            expires_in: token_response.expires_in,
            refresh_token: token_response.refresh_token,
            id_token: token_response.id_token,
            scope: None, // Apple doesn't return scope in response
        })
    }
    
    async fn get_user_info(
        &self,
        _access_token: &str,
        id_token: Option<&str>,
    ) -> Result<OAuth2UserInfo, AuthError> {
        // Apple doesn't provide a UserInfo endpoint
        // All user info must come from the ID token
        let id_token = id_token.ok_or_else(|| {
            AuthError::OAuth2Error("Apple requires ID token for user info".to_string())
        })?;
        
        let claims = self.verify_id_token(id_token).await?;
        Ok(self.convert_id_token_claims(claims))
    }
    
    async fn refresh_token(
        &self,
        refresh_token: &str,
    ) -> Result<OAuth2TokenResponse, AuthError> {
        let token_url = self.config.token_url.as_deref().unwrap_or(APPLE_TOKEN_URL);
        let client_secret = self.generate_client_secret()?;
        
        let refresh_request = AppleRefreshRequest {
            refresh_token: refresh_token.to_string(),
            client_id: self.config.client_id.clone(),
            client_secret,
            grant_type: "refresh_token".to_string(),
        };
        
        let response = self.http_client
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
        
        let token_response: AppleTokenResponseRaw = response
            .json()
            .await
            .map_err(|e| AuthError::OAuth2Error(format!("Failed to parse refresh response: {}", e)))?;
        
        Ok(OAuth2TokenResponse {
            access_token: token_response.access_token,
            token_type: token_response.token_type,
            expires_in: token_response.expires_in,
            refresh_token: Some(refresh_token.to_string()), // Reuse same refresh token
            id_token: token_response.id_token,
            scope: None,
        })
    }
    
    async fn revoke_token(&self, token: &str) -> Result<(), AuthError> {
        let revoke_url = "https://appleid.apple.com/auth/revoke";
        let client_secret = self.generate_client_secret()?;
        
        let revoke_request = AppleRevokeRequest {
            token: token.to_string(),
            client_id: self.config.client_id.clone(),
            client_secret,
            token_type_hint: "refresh_token".to_string(),
        };
        
        let response = self.http_client
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
        let mut extra_params = HashMap::new();
        extra_params.insert("team_id".to_string(), "TEST_TEAM_ID".to_string());
        extra_params.insert("key_id".to_string(), "TEST_KEY_ID".to_string());
        // This is a dummy EC private key for testing only
        extra_params.insert("private_key".to_string(), 
            "-----BEGIN EC PRIVATE KEY-----\nMHcCAQEEIIGlRHjKf+PZYsOJPxXqz4XQYEfG4qY4rA5lqJhSPBKDoAoGCCqGSM49\nAwEHoUQDQgAEeJdqLKWkHUZSQKG7Xw3sL8tMh8HQTGLYKpKFkDLXPF7bQIbzXbLN\nv9LM3I1R7vQXlVh/V8ZvCHW1JzMjLT0+jw==\n-----END EC PRIVATE KEY-----".to_string()
        );
        
        OAuth2ProviderConfig {
            client_id: "com.example.service".to_string(),
            client_secret: "not-used".to_string(), // Apple uses JWT client secret
            scopes: vec!["openid".to_string(), "email".to_string(), "name".to_string()],
            redirect_uri: "https://example.com/auth/callback".to_string(),
            auth_url: None,
            token_url: None,
            userinfo_url: None,
            extra_params,
        }
    }

    #[test]
    fn test_apple_provider_creation() {
        let config = create_test_config();
        let provider = AppleProvider::new(config);
        assert!(provider.is_ok());
    }

    #[test]
    fn test_missing_team_id() {
        let mut config = create_test_config();
        config.extra_params.remove("team_id");
        
        let provider = AppleProvider::new(config);
        assert!(matches!(provider, Err(AuthError::ConfigError(_))));
    }

    #[test]
    fn test_authorization_url_generation() {
        let config = create_test_config();
        let provider = AppleProvider::new(config).unwrap();
        
        let auth_url = provider.authorization_url("test-state", Some("test-nonce")).unwrap();
        
        assert!(auth_url.contains("client_id=com.example.service"));
        assert!(auth_url.contains("state=test-state"));
        assert!(auth_url.contains("nonce=test-nonce"));
        assert!(auth_url.contains("response_type=code"));
        assert!(auth_url.contains("response_mode=form_post"));
    }

    #[test]
    fn test_provider_name() {
        let config = create_test_config();
        let provider = AppleProvider::new(config).unwrap();
        assert_eq!(provider.name(), "apple");
    }
}
