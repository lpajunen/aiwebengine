// CSRF (Cross-Site Request Forgery) Protection Module
// Provides HMAC-based stateless token generation and validation for state-changing operations
// Tokens are self-contained with timestamp and HMAC signature - no server-side storage needed

use base64::Engine;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tracing::{debug, warn};

use super::validation::SecurityError;

/// CSRF token with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CsrfToken {
    pub token: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// CSRF protection manager with stateless tokens
/// Token format: base64(timestamp:session_id:random):hmac
/// This allows load-balanced deployments without shared state
pub struct CsrfProtection {
    /// HMAC secret key
    secret_key: [u8; 32],
    /// Token lifetime
    token_lifetime: Duration,
}

impl CsrfProtection {
    /// Create a new CSRF protection manager
    pub fn new(secret_key: [u8; 32], token_lifetime_seconds: i64) -> Self {
        Self {
            secret_key,
            token_lifetime: Duration::seconds(token_lifetime_seconds),
        }
    }

    /// Generate a stateless CSRF token, optionally tied to a session
    /// Token format: base64(timestamp:session_id:random):hmac
    pub async fn generate_token(&self, session_id: Option<String>) -> CsrfToken {
        let now = Utc::now();
        let expires_at = now + self.token_lifetime;

        // Generate random bytes for uniqueness
        let random_data: [u8; 16] = rand::random();
        let random_b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random_data);

        // Build token payload: timestamp:session_id:random
        let session_part = session_id.as_deref().unwrap_or("");
        let payload = format!("{}:{}:{}", now.timestamp(), session_part, random_b64);

        // Create HMAC signature over payload
        let signature = self.create_hmac(payload.as_bytes());

        // Final token: base64(payload):signature
        let payload_b64 =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload.as_bytes());
        let token = format!("{}:{}", payload_b64, signature);

        debug!(
            "Generated stateless CSRF token (session: {:?}, expires: {})",
            session_id, expires_at
        );

        CsrfToken {
            token,
            created_at: now,
            expires_at,
        }
    }

    /// Validate a stateless CSRF token, optionally checking session binding
    /// Extracts timestamp and session from token payload and validates HMAC
    pub async fn validate_token(
        &self,
        token: &str,
        session_id: Option<&str>,
    ) -> Result<(), SecurityError> {
        // Parse token format: base64(payload):signature
        let parts: Vec<&str> = token.split(':').collect();
        if parts.len() != 2 {
            warn!("Invalid CSRF token format");
            return Err(SecurityError::CsrfValidationFailed);
        }

        let payload_b64 = parts[0];
        let provided_signature = parts[1];

        // Decode payload
        let payload_bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload_b64)
            .map_err(|_| {
                warn!("Failed to decode CSRF token payload");
                SecurityError::CsrfValidationFailed
            })?;

        let payload = String::from_utf8(payload_bytes).map_err(|_| {
            warn!("Invalid UTF-8 in CSRF token payload");
            SecurityError::CsrfValidationFailed
        })?;

        // Verify HMAC signature
        let expected_signature = self.create_hmac(payload.as_bytes());
        if !self.constant_time_eq(expected_signature.as_bytes(), provided_signature.as_bytes()) {
            warn!("CSRF token HMAC verification failed");
            return Err(SecurityError::CsrfValidationFailed);
        }

        // Parse payload: timestamp:session_id:random
        let payload_parts: Vec<&str> = payload.split(':').collect();
        if payload_parts.len() != 3 {
            warn!("Invalid CSRF token payload structure");
            return Err(SecurityError::CsrfValidationFailed);
        }

        let timestamp_str = payload_parts[0];
        let token_session = payload_parts[1];

        // Check expiration
        let timestamp = timestamp_str.parse::<i64>().map_err(|_| {
            warn!("Invalid timestamp in CSRF token");
            SecurityError::CsrfValidationFailed
        })?;

        let token_time = DateTime::from_timestamp(timestamp, 0).ok_or_else(|| {
            warn!("Invalid timestamp value in CSRF token");
            SecurityError::CsrfValidationFailed
        })?;

        let expires_at = token_time + self.token_lifetime;
        if Utc::now() > expires_at {
            warn!("CSRF token expired");
            return Err(SecurityError::CsrfValidationFailed);
        }

        // Validate session binding if present
        if !token_session.is_empty() {
            match session_id {
                Some(provided_session) => {
                    if !self.constant_time_eq(token_session.as_bytes(), provided_session.as_bytes())
                    {
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

    /// Validate a token that must have been issued *to* a particular principal.
    ///
    /// [`Self::validate_token`] accepts an unbound token against any session,
    /// which is right for the sign-in forms: they are submitted before there is
    /// a session to bind one to. It is wrong wherever a form acts on a session
    /// that already exists. An unbound token is one anybody can obtain by
    /// asking the engine for a page — including from their own server, with no
    /// browser and no account — so accepting one there means the token proves
    /// nothing about who submitted the form, and the only thing left standing
    /// between the form and a cross-site POST is the cookie's `SameSite=Lax`.
    /// Which mostly holds, except that browsers permit a cross-site POST to
    /// carry a Lax cookie for the first couple of minutes after it is set.
    pub async fn validate_token_for(
        &self,
        token: &str,
        session_id: &str,
    ) -> Result<(), SecurityError> {
        self.validate_token(token, Some(session_id)).await?;

        // The signature is verified by the call above, so the payload can be
        // read: what is left to check is that a binding is present at all.
        if Self::token_session(token).is_none_or(|session| session.is_empty()) {
            warn!("CSRF token carries no session binding where one is required");
            return Err(SecurityError::CsrfValidationFailed);
        }

        Ok(())
    }

    /// The session a token was bound to, from its already-verified payload.
    fn token_session(token: &str) -> Option<String> {
        let payload_b64 = token.split(':').next()?;
        let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload_b64)
            .ok()?;
        let payload = String::from_utf8(payload).ok()?;
        payload.split(':').nth(1).map(str::to_string)
    }

    /// Invalidate a CSRF token after use (for one-time tokens)
    /// Note: With stateless tokens, this is a no-op but kept for API compatibility
    /// For one-time use, consider using OAuthStateManager which tracks used tokens
    pub async fn invalidate_token(&self, _token: &str) -> Result<(), SecurityError> {
        // Stateless tokens cannot be invalidated server-side
        // This is a design trade-off: stateless = load-balancer friendly but not truly one-time
        // For critical one-time operations, use a separate mechanism with state storage
        debug!("CSRF token invalidation requested (no-op for stateless tokens)");
        Ok(())
    }

    /// Cleanup expired tokens
    /// Note: With stateless tokens, there's nothing to clean up
    pub async fn cleanup_expired_tokens(&self) -> usize {
        // No server-side storage, so nothing to clean
        0
    }

    /// Create HMAC signature
    fn create_hmac(&self, data: &[u8]) -> String {
        let mut mac = Sha256::new();
        mac.update(self.secret_key);
        mac.update(data);
        hex::encode(mac.finalize())
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
    /// Note: With stateless tokens, one-time use is not enforced server-side
    /// The state still provides CSRF protection via HMAC and timestamp validation
    pub async fn validate_state(
        &self,
        state: &str,
        session_id: Option<&str>,
    ) -> Result<(), SecurityError> {
        self.csrf.validate_token(state, session_id).await?;
        // Note: Cannot invalidate stateless tokens for one-time use
        // This is acceptable for OAuth as the state is provider-generated and short-lived
        Ok(())
    }

    /// Cleanup expired states
    /// Note: With stateless tokens, there's nothing to clean up
    pub async fn cleanup_expired(&self) -> usize {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_csrf() -> CsrfProtection {
        let key: [u8; 32] = rand::random();
        CsrfProtection::new(key, 3600)
    }

    /// An unbound token is one anybody can fetch from the sign-in page, so a
    /// form acting on an existing session must not accept one. This is the
    /// difference between the consent form proving who submitted it and
    /// leaning entirely on `SameSite=Lax`.
    #[tokio::test]
    async fn a_form_that_requires_binding_refuses_an_unbound_token() {
        let csrf = CsrfProtection::new([7u8; 32], 3600);

        let unbound = csrf.generate_token(None).await;
        assert!(
            csrf.validate_token(&unbound.token, Some("user-1"))
                .await
                .is_ok(),
            "the lenient check accepts it, which is why the strict one exists"
        );
        assert!(
            csrf.validate_token_for(&unbound.token, "user-1")
                .await
                .is_err(),
            "a token nobody was issued must not stand in for this user's approval"
        );

        let bound = csrf.generate_token(Some("user-1".to_string())).await;
        assert!(
            csrf.validate_token_for(&bound.token, "user-1")
                .await
                .is_ok()
        );
        assert!(
            csrf.validate_token_for(&bound.token, "user-2")
                .await
                .is_err(),
            "one person's token is not another's"
        );
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

        // Invalidation is a no-op for stateless tokens but shouldn't error
        csrf.invalidate_token(&token.token).await.unwrap();

        // Token is still valid (stateless design trade-off)
        let result = csrf.validate_token(&token.token, None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_oauth_state_reuse() {
        let key: [u8; 32] = rand::random();
        let oauth = OAuthStateManager::new(key);

        let state = oauth.generate_state(None).await;

        // First validation should succeed
        let result = oauth.validate_state(&state, None).await;
        assert!(result.is_ok());

        // Second validation also succeeds (stateless tokens can be reused within lifetime)
        // This is a trade-off for load-balancer compatibility
        let result = oauth.validate_state(&state, None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_token_expiration() {
        // Create CSRF with very short lifetime (1 second)
        let key: [u8; 32] = rand::random();
        let csrf = CsrfProtection::new(key, 1);

        let token = csrf.generate_token(None).await;

        // Immediately should be valid
        assert!(csrf.validate_token(&token.token, None).await.is_ok());

        // Wait for expiration
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

        // Should now be expired
        assert!(csrf.validate_token(&token.token, None).await.is_err());
    }

    #[tokio::test]
    async fn test_stateless_token_structure() {
        let csrf = create_test_csrf();
        let token = csrf.generate_token(Some("session123".to_string())).await;

        // Token should contain base64 payload and signature separated by colon
        assert!(token.token.contains(':'));
        let parts: Vec<&str> = token.token.split(':').collect();
        assert_eq!(parts.len(), 2);

        // Should be able to decode the payload
        let payload_b64 = parts[0];
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(payload_b64)
            .unwrap();
        let payload = String::from_utf8(decoded).unwrap();

        // Payload should contain timestamp, session, and random data
        assert!(payload.contains("session123"));
        let payload_parts: Vec<&str> = payload.split(':').collect();
        assert_eq!(payload_parts.len(), 3);
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
