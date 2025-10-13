// Secure Session Management Module
// Provides encrypted session storage with fingerprinting, concurrent session limits,
// and comprehensive security controls for authentication

use super::audit::{SecurityAuditor, SecurityEvent, SecurityEventType, SecuritySeverity};
use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit},
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

/// Session-related errors
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("Session not found")]
    SessionNotFound,

    #[error("Session expired")]
    SessionExpired,

    #[error("Session validation failed: {0}")]
    ValidationFailed(String),

    #[error("Maximum concurrent sessions exceeded")]
    MaxSessionsExceeded,

    #[error("Session fingerprint mismatch - possible hijacking attempt")]
    FingerprintMismatch,

    #[error("Encryption error: {0}")]
    EncryptionError(String),

    #[error("Decryption error: {0}")]
    DecryptionError(String),

    #[error("Invalid session token")]
    InvalidToken,
}

/// Session data stored for each authenticated user
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionData {
    pub session_id: String,
    pub user_id: String,
    pub provider: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub is_admin: bool,
    pub created_at: DateTime<Utc>,
    pub last_access: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub fingerprint: SessionFingerprint,
}

/// Session fingerprint for detecting hijacking attempts
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionFingerprint {
    pub ip_addr: String,
    pub user_agent_hash: String,
    /// Allow for IP changes (mobile networks, VPN switches)
    pub strict_ip_validation: bool,
}

impl SessionFingerprint {
    pub fn new(ip_addr: String, user_agent: &str, strict_ip: bool) -> Self {
        let user_agent_hash = Self::hash_user_agent(user_agent);
        Self {
            ip_addr,
            user_agent_hash,
            strict_ip_validation: strict_ip,
        }
    }

    fn hash_user_agent(user_agent: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(user_agent.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Validate fingerprint with tolerance for IP changes
    pub fn validate(&self, ip_addr: &str, user_agent: &str) -> bool {
        let user_agent_hash = Self::hash_user_agent(user_agent);

        // User-Agent must always match (browsers don't change UA mid-session)
        if user_agent_hash != self.user_agent_hash {
            return false;
        }

        // IP validation depends on strict mode
        if self.strict_ip_validation {
            ip_addr == self.ip_addr
        } else {
            // In non-strict mode, allow IP changes but log them
            true
        }
    }
}

/// Encrypted session storage
#[derive(Clone)]
struct EncryptedSessionData {
    ciphertext: Vec<u8>,
    nonce: [u8; 12],
    created_at: DateTime<Utc>,
}

/// Session token that's given to the client
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionToken {
    pub token: String,
    pub expires_at: DateTime<Utc>,
}

impl SessionToken {
    fn generate() -> String {
        let random_bytes: [u8; 32] = rand::random();
        use base64::Engine;
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&random_bytes)
    }
}

/// Secure session manager with encryption and comprehensive security controls
pub struct SecureSessionManager {
    /// Encrypted session storage
    sessions: Arc<RwLock<HashMap<String, EncryptedSessionData>>>,
    /// User session index for concurrent session tracking
    user_sessions: Arc<RwLock<HashMap<String, Vec<String>>>>,
    /// Encryption cipher
    cipher: Aes256Gcm,
    /// Maximum concurrent sessions per user
    max_concurrent_sessions: usize,
    /// Session timeout duration
    session_timeout: Duration,
    /// Security auditor for logging
    auditor: Arc<SecurityAuditor>,
    /// Strict IP validation (false for mobile-friendly)
    strict_ip_validation: bool,
}

impl SecureSessionManager {
    /// Create a new secure session manager
    pub fn new(
        encryption_key: &[u8; 32],
        session_timeout_seconds: i64,
        max_concurrent_sessions: usize,
        auditor: Arc<SecurityAuditor>,
    ) -> Result<Self, SessionError> {
        let cipher = Aes256Gcm::new(encryption_key.into());

        Ok(Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            user_sessions: Arc::new(RwLock::new(HashMap::new())),
            cipher,
            max_concurrent_sessions,
            session_timeout: Duration::seconds(session_timeout_seconds),
            auditor,
            strict_ip_validation: false, // Mobile-friendly by default
        })
    }

    /// Create a new session for an authenticated user
    pub async fn create_session(
        &self,
        user_id: String,
        provider: String,
        email: Option<String>,
        name: Option<String>,
        is_admin: bool,
        ip_addr: String,
        user_agent: String,
    ) -> Result<SessionToken, SessionError> {
        // Check concurrent session limits
        let mut user_sessions = self.user_sessions.write().await;
        let existing_sessions = user_sessions
            .entry(user_id.clone())
            .or_insert_with(Vec::new);

        if existing_sessions.len() >= self.max_concurrent_sessions {
            // Remove oldest session
            if let Some(oldest_token) = existing_sessions.first().cloned() {
                self.invalidate_session_internal(&oldest_token).await?;
                existing_sessions.remove(0);

                warn!(
                    "Removed oldest session for user {} due to concurrent session limit",
                    user_id
                );
            }
        }

        // Generate session token
        let token = SessionToken::generate();
        let now = Utc::now();
        let expires_at = now + self.session_timeout;

        // Create session data
        let session_data = SessionData {
            session_id: token.clone(),
            user_id: user_id.clone(),
            provider: provider.clone(),
            email: email.clone(),
            name: name.clone(),
            is_admin,
            created_at: now,
            last_access: now,
            expires_at,
            fingerprint: SessionFingerprint::new(
                ip_addr.clone(),
                &user_agent,
                self.strict_ip_validation,
            ),
        };

        // Encrypt session data
        let encrypted = self.encrypt_session(&session_data)?;

        // Store encrypted session
        let mut sessions = self.sessions.write().await;
        sessions.insert(token.clone(), encrypted);

        // Track session for user
        existing_sessions.push(token.clone());

        // Audit log
        self.auditor.log_event(
            SecurityEvent::new(
                SecurityEventType::AuthenticationSuccess,
                SecuritySeverity::Low,
                Some(user_id.clone()),
            )
            .with_detail("provider", &provider)
            .with_detail("ip_address", &ip_addr),
        );

        info!(
            "Created session for user {} (provider: {})",
            user_id, provider
        );

        Ok(SessionToken { token, expires_at })
    }

    /// Validate and retrieve session data
    pub async fn validate_session(
        &self,
        token: &str,
        ip_addr: &str,
        user_agent: &str,
    ) -> Result<SessionData, SessionError> {
        // Retrieve encrypted session
        let sessions = self.sessions.read().await;
        let encrypted = sessions
            .get(token)
            .ok_or(SessionError::SessionNotFound)?
            .clone();
        drop(sessions);

        // Decrypt session data
        let mut session_data = self.decrypt_session(&encrypted)?;

        // Check expiration
        if Utc::now() > session_data.expires_at {
            self.invalidate_session(token).await?;

            self.auditor.log_event(
                SecurityEvent::new(
                    SecurityEventType::AuthenticationFailure,
                    SecuritySeverity::Low,
                    Some(session_data.user_id.clone()),
                )
                .with_error("Session expired".to_string()),
            );

            return Err(SessionError::SessionExpired);
        }

        // Validate fingerprint
        if !session_data.fingerprint.validate(ip_addr, user_agent) {
            self.auditor.log_event(
                SecurityEvent::new(
                    SecurityEventType::SuspiciousActivity,
                    SecuritySeverity::High,
                    Some(session_data.user_id.clone()),
                )
                .with_detail("reason", "Session fingerprint mismatch")
                .with_detail("ip_addr", ip_addr)
                .with_detail("expected_ip", &session_data.fingerprint.ip_addr),
            );

            // Don't invalidate session if IP changed but UA matches (mobile networks)
            if !self.strict_ip_validation && ip_addr != session_data.fingerprint.ip_addr {
                warn!(
                    "IP changed for user {} session (old: {}, new: {})",
                    session_data.user_id, session_data.fingerprint.ip_addr, ip_addr
                );

                // Update fingerprint with new IP
                session_data.fingerprint.ip_addr = ip_addr.to_string();
            } else {
                // Strict mode or UA mismatch - reject
                return Err(SessionError::FingerprintMismatch);
            }
        }

        // Update last access time
        session_data.last_access = Utc::now();

        // Re-encrypt and update session
        let encrypted = self.encrypt_session(&session_data)?;
        let mut sessions = self.sessions.write().await;
        sessions.insert(token.to_string(), encrypted);

        debug!("Validated session for user {}", session_data.user_id);

        Ok(session_data)
    }

    /// Invalidate a session (logout)
    pub async fn invalidate_session(&self, token: &str) -> Result<(), SessionError> {
        self.invalidate_session_internal(token).await
    }

    async fn invalidate_session_internal(&self, token: &str) -> Result<(), SessionError> {
        // Remove session
        let mut sessions = self.sessions.write().await;
        let encrypted = sessions
            .remove(token)
            .ok_or(SessionError::SessionNotFound)?;
        drop(sessions);

        // Decrypt to get user_id for cleanup
        if let Ok(session_data) = self.decrypt_session(&encrypted) {
            // Remove from user sessions index
            let mut user_sessions = self.user_sessions.write().await;
            if let Some(user_tokens) = user_sessions.get_mut(&session_data.user_id) {
                user_tokens.retain(|t| t != token);
                if user_tokens.is_empty() {
                    user_sessions.remove(&session_data.user_id);
                }
            }

            self.auditor.log_event(
                SecurityEvent::new(
                    SecurityEventType::SystemSecurityEvent,
                    SecuritySeverity::Low,
                    Some(session_data.user_id.clone()),
                )
                .with_action("logout".to_string()),
            );

            info!("Invalidated session for user {}", session_data.user_id);
        }

        Ok(())
    }

    /// Cleanup expired sessions
    pub async fn cleanup_expired_sessions(&self) -> usize {
        let now = Utc::now();
        let mut sessions = self.sessions.write().await;
        let mut expired_tokens = Vec::new();

        // Find expired sessions
        for (token, encrypted) in sessions.iter() {
            if let Ok(session_data) = self.decrypt_session(encrypted) {
                if now > session_data.expires_at {
                    expired_tokens.push(token.clone());
                }
            }
        }

        // Remove expired sessions
        let count = expired_tokens.len();
        for token in expired_tokens {
            sessions.remove(&token);
        }

        drop(sessions);

        if count > 0 {
            info!("Cleaned up {} expired sessions", count);
        }

        count
    }

    /// Get active session count for a user
    pub async fn get_user_session_count(&self, user_id: &str) -> usize {
        let user_sessions = self.user_sessions.read().await;
        user_sessions.get(user_id).map(|v| v.len()).unwrap_or(0)
    }

    /// Encrypt session data
    fn encrypt_session(
        &self,
        session_data: &SessionData,
    ) -> Result<EncryptedSessionData, SessionError> {
        // Serialize session data
        let plaintext = serde_json::to_vec(session_data)
            .map_err(|e| SessionError::EncryptionError(format!("Serialization failed: {}", e)))?;

        // Generate random nonce
        let nonce_bytes: [u8; 12] = rand::random();
        let nonce = Nonce::from_slice(&nonce_bytes);

        // Encrypt
        let ciphertext = self
            .cipher
            .encrypt(nonce, plaintext.as_ref())
            .map_err(|e| SessionError::EncryptionError(format!("Encryption failed: {}", e)))?;

        Ok(EncryptedSessionData {
            ciphertext,
            nonce: nonce_bytes,
            created_at: Utc::now(),
        })
    }

    /// Decrypt session data
    fn decrypt_session(
        &self,
        encrypted: &EncryptedSessionData,
    ) -> Result<SessionData, SessionError> {
        let nonce = Nonce::from_slice(&encrypted.nonce);

        // Decrypt
        let plaintext = self
            .cipher
            .decrypt(nonce, encrypted.ciphertext.as_ref())
            .map_err(|e| SessionError::DecryptionError(format!("Decryption failed: {}", e)))?;

        // Deserialize
        let session_data: SessionData = serde_json::from_slice(&plaintext)
            .map_err(|e| SessionError::DecryptionError(format!("Deserialization failed: {}", e)))?;

        Ok(session_data)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_auditor() -> Arc<SecurityAuditor> {
        Arc::new(SecurityAuditor::new())
    }

    fn create_test_manager() -> SecureSessionManager {
        let key: [u8; 32] = rand::random();
        SecureSessionManager::new(&key, 3600, 3, create_test_auditor()).unwrap()
    }

    #[tokio::test]
    async fn test_create_and_validate_session() {
        let manager = create_test_manager();

        let token = manager
            .create_session(
                "user123".to_string(),
                "google".to_string(),
                Some("user@example.com".to_string()),
                Some("Test User".to_string()),
                false,
                "192.168.1.1".to_string(),
                "Mozilla/5.0".to_string(),
            )
            .await
            .unwrap();

        let session = manager
            .validate_session(&token.token, "192.168.1.1", "Mozilla/5.0")
            .await
            .unwrap();

        assert_eq!(session.user_id, "user123");
        assert_eq!(session.provider, "google");
        assert!(!session.is_admin);
    }

    #[tokio::test]
    async fn test_session_fingerprint_validation() {
        let manager = create_test_manager();

        let token = manager
            .create_session(
                "user123".to_string(),
                "google".to_string(),
                None,
                None,
                false,
                "192.168.1.1".to_string(),
                "Mozilla/5.0".to_string(),
            )
            .await
            .unwrap();

        // Different user agent should fail
        let result = manager
            .validate_session(&token.token, "192.168.1.1", "Chrome/90.0")
            .await;
        assert!(matches!(result, Err(SessionError::FingerprintMismatch)));
    }

    #[tokio::test]
    async fn test_concurrent_session_limit() {
        let manager = create_test_manager();

        // Create 4 sessions (limit is 3)
        for _i in 0..4 {
            manager
                .create_session(
                    "user123".to_string(),
                    "google".to_string(),
                    None,
                    None,
                    false,
                    "192.168.1.1".to_string(),
                    "Mozilla/5.0".to_string(),
                )
                .await
                .unwrap();
        }

        let count = manager.get_user_session_count("user123").await;
        assert_eq!(count, 3); // Should be limited to 3
    }

    #[tokio::test]
    async fn test_session_invalidation() {
        let manager = create_test_manager();

        let token = manager
            .create_session(
                "user123".to_string(),
                "google".to_string(),
                None,
                None,
                false,
                "192.168.1.1".to_string(),
                "Mozilla/5.0".to_string(),
            )
            .await
            .unwrap();

        manager.invalidate_session(&token.token).await.unwrap();

        let result = manager
            .validate_session(&token.token, "192.168.1.1", "Mozilla/5.0")
            .await;
        assert!(matches!(result, Err(SessionError::SessionNotFound)));
    }
}
