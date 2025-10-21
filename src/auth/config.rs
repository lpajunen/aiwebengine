// Authentication Configuration
// Manages OAuth2 provider settings, JWT secrets, and session configuration

use serde::{Deserialize, Serialize};
use std::time::Duration;

use super::error::AuthError;

/// Main authentication configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthConfig {
    /// JWT secret for signing tokens (minimum 32 characters)
    pub jwt_secret: String,

    /// Session timeout in seconds (default: 1 hour)
    #[serde(default = "default_session_timeout")]
    pub session_timeout: u64,

    /// Maximum concurrent sessions per user (default: 3)
    #[serde(default = "default_max_sessions")]
    pub max_concurrent_sessions: usize,

    /// Cookie configuration
    #[serde(default)]
    pub cookie: CookieConfig,

    /// OAuth2 provider configurations
    #[serde(default)]
    pub providers: ProvidersConfig,

    /// Enable authentication (can be disabled for testing)
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Bootstrap admin emails - users with these emails automatically get admin role
    /// Use this to set up the first administrator who can then grant roles to others
    #[serde(default)]
    pub bootstrap_admins: Vec<String>,
}

impl AuthConfig {
    /// Validate configuration values
    pub fn validate(&self) -> Result<(), AuthError> {
        // Validate JWT secret length
        if self.jwt_secret.len() < 32 {
            return Err(AuthError::InvalidConfig {
                key: "jwt_secret".to_string(),
                reason: "must be at least 32 characters".to_string(),
            });
        }

        // Validate session timeout
        if self.session_timeout < 60 {
            return Err(AuthError::InvalidConfig {
                key: "session_timeout".to_string(),
                reason: "must be at least 60 seconds".to_string(),
            });
        }

        if self.session_timeout > 86400 * 7 {
            // 7 days
            return Err(AuthError::InvalidConfig {
                key: "session_timeout".to_string(),
                reason: "must not exceed 7 days".to_string(),
            });
        }

        // Validate concurrent sessions
        if self.max_concurrent_sessions == 0 {
            return Err(AuthError::InvalidConfig {
                key: "max_concurrent_sessions".to_string(),
                reason: "must be at least 1".to_string(),
            });
        }

        if self.max_concurrent_sessions > 10 {
            return Err(AuthError::InvalidConfig {
                key: "max_concurrent_sessions".to_string(),
                reason: "should not exceed 10".to_string(),
            });
        }

        // Validate cookie config
        self.cookie.validate()?;

        // Validate provider configs
        self.providers.validate()?;

        Ok(())
    }

    /// Get session timeout as Duration
    pub fn session_duration(&self) -> Duration {
        Duration::from_secs(self.session_timeout)
    }

    /// Get JWT secret as bytes for encryption
    pub fn jwt_secret_bytes(&self) -> [u8; 32] {
        let mut key = [0u8; 32];
        let bytes = self.jwt_secret.as_bytes();
        let len = bytes.len().min(32);
        key[..len].copy_from_slice(&bytes[..len]);
        key
    }
}

impl Default for AuthConfig {
    fn default() -> Self {
        Self {
            jwt_secret: String::new(), // Must be set explicitly
            session_timeout: default_session_timeout(),
            max_concurrent_sessions: default_max_sessions(),
            cookie: CookieConfig::default(),
            providers: ProvidersConfig::default(),
            enabled: true,
            bootstrap_admins: Vec::new(),
        }
    }
}

/// Cookie configuration for session management
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CookieConfig {
    /// Cookie domain (None = current domain)
    pub domain: Option<String>,

    /// Secure flag (HTTPS only) - should be true in production
    #[serde(default = "default_false")]
    pub secure: bool,

    /// HttpOnly flag (prevent JavaScript access)
    #[serde(default = "default_true")]
    pub http_only: bool,

    /// SameSite policy
    #[serde(default = "default_same_site")]
    pub same_site: SameSitePolicy,

    /// Cookie name
    #[serde(default = "default_cookie_name")]
    pub name: String,

    /// Cookie path
    #[serde(default = "default_cookie_path")]
    pub path: String,
}

impl CookieConfig {
    fn validate(&self) -> Result<(), AuthError> {
        if self.name.is_empty() {
            return Err(AuthError::InvalidConfig {
                key: "cookie.name".to_string(),
                reason: "cannot be empty".to_string(),
            });
        }

        Ok(())
    }
}

impl Default for CookieConfig {
    fn default() -> Self {
        Self {
            domain: None,
            secure: false, // Development default
            http_only: true,
            same_site: SameSitePolicy::Lax,
            name: default_cookie_name(),
            path: default_cookie_path(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum SameSitePolicy {
    Strict,
    Lax,
    None,
}

/// OAuth2 providers configuration
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ProvidersConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub google: Option<ProviderConfig>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub microsoft: Option<ProviderConfig>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub apple: Option<ProviderConfig>,
}

impl ProvidersConfig {
    fn validate(&self) -> Result<(), AuthError> {
        if let Some(ref google) = self.google {
            google.validate("google")?;
        }

        if let Some(ref microsoft) = self.microsoft {
            microsoft.validate("microsoft")?;
        }

        if let Some(ref apple) = self.apple {
            apple.validate("apple")?;
        }

        Ok(())
    }

    /// Check if any provider is configured
    pub fn has_any_provider(&self) -> bool {
        self.google.is_some() || self.microsoft.is_some() || self.apple.is_some()
    }

    /// Get list of enabled provider names
    pub fn enabled_providers(&self) -> Vec<&str> {
        let mut providers = Vec::new();
        if self.google.is_some() {
            providers.push("google");
        }
        if self.microsoft.is_some() {
            providers.push("microsoft");
        }
        if self.apple.is_some() {
            providers.push("apple");
        }
        providers
    }
}

/// OAuth2 provider configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// OAuth2 client ID
    pub client_id: String,

    /// OAuth2 client secret
    pub client_secret: String,

    /// OAuth2 redirect URI
    pub redirect_uri: String,

    /// OAuth2 scopes
    #[serde(default)]
    pub scopes: Vec<String>,

    /// Provider-specific: Microsoft tenant ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tenant_id: Option<String>,

    /// Provider-specific: Apple team ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub team_id: Option<String>,

    /// Provider-specific: Apple key ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key_id: Option<String>,

    /// Provider-specific: Apple private key
    #[serde(skip_serializing_if = "Option::is_none")]
    pub private_key: Option<String>,
}

impl ProviderConfig {
    fn validate(&self, provider_name: &str) -> Result<(), AuthError> {
        if self.client_id.is_empty() {
            return Err(AuthError::InvalidConfig {
                key: format!("providers.{}.client_id", provider_name),
                reason: "cannot be empty".to_string(),
            });
        }

        if self.client_secret.is_empty() {
            return Err(AuthError::InvalidConfig {
                key: format!("providers.{}.client_secret", provider_name),
                reason: "cannot be empty".to_string(),
            });
        }

        if self.redirect_uri.is_empty() {
            return Err(AuthError::InvalidConfig {
                key: format!("providers.{}.redirect_uri", provider_name),
                reason: "cannot be empty".to_string(),
            });
        }

        // Validate redirect URI format
        if !self.redirect_uri.starts_with("http://") && !self.redirect_uri.starts_with("https://") {
            return Err(AuthError::InvalidConfig {
                key: format!("providers.{}.redirect_uri", provider_name),
                reason: "must start with http:// or https://".to_string(),
            });
        }

        // Provider-specific validation
        if provider_name == "apple" {
            if self.team_id.is_none() {
                return Err(AuthError::MissingConfig(format!(
                    "providers.{}.team_id",
                    provider_name
                )));
            }
            if self.key_id.is_none() {
                return Err(AuthError::MissingConfig(format!(
                    "providers.{}.key_id",
                    provider_name
                )));
            }
            if self.private_key.is_none() {
                return Err(AuthError::MissingConfig(format!(
                    "providers.{}.private_key",
                    provider_name
                )));
            }
        }

        Ok(())
    }

    /// Get default scopes for a provider
    pub fn default_scopes_for_provider(provider: &str) -> Vec<String> {
        match provider {
            "google" => vec![
                "openid".to_string(),
                "email".to_string(),
                "profile".to_string(),
            ],
            "microsoft" => vec![
                "openid".to_string(),
                "email".to_string(),
                "profile".to_string(),
            ],
            "apple" => vec!["name".to_string(), "email".to_string()],
            _ => vec![],
        }
    }
}

// Default value functions
fn default_session_timeout() -> u64 {
    3600 // 1 hour
}

fn default_max_sessions() -> usize {
    3
}

fn default_true() -> bool {
    true
}

fn default_false() -> bool {
    false
}

fn default_same_site() -> SameSitePolicy {
    SameSitePolicy::Lax
}

fn default_cookie_name() -> String {
    "aiwebengine_session".to_string()
}

fn default_cookie_path() -> String {
    "/".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_valid_config() {
        let config = AuthConfig {
            jwt_secret: "a".repeat(32),
            session_timeout: 3600,
            max_concurrent_sessions: 3,
            cookie: CookieConfig::default(),
            providers: ProvidersConfig::default(),
            enabled: true,
            bootstrap_admins: Vec::new(),
        };

        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_jwt_secret_too_short() {
        let config = AuthConfig {
            jwt_secret: "short".to_string(),
            ..Default::default()
        };

        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            AuthError::InvalidConfig { .. }
        ));
    }

    #[test]
    fn test_session_timeout_validation() {
        let mut config = AuthConfig {
            jwt_secret: "a".repeat(32),
            session_timeout: 30, // Too short
            ..Default::default()
        };

        assert!(config.validate().is_err());

        config.session_timeout = 86400 * 8; // Too long
        assert!(config.validate().is_err());

        config.session_timeout = 3600; // Valid
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_provider_validation() {
        let provider = ProviderConfig {
            client_id: "test-client-id".to_string(),
            client_secret: "test-secret".to_string(),
            redirect_uri: "https://example.com/callback".to_string(),
            scopes: vec!["openid".to_string()],
            tenant_id: None,
            team_id: None,
            key_id: None,
            private_key: None,
        };

        assert!(provider.validate("google").is_ok());
    }

    #[test]
    fn test_apple_provider_validation() {
        let provider = ProviderConfig {
            client_id: "test-client-id".to_string(),
            client_secret: "test-secret".to_string(),
            redirect_uri: "https://example.com/callback".to_string(),
            scopes: vec![],
            tenant_id: None,
            team_id: None,
            key_id: None,
            private_key: None,
        };

        // Should fail without Apple-specific fields
        assert!(provider.validate("apple").is_err());

        let provider = ProviderConfig {
            team_id: Some("TEAM123".to_string()),
            key_id: Some("KEY123".to_string()),
            private_key: Some("-----BEGIN PRIVATE KEY-----".to_string()),
            ..provider
        };

        // Should pass with Apple-specific fields
        assert!(provider.validate("apple").is_ok());
    }

    #[test]
    fn test_enabled_providers() {
        let mut providers = ProvidersConfig::default();
        assert_eq!(providers.enabled_providers(), Vec::<&str>::new());

        providers.google = Some(ProviderConfig {
            client_id: "test".to_string(),
            client_secret: "test".to_string(),
            redirect_uri: "https://example.com/callback".to_string(),
            scopes: vec![],
            tenant_id: None,
            team_id: None,
            key_id: None,
            private_key: None,
        });

        assert_eq!(providers.enabled_providers(), vec!["google"]);
    }

    #[test]
    fn test_jwt_secret_bytes() {
        let config = AuthConfig {
            jwt_secret: "a".repeat(32),
            ..Default::default()
        };

        let bytes = config.jwt_secret_bytes();
        assert_eq!(bytes.len(), 32);
        assert_eq!(bytes[0], b'a');
    }
}
