/// OAuth2 Provider implementations
///
/// This module provides a generic OAuth2Provider trait and implementations
/// for major OAuth2 providers (Google, Microsoft, Apple).

use crate::auth::error::AuthError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub mod google;
pub mod microsoft;
pub mod apple;

/// User information returned from OAuth2 providers
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuth2UserInfo {
    /// Unique user identifier from the provider
    pub provider_user_id: String,
    
    /// User's email address
    pub email: String,
    
    /// Whether the email has been verified by the provider
    pub email_verified: bool,
    
    /// User's full name (if available)
    pub name: Option<String>,
    
    /// User's given/first name (if available)
    pub given_name: Option<String>,
    
    /// User's family/last name (if available)
    pub family_name: Option<String>,
    
    /// URL to user's profile picture (if available)
    pub picture: Option<String>,
    
    /// User's locale/language preference (if available)
    pub locale: Option<String>,
    
    /// Additional provider-specific data
    pub raw_data: HashMap<String, serde_json::Value>,
}

/// OAuth2 token response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuth2TokenResponse {
    /// Access token for API requests
    pub access_token: String,
    
    /// Token type (usually "Bearer")
    pub token_type: String,
    
    /// Token expiration time in seconds
    pub expires_in: Option<u64>,
    
    /// Refresh token for obtaining new access tokens
    pub refresh_token: Option<String>,
    
    /// ID token (for OpenID Connect providers)
    pub id_token: Option<String>,
    
    /// OAuth2 scopes granted
    pub scope: Option<String>,
}

/// Configuration for an OAuth2 provider
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuth2ProviderConfig {
    /// Client ID from the provider
    pub client_id: String,
    
    /// Client secret from the provider
    pub client_secret: String,
    
    /// OAuth2 scopes to request
    pub scopes: Vec<String>,
    
    /// Redirect URI (callback URL)
    pub redirect_uri: String,
    
    /// Authorization endpoint URL (if custom)
    pub auth_url: Option<String>,
    
    /// Token endpoint URL (if custom)
    pub token_url: Option<String>,
    
    /// UserInfo endpoint URL (if custom)
    pub userinfo_url: Option<String>,
    
    /// Additional provider-specific configuration
    pub extra_params: HashMap<String, String>,
}

impl OAuth2ProviderConfig {
    /// Validate the provider configuration
    pub fn validate(&self) -> Result<(), AuthError> {
        if self.client_id.is_empty() {
            return Err(AuthError::ConfigError(
                "OAuth2 client_id cannot be empty".to_string(),
            ));
        }
        
        if self.client_secret.is_empty() {
            return Err(AuthError::ConfigError(
                "OAuth2 client_secret cannot be empty".to_string(),
            ));
        }
        
        if self.redirect_uri.is_empty() {
            return Err(AuthError::ConfigError(
                "OAuth2 redirect_uri cannot be empty".to_string(),
            ));
        }
        
        // Validate redirect_uri is a valid URL
        url::Url::parse(&self.redirect_uri)
            .map_err(|e| AuthError::ConfigError(format!("Invalid redirect_uri: {}", e)))?;
        
        Ok(())
    }
}

/// Generic OAuth2 provider trait
///
/// This trait defines the common interface for all OAuth2 providers.
/// Each provider implementation handles the specific details of their
/// OAuth2/OIDC flow.
#[async_trait]
pub trait OAuth2Provider: Send + Sync {
    /// Get the provider name (e.g., "google", "microsoft", "apple")
    fn name(&self) -> &str;
    
    /// Generate the authorization URL for the OAuth2 flow
    ///
    /// # Arguments
    /// * `state` - CSRF state token to include in the authorization request
    /// * `nonce` - Optional nonce for OIDC providers
    ///
    /// # Returns
    /// The authorization URL to redirect the user to
    fn authorization_url(&self, state: &str, nonce: Option<&str>) -> Result<String, AuthError>;
    
    /// Exchange the authorization code for tokens
    ///
    /// # Arguments
    /// * `code` - The authorization code from the provider callback
    /// * `state` - The CSRF state token to validate
    ///
    /// # Returns
    /// The token response containing access token, ID token, etc.
    async fn exchange_code(
        &self,
        code: &str,
        state: &str,
    ) -> Result<OAuth2TokenResponse, AuthError>;
    
    /// Get user information using the access token
    ///
    /// # Arguments
    /// * `access_token` - The access token from the token response
    /// * `id_token` - Optional ID token for OIDC providers
    ///
    /// # Returns
    /// User information from the provider
    async fn get_user_info(
        &self,
        access_token: &str,
        id_token: Option<&str>,
    ) -> Result<OAuth2UserInfo, AuthError>;
    
    /// Refresh an access token using a refresh token
    ///
    /// # Arguments
    /// * `refresh_token` - The refresh token from a previous token response
    ///
    /// # Returns
    /// A new token response with fresh access token
    async fn refresh_token(
        &self,
        refresh_token: &str,
    ) -> Result<OAuth2TokenResponse, AuthError>;
    
    /// Revoke a token (logout)
    ///
    /// # Arguments
    /// * `token` - The token to revoke (access or refresh token)
    ///
    /// # Returns
    /// Ok if revocation succeeded or is not supported
    async fn revoke_token(&self, token: &str) -> Result<(), AuthError>;
}

/// Provider factory for creating OAuth2 providers
pub struct ProviderFactory;

impl ProviderFactory {
    /// Create an OAuth2 provider from configuration
    ///
    /// # Arguments
    /// * `provider_name` - Name of the provider ("google", "microsoft", "apple")
    /// * `config` - Provider configuration
    ///
    /// # Returns
    /// A boxed OAuth2Provider implementation
    pub fn create_provider(
        provider_name: &str,
        config: OAuth2ProviderConfig,
    ) -> Result<Box<dyn OAuth2Provider>, AuthError> {
        // Validate configuration first
        config.validate()?;
        
        match provider_name.to_lowercase().as_str() {
            "google" => Ok(Box::new(google::GoogleProvider::new(config)?)),
            "microsoft" => Ok(Box::new(microsoft::MicrosoftProvider::new(config)?)),
            "apple" => Ok(Box::new(apple::AppleProvider::new(config)?)),
            _ => Err(AuthError::ConfigError(format!(
                "Unknown OAuth2 provider: {}",
                provider_name
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provider_config_validation() {
        let config = OAuth2ProviderConfig {
            client_id: "test-client".to_string(),
            client_secret: "test-secret".to_string(),
            scopes: vec!["email".to_string(), "profile".to_string()],
            redirect_uri: "https://example.com/auth/callback".to_string(),
            auth_url: None,
            token_url: None,
            userinfo_url: None,
            extra_params: HashMap::new(),
        };
        
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_provider_config_validation_empty_client_id() {
        let config = OAuth2ProviderConfig {
            client_id: "".to_string(),
            client_secret: "test-secret".to_string(),
            scopes: vec!["email".to_string()],
            redirect_uri: "https://example.com/callback".to_string(),
            auth_url: None,
            token_url: None,
            userinfo_url: None,
            extra_params: HashMap::new(),
        };
        
        assert!(matches!(config.validate(), Err(AuthError::ConfigError(_))));
    }

    #[test]
    fn test_provider_config_validation_invalid_redirect_uri() {
        let config = OAuth2ProviderConfig {
            client_id: "test-client".to_string(),
            client_secret: "test-secret".to_string(),
            scopes: vec!["email".to_string()],
            redirect_uri: "not-a-url".to_string(),
            auth_url: None,
            token_url: None,
            userinfo_url: None,
            extra_params: HashMap::new(),
        };
        
        assert!(matches!(config.validate(), Err(AuthError::ConfigError(_))));
    }
}
