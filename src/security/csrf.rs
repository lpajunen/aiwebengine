// CSRF (Cross-Site Request Forgery) Protection Module
// Provides HMAC-based token generation and validation for state-changing operations

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use subtle::ConstantTimeEq;
use tokio::sync::RwLock;
use tracing::{debug, warn};

use super::validation::SecurityError;

/// CSRF token with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CsrfToken {
    pub token: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// CSRF protection manager
pub struct CsrfProtection {
    /// HMAC secret key
    secret_key: [u8; 32],
    /// Token lifetime
    token_lifetime: Duration,
    /// Token storage for validation (token -> session_id)
    tokens: Arc<RwLock<HashMap<String, TokenMetadata>>>,
}

#[derive(Debug, Clone)]
struct TokenMetadata {
    session_id: Option<String>,
    created_at: DateTime<Utc>,
    expires_at: DateTime<Utc>,
}

impl CsrfProtection {
    /// Create a new CSRF protection manager
    pub fn new(secret_key: [u8; 32], token_lifetime_seconds: i64) -> Self {
        Self {
            secret_key,
            token_lifetime: Duration::seconds(token_lifetime_seconds),
            tokens: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Generate a CSRF token, optionally tied to a session
    pub async fn generate_token(&self, session_id: Option<String>) -> CsrfToken {
        let now = Utc::now();
        let expires_at = now + self.token_lifetime;
        
        // Generate random data for token
        let random_data: [u8; 16] = rand::random();
        let timestamp = now.timestamp().to_be_bytes();
        
        // Combine session_id (if present), timestamp, and random data
        let mut data = Vec::new();
        if let Some(ref sid) = session_id {
            data.extend_from_slice(sid.as_bytes());
        }
        data.extend_from_slice(&timestamp);
        data.extend_from_slice(&random_data);
        
        // Create HMAC
        let token = self.create_hmac(&data);
        
        // Store token metadata
        let mut tokens = self.tokens.write().await;
        tokens.insert(
            token.clone(),
            TokenMetadata {
                session_id: session_id.clone(),
                created_at: now,
                expires_at,
            },
        );
        
        debug!(
            "Generated CSRF token (session: {:?}, expires: {})",
            session_id, expires_at
        );
        
        CsrfToken {
            token,
            created_at: now,
            expires_at,
        }
    }

    /// Validate a CSRF token, optionally checking session binding
    pub async fn validate_token(
        &self,
        token: &str,
        session_id: Option<&str>,
    ) -> Result<(), SecurityError> {
        // Retrieve token metadata
        let tokens = self.tokens.read().await;
        let metadata = tokens
            .get(token)
            .ok_or(SecurityError::CsrfValidationFailed)?
            .clone();
        drop(tokens);

        // Check expiration
        if Utc::now() > metadata.expires_at {
            warn!("CSRF token expired");
            return Err(SecurityError::CsrfValidationFailed);
        }

        // If token is bound to a session, validate session match
        if let Some(token_session) = &metadata.session_id {
            match session_id {
                Some(provided_session) => {
                    if !self.constant_time_eq(token_session.as_bytes(), provided_session.as_bytes()) {
                        warn!("CSRF token session mismatch");
                        return Err(SecurityError::CsrfValidationFailed);
                    }
                }
                None => {
                    warn!("CSRF token requires session but none provided");
                    return Err(SecurityError::CsrfValidationFailed);
                }
            }
        }

        debug!("CSRF token validated successfully");
        Ok(())
    }

    /// Invalidate a CSRF token after use (for one-time tokens)
    pub async fn invalidate_token(&self, token: &str) -> Result<(), SecurityError> {
        let mut tokens = self.tokens.write().await;
        tokens
            .remove(token)
            .ok_or(SecurityError::CsrfValidationFailed)?;
        
        debug!("CSRF token invalidated");
        Ok(())
    }

    /// Cleanup expired tokens
    pub async fn cleanup_expired_tokens(&self) -> usize {
        let now = Utc::now();
        let mut tokens = self.tokens.write().await;
        
        let initial_count = tokens.len();
        tokens.retain(|_, metadata| now <= metadata.expires_at);
        let removed = initial_count - tokens.len();
        
        if removed > 0 {
            debug!("Cleaned up {} expired CSRF tokens", removed);
        }
        
        removed
    }

    /// Create HMAC signature
    fn create_hmac(&self, data: &[u8]) -> String {
        let mut mac = Sha256::new();
        mac.update(&self.secret_key);
        mac.update(data);
        format!("{:x}", mac.finalize())
    }

    /// Constant-time comparison to prevent timing attacks
    fn constant_time_eq(&self, a: &[u8], b: &[u8]) -> bool {
        if a.len() != b.len() {
            return false;
        }
        a.ct_eq(b).into()
    }
}

/// OAuth state parameter manager (specialized CSRF for OAuth flows)
pub struct OAuthStateManager {
    csrf: CsrfProtection,
}

impl OAuthStateManager {
    pub fn new(secret_key: [u8; 32]) -> Self {
        // OAuth state parameters typically have shorter lifetime (5-10 minutes)
        Self {
            csrf: CsrfProtection::new(secret_key, 600), // 10 minutes
        }
    }

    /// Generate OAuth state parameter
    pub async fn generate_state(&self, session_id: Option<String>) -> String {
        let token = self.csrf.generate_token(session_id).await;
        token.token
    }

    /// Validate OAuth state parameter
    pub async fn validate_state(
        &self,
        state: &str,
        session_id: Option<&str>,
    ) -> Result<(), SecurityError> {
        self.csrf.validate_token(state, session_id).await?;
        // One-time use for OAuth state
        self.csrf.invalidate_token(state).await?;
        Ok(())
    }

    /// Cleanup expired states
    pub async fn cleanup_expired(&self) -> usize {
        self.csrf.cleanup_expired_tokens().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_csrf() -> CsrfProtection {
        let key: [u8; 32] = rand::random();
        CsrfProtection::new(key, 3600)
    }

    #[tokio::test]
    async fn test_generate_and_validate_token() {
        let csrf = create_test_csrf();

        let token = csrf.generate_token(None).await;
        let result = csrf.validate_token(&token.token, None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_token_session_binding() {
        let csrf = create_test_csrf();

        let token = csrf.generate_token(Some("session123".to_string())).await;

        // Correct session - should pass
        let result = csrf.validate_token(&token.token, Some("session123")).await;
        assert!(result.is_ok());

        // Generate new token for different session test
        let token = csrf.generate_token(Some("session123".to_string())).await;

        // Wrong session - should fail
        let result = csrf.validate_token(&token.token, Some("session456")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_token_invalidation() {
        let csrf = create_test_csrf();

        let token = csrf.generate_token(None).await;

        csrf.invalidate_token(&token.token).await.unwrap();

        let result = csrf.validate_token(&token.token, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_oauth_state_one_time_use() {
        let key: [u8; 32] = rand::random();
        let oauth = OAuthStateManager::new(key);

        let state = oauth.generate_state(None).await;

        // First validation should succeed
        let result = oauth.validate_state(&state, None).await;
        assert!(result.is_ok());

        // Second validation should fail (one-time use)
        let result = oauth.validate_state(&state, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_constant_time_comparison() {
        let csrf = create_test_csrf();

        // Same strings should match
        assert!(csrf.constant_time_eq(b"test", b"test"));

        // Different strings should not match
        assert!(!csrf.constant_time_eq(b"test", b"different"));

        // Different lengths should not match
        assert!(!csrf.constant_time_eq(b"test", b"testing"));
    }
}
