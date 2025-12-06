/// Authentication Manager
///
/// Central orchestrator for authentication operations, coordinating providers,
/// sessions, and security infrastructure.
use crate::auth::{
    AuthError, AuthSecurityContext, AuthSessionManager, OAuth2Provider, OAuth2ProviderConfig,
    OAuth2TokenResponse, OAuth2UserInfo, ProviderFactory,
};
use std::collections::HashMap;
use std::sync::Arc;

#[cfg(test)]
use crate::security::ThreatDetectionConfig;

/// User information after successful authentication
#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    /// Unique user identifier (from provider)
    pub user_id: String,

    /// OAuth2 provider name
    pub provider: String,

    /// User information from provider
    pub user_info: OAuth2UserInfo,

    /// OAuth2 tokens
    pub tokens: OAuth2TokenResponse,
}

/// Authentication manager configuration
#[derive(Debug, Clone)]
pub struct AuthManagerConfig {
    /// Base URL for redirect URIs
    pub base_url: String,

    /// Session cookie name
    pub session_cookie_name: String,

    /// Session cookie domain
    pub cookie_domain: Option<String>,

    /// Session cookie secure flag
    pub cookie_secure: bool,

    /// Session cookie http-only flag
    pub cookie_http_only: bool,

    /// Session cookie same-site policy
    pub cookie_same_site: CookieSameSite,

    /// Session timeout in seconds
    pub session_timeout: u64,
}

#[derive(Debug, Clone)]
pub enum CookieSameSite {
    Strict,
    Lax,
    None,
}

impl Default for AuthManagerConfig {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:3000".to_string(),
            session_cookie_name: "auth_session".to_string(),
            cookie_domain: None,
            cookie_secure: true,
            cookie_http_only: true,
            cookie_same_site: CookieSameSite::Lax,
            session_timeout: 3600 * 24 * 7, // 7 days
        }
    }
}

/// Central authentication manager
pub struct AuthManager {
    config: AuthManagerConfig,
    providers: HashMap<String, Arc<Box<dyn OAuth2Provider>>>,
    session_manager: Arc<AuthSessionManager>,
    security_context: Arc<AuthSecurityContext>,
}

impl AuthManager {
    /// Create a new authentication manager
    pub fn new(
        config: AuthManagerConfig,
        session_manager: Arc<AuthSessionManager>,
        security_context: Arc<AuthSecurityContext>,
    ) -> Self {
        Self {
            config,
            providers: HashMap::new(),
            session_manager,
            security_context,
        }
    }

    /// Register an OAuth2 provider
    pub fn register_provider(
        &mut self,
        provider_name: &str,
        provider_config: OAuth2ProviderConfig,
    ) -> Result<(), AuthError> {
        let provider = ProviderFactory::create_provider(provider_name, provider_config)?;
        self.providers
            .insert(provider_name.to_string(), Arc::new(provider));
        Ok(())
    }

    /// Get a registered provider
    pub fn get_provider(&self, provider_name: &str) -> Option<Arc<Box<dyn OAuth2Provider>>> {
        self.providers.get(provider_name).cloned()
    }

    /// List all registered providers
    pub fn list_providers(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }

    /// Generate OAuth2 authorization URL for a provider
    ///
    /// # Arguments
    /// * `provider_name` - Name of the OAuth2 provider
    /// * `ip_addr` - Client IP address for CSRF state tracking
    ///
    /// # Returns
    /// Tuple of (authorization_url, csrf_state_token)
    pub async fn start_login(
        &self,
        provider_name: &str,
        ip_addr: &str,
    ) -> Result<(String, String), AuthError> {
        let provider = self
            .get_provider(provider_name)
            .ok_or_else(|| AuthError::UnsupportedProvider(provider_name.to_string()))?;

        // Generate CSRF state token
        let state = self
            .security_context
            .create_oauth_state(provider_name, ip_addr)
            .await?;

        // Generate nonce for OIDC providers
        let nonce = format!("nonce_{}", uuid::Uuid::new_v4());

        // Generate authorization URL (no PKCE for now - will be added when needed)
        let auth_url = provider.authorization_url(&state, Some(&nonce), None, None)?;

        // Log authentication attempt
        self.security_context
            .log_auth_attempt(provider_name, ip_addr)
            .await;

        Ok((auth_url, state))
    }

    /// Generate OAuth2 authorization URL with redirect URL
    ///
    /// # Arguments
    /// * `provider_name` - Name of the OAuth2 provider
    /// * `ip_addr` - Client IP address for CSRF state tracking
    /// * `redirect_url` - URL to redirect to after successful authentication
    ///
    /// # Returns
    /// Tuple of (authorization_url, csrf_state_token)
    pub async fn start_login_with_redirect(
        &self,
        provider_name: &str,
        ip_addr: &str,
        redirect_url: String,
    ) -> Result<(String, String), AuthError> {
        let provider = self
            .get_provider(provider_name)
            .ok_or_else(|| AuthError::UnsupportedProvider(provider_name.to_string()))?;

        // Generate CSRF state token with redirect URL
        let state = self
            .security_context
            .create_oauth_state_with_redirect(provider_name, ip_addr, redirect_url)
            .await?;

        // Generate nonce for OIDC providers
        let nonce = format!("nonce_{}", uuid::Uuid::new_v4());

        // Generate authorization URL (no PKCE for now - will be added when needed)
        let auth_url = provider.authorization_url(&state, Some(&nonce), None, None)?;

        // Log authentication attempt
        self.security_context
            .log_auth_attempt(provider_name, ip_addr)
            .await;

        Ok((auth_url, state))
    }

    /// Handle OAuth2 callback and complete authentication
    ///
    /// # Arguments
    /// * `provider_name` - Name of the OAuth2 provider
    /// * `code` - Authorization code from provider
    /// * `state` - CSRF state token to validate
    /// * `ip_addr` - Client IP address
    /// * `user_agent` - Client user agent string
    ///
    /// # Returns
    /// Session token for the authenticated user
    pub async fn handle_callback(
        &self,
        provider_name: &str,
        code: &str,
        state: &str,
        ip_addr: &str,
        user_agent: &str,
    ) -> Result<String, AuthError> {
        // Validate CSRF state
        if !self
            .security_context
            .validate_oauth_state(state, provider_name, ip_addr)
            .await?
        {
            self.security_context
                .log_auth_failure(provider_name, "Invalid OAuth state", Some(ip_addr))
                .await;
            return Err(AuthError::InvalidState);
        }

        // Get provider
        let provider = self
            .get_provider(provider_name)
            .ok_or_else(|| AuthError::UnsupportedProvider(provider_name.to_string()))?;

        // Check rate limiting
        if !self.security_context.check_auth_rate_limit(ip_addr).await {
            return Err(AuthError::RateLimitExceeded);
        }

        // Exchange code for tokens (no PKCE verifier for now - will be added when needed)
        let tokens = provider
            .exchange_code(code, state, None, None)
            .await
            .map_err(|e| {
                // Log failure (spawn to avoid blocking)
                let security_context = self.security_context.clone();
                let provider_name = provider_name.to_string();
                let error_msg = format!("Token exchange failed: {}", e);
                let ip = ip_addr.to_string();
                tokio::spawn(async move {
                    let _ = security_context
                        .log_auth_failure(&provider_name, &error_msg, Some(&ip))
                        .await;
                });
                e
            })?;

        // Get user info
        let user_info = provider
            .get_user_info(&tokens.access_token, tokens.id_token.as_deref())
            .await
            .map_err(|e| {
                // Log failure (spawn to avoid blocking)
                let security_context = self.security_context.clone();
                let provider_name = provider_name.to_string();
                let error_msg = format!("User info retrieval failed: {}", e);
                let ip = ip_addr.to_string();
                tokio::spawn(async move {
                    let _ = security_context
                        .log_auth_failure(&provider_name, &error_msg, Some(&ip))
                        .await;
                });
                e
            })?;

        // Verify email if required
        if !user_info.email_verified {
            self.security_context
                .log_auth_failure(provider_name, "Email not verified", Some(ip_addr))
                .await;
            return Err(AuthError::ProviderError(
                "Email not verified by provider".to_string(),
            ));
        }

        // Upsert user in repository (this handles bootstrap admin assignment)
        let user_id = crate::user_repository::upsert_user(
            user_info.email.clone(),
            user_info.name.clone(),
            provider_name.to_string(),
            user_info.provider_user_id.clone(),
        )
        .map_err(|e| {
            tracing::error!("Failed to upsert user: {}", e);
            AuthError::Internal(format!("Failed to create/update user: {}", e))
        })?;

        // Get user from repository to check roles
        let user = crate::user_repository::get_user(&user_id).map_err(|e| {
            tracing::error!("User not found after upsert: {}", e);
            AuthError::Internal("User not found after creation".to_string())
        })?;

        // Check if user has Administrator role
        let is_admin = user
            .roles
            .contains(&crate::user_repository::UserRole::Administrator);

        // Check if user has Editor role
        let is_editor = user
            .roles
            .contains(&crate::user_repository::UserRole::Editor);

        // Create session with correct admin and editor status
        let session_token = self
            .session_manager
            .create_session(crate::auth::session::CreateAuthSessionParams {
                user_id: user_id.clone(),
                provider: provider_name.to_string(),
                email: Some(user_info.email.clone()),
                name: user_info.name.clone(),
                is_admin,
                is_editor,
                ip_addr: ip_addr.to_string(),
                user_agent: user_agent.to_string(),
                refresh_token: tokens.refresh_token.clone(),
                audience: None, // Will be set for MCP endpoints
            })
            .await?;

        // Log successful authentication
        self.security_context
            .log_auth_success(&user_id, provider_name, Some(ip_addr))
            .await;

        Ok(session_token.token)
    }

    /// Validate session and return user ID
    ///
    /// # Arguments
    /// * `session_token` - Session token to validate
    /// * `ip_addr` - Client IP address
    /// * `user_agent` - Client user agent string
    ///
    /// # Returns
    /// User ID if session is valid
    pub async fn validate_session(
        &self,
        session_token: &str,
        ip_addr: &str,
        user_agent: &str,
    ) -> Result<String, AuthError> {
        self.session_manager
            .get_session(session_token, ip_addr, user_agent)
            .await
            .map(|session| session.user_id)
    }

    /// Get full session information
    ///
    /// # Arguments
    /// * `session_token` - Session token to validate
    /// * `ip_addr` - Client IP address
    /// * `user_agent` - Client user agent string
    ///
    /// # Returns
    /// Complete AuthSession if valid
    pub async fn get_session(
        &self,
        session_token: &str,
        ip_addr: &str,
        user_agent: &str,
    ) -> Result<crate::auth::session::AuthSession, AuthError> {
        self.session_manager
            .get_session(session_token, ip_addr, user_agent)
            .await
    }

    /// Validate session with resource indicator check (RFC 8707)
    ///
    /// # Arguments
    /// * `session_token` - Session token to validate
    /// * `ip_addr` - Client IP address
    /// * `user_agent` - Client user agent string
    /// * `resource` - Optional resource indicator (e.g., "/mcp/tools")
    ///
    /// # Returns
    /// Complete AuthSession if valid and authorized for resource
    pub async fn validate_session_with_resource(
        &self,
        session_token: &str,
        ip_addr: &str,
        user_agent: &str,
        resource: Option<&str>,
    ) -> Result<crate::auth::session::AuthSession, AuthError> {
        self.session_manager
            .validate_session_with_resource(session_token, ip_addr, user_agent, resource)
            .await
    }

    /// Refresh an OAuth2 access token
    ///
    /// # Arguments
    /// * `provider_name` - Name of the OAuth2 provider
    /// * `refresh_token` - Refresh token from previous authentication
    ///
    /// # Returns
    /// New token response
    pub async fn refresh_token(
        &self,
        provider_name: &str,
        refresh_token: &str,
    ) -> Result<OAuth2TokenResponse, AuthError> {
        let provider = self
            .get_provider(provider_name)
            .ok_or_else(|| AuthError::UnsupportedProvider(provider_name.to_string()))?;

        provider.refresh_token(refresh_token).await
    }

    /// Logout a user session
    ///
    /// # Arguments
    /// * `session_token` - Session token to invalidate
    /// * `revoke_oauth_token` - Whether to revoke OAuth tokens with provider
    ///
    /// # Returns
    /// Ok if logout succeeded
    pub async fn logout(
        &self,
        session_token: &str,
        revoke_oauth_token: bool,
    ) -> Result<(), AuthError> {
        // Destroy session
        self.session_manager.delete_session(session_token).await?;

        // Optionally revoke OAuth tokens
        if revoke_oauth_token {
            // Note: Would need to store OAuth tokens in session to revoke them
            // This is a simplified version
            // In production, you'd want to:
            // 1. Store access/refresh tokens in encrypted session data
            // 2. Retrieve them here
            // 3. Call provider.revoke_token()
        }

        Ok(())
    }

    /// Get authentication manager configuration
    pub fn config(&self) -> &AuthManagerConfig {
        &self.config
    }

    /// Get session manager
    pub fn session_manager(&self) -> Arc<AuthSessionManager> {
        Arc::clone(&self.session_manager)
    }

    /// Get security context
    pub fn security_context(&self) -> Arc<AuthSecurityContext> {
        Arc::clone(&self.security_context)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::OAuth2ProviderConfig;
    use crate::security::{
        CsrfProtection, DataEncryption, RateLimiter, SecureSessionManager, SecurityAuditor,
        ThreatDetector,
    };
    use std::collections::HashMap;

    async fn create_test_manager() -> AuthManager {
        let config = AuthManagerConfig::default();

        // Create security infrastructure
        let auditor = Arc::new(SecurityAuditor::new());
        let rate_limiter = Arc::new(RateLimiter::new().with_security_auditor(Arc::clone(&auditor)));
        let threat_config = ThreatDetectionConfig::default();
        let _threat_detector = Arc::new(ThreatDetector::new(threat_config));
        let csrf_key: [u8; 32] = *b"test-csrf-secret-key-32-bytes!!!";
        let csrf = Arc::new(CsrfProtection::new(csrf_key, 3600));
        let encryption_key: [u8; 32] = *b"test-encryption-key-32-bytes!!!!";
        let encryption = Arc::new(DataEncryption::new(&encryption_key));

        let session_mgr =
            SecureSessionManager::new(&encryption_key, 3600, 10, Arc::clone(&auditor)).unwrap();
        let session_mgr = Arc::new(session_mgr);

        let auth_session_mgr = Arc::new(AuthSessionManager::new(session_mgr));

        let security_context = Arc::new(AuthSecurityContext::new(
            Arc::clone(&auditor),
            rate_limiter,
            csrf,
            encryption,
        ));

        AuthManager::new(config, auth_session_mgr, security_context)
    }

    #[tokio::test]
    async fn test_manager_creation() {
        let manager = create_test_manager().await;
        assert_eq!(manager.list_providers().len(), 0);
    }

    #[tokio::test]
    async fn test_register_provider() {
        let mut manager = create_test_manager().await;

        let config = OAuth2ProviderConfig {
            client_id: "test-client".to_string(),
            client_secret: "test-secret".to_string(),
            scopes: vec!["openid".to_string(), "email".to_string()],
            redirect_uri: "https://example.com/callback".to_string(),
            auth_url: None,
            token_url: None,
            userinfo_url: None,
            extra_params: HashMap::new(),
        };

        let result = manager.register_provider("google", config);
        assert!(result.is_ok());
        assert_eq!(manager.list_providers().len(), 1);
        assert!(manager.get_provider("google").is_some());
    }

    #[tokio::test]
    async fn test_unsupported_provider() {
        let manager = create_test_manager().await;
        let result = manager.start_login("nonexistent", "127.0.0.1").await;
        assert!(matches!(result, Err(AuthError::UnsupportedProvider(_))));
    }
}
