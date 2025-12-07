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
use sha2::Sha256;
use sqlx::PgPool;

use std::sync::Arc;

use tracing::{debug, info, warn};

/// Session-related errors
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("Session not found")]
    SessionNotFound,

    #[error("Session expired")]
    SessionExpired,

    #[error("Invalid session")]
    InvalidSession,

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
    pub is_editor: bool,
    pub created_at: DateTime<Utc>,
    pub last_access: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub fingerprint: SessionFingerprint,
    /// OAuth refresh token for renewing access tokens
    pub refresh_token: Option<String>,
    /// Target resource URI (for OAuth2 resource indicators)
    pub audience: Option<String>,
}

/// Parameters for creating a new session
#[derive(Debug, Clone)]
pub struct CreateSessionParams {
    pub user_id: String,
    pub provider: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub is_admin: bool,
    pub is_editor: bool,
    pub ip_addr: String,
    pub user_agent: String,
    /// OAuth refresh token (if available)
    pub refresh_token: Option<String>,
    /// Target resource audience
    pub audience: Option<String>,
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
        use sha2::Digest;
        let mut hasher = Sha256::default();
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
#[derive(Clone, Serialize, Deserialize)]
struct EncryptedSessionData {
    ciphertext: Vec<u8>,
    nonce: [u8; 12],
    #[allow(dead_code)]
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
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random_bytes)
    }
}

/// Secure session manager with encryption and comprehensive security controls
pub struct SecureSessionManager {
    /// Database pool for session storage
    pool: PgPool,
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
        pool: PgPool,
        encryption_key: &[u8; 32],
        session_timeout_seconds: i64,
        max_concurrent_sessions: usize,
        auditor: Arc<SecurityAuditor>,
    ) -> Result<Self, SessionError> {
        let cipher = Aes256Gcm::new(encryption_key.into());

        Ok(Self {
            pool,
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
        params: CreateSessionParams,
    ) -> Result<SessionToken, SessionError> {
        let user_id = params.user_id.clone();

        // Check concurrent session limits
        let active_sessions_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sessions WHERE user_id = $1 AND expires_at > NOW()",
        )
        .bind(&user_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| SessionError::ValidationFailed(format!("Database error: {}", e)))?;

        if active_sessions_count >= self.max_concurrent_sessions as i64 {
            // Remove oldest session
            sqlx::query(
                "DELETE FROM sessions WHERE id IN (
                     SELECT id FROM sessions 
                     WHERE user_id = $1 
                     ORDER BY created_at ASC 
                     LIMIT 1
                 )",
            )
            .bind(&user_id)
            .execute(&self.pool)
            .await
            .map_err(|e| SessionError::ValidationFailed(format!("Database error: {}", e)))?;

            warn!(
                "Removed oldest session for user {} due to concurrent session limit",
                user_id
            );
        }

        // Generate session token
        let token = SessionToken::generate();
        let now = Utc::now();
        let expires_at = now + self.session_timeout;

        // Create session data
        let session_data = SessionData {
            session_id: token.clone(),
            user_id: params.user_id.clone(),
            provider: params.provider.clone(),
            email: params.email.clone(),
            name: params.name.clone(),
            is_admin: params.is_admin,
            is_editor: params.is_editor,
            created_at: now,
            last_access: now,
            expires_at,
            fingerprint: SessionFingerprint::new(
                params.ip_addr.clone(),
                &params.user_agent,
                self.strict_ip_validation,
            ),
            refresh_token: params.refresh_token.clone(),
            audience: params.audience.clone(),
        };

        // Encrypt session data
        let encrypted = self.encrypt_session(&session_data)?;
        let encrypted_json = serde_json::to_value(&encrypted)
            .map_err(|e| SessionError::EncryptionError(e.to_string()))?;

        // Store in database
        sqlx::query(
            "INSERT INTO sessions (session_id, user_id, data, created_at, expires_at, last_accessed_at) 
             VALUES ($1, $2, $3, $4, $5, $6)"
        )
        .bind(&token)
        .bind(&user_id)
        .bind(encrypted_json)
        .bind(now)
        .bind(expires_at)
        .bind(now)
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::ValidationFailed(format!("Database error: {}", e)))?;

        // Audit log
        self.auditor
            .log_event(
                SecurityEvent::new(
                    SecurityEventType::AuthenticationSuccess,
                    SecuritySeverity::Low,
                    Some(params.user_id.clone()),
                )
                .with_detail("provider", &params.provider)
                .with_detail("ip_address", &params.ip_addr),
            )
            .await;

        info!(
            "Created session for user {} (provider: {})",
            params.user_id, params.provider
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
        let row: (serde_json::Value, DateTime<Utc>) =
            sqlx::query_as("SELECT data, expires_at FROM sessions WHERE session_id = $1")
                .bind(token)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| SessionError::ValidationFailed(format!("Database error: {}", e)))?
                .ok_or(SessionError::SessionNotFound)?;

        let (encrypted_json, expires_at) = row;

        // Check expiration
        if expires_at < Utc::now() {
            self.invalidate_session(token).await?;

            self.auditor
                .log_event(
                    SecurityEvent::new(
                        SecurityEventType::AuthenticationFailure,
                        SecuritySeverity::Low,
                        None,
                    )
                    .with_error("Session expired".to_string()),
                )
                .await;

            return Err(SessionError::SessionExpired);
        }

        let encrypted: EncryptedSessionData = serde_json::from_value(encrypted_json)
            .map_err(|e| SessionError::ValidationFailed(format!("Data corruption: {}", e)))?;

        // Decrypt session data
        let mut session_data = self.decrypt_session(&encrypted)?;

        // Validate fingerprint
        if !session_data.fingerprint.validate(ip_addr, user_agent) {
            self.auditor
                .log_event(
                    SecurityEvent::new(
                        SecurityEventType::SuspiciousActivity,
                        SecuritySeverity::High,
                        Some(session_data.user_id.clone()),
                    )
                    .with_detail("reason", "Session fingerprint mismatch")
                    .with_detail("ip_addr", ip_addr)
                    .with_detail("expected_ip", &session_data.fingerprint.ip_addr),
                )
                .await;

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
        let encrypted_json = serde_json::to_value(&encrypted)
            .map_err(|e| SessionError::EncryptionError(e.to_string()))?;

        sqlx::query(
            "UPDATE sessions SET data = $1, last_accessed_at = NOW() WHERE session_id = $2",
        )
        .bind(encrypted_json)
        .bind(token)
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::ValidationFailed(format!("Database error: {}", e)))?;

        debug!("Validated session for user {}", session_data.user_id);

        Ok(session_data)
    }

    /// Validate session with resource indicator check (RFC 8707)
    ///
    /// # Arguments
    /// * `token` - Session token to validate
    /// * `ip_addr` - Client IP address
    /// * `user_agent` - Client user agent
    /// * `resource` - Optional resource indicator to validate against session audience
    ///
    /// # Returns
    /// Session data if valid and authorized for the requested resource
    pub async fn validate_session_with_resource(
        &self,
        token: &str,
        ip_addr: &str,
        user_agent: &str,
        resource: Option<&str>,
    ) -> Result<SessionData, SessionError> {
        // First validate the session normally
        let session_data = self.validate_session(token, ip_addr, user_agent).await?;

        // If a resource is requested, validate audience claim
        if let Some(requested_resource) = resource {
            match &session_data.audience {
                Some(audience) if audience == requested_resource => {
                    // Audience matches, allow access
                    debug!(
                        "Session audience validated for resource: {}",
                        requested_resource
                    );
                }
                Some(audience) => {
                    // Audience mismatch
                    warn!(
                        "Session audience mismatch: expected {}, got {}",
                        requested_resource, audience
                    );

                    self.auditor
                        .log_event(
                            SecurityEvent::new(
                                SecurityEventType::AuthorizationFailure,
                                SecuritySeverity::Medium,
                                Some(session_data.user_id.clone()),
                            )
                            .with_detail("reason", "Audience mismatch")
                            .with_detail("requested_resource", requested_resource)
                            .with_detail("session_audience", audience),
                        )
                        .await;

                    return Err(SessionError::InvalidSession);
                }
                None => {
                    // Session has no audience claim - allow for backward compatibility
                    // but log a warning for MCP endpoints
                    if requested_resource.starts_with("/mcp") {
                        warn!(
                            "Session without audience claim used for MCP endpoint: {}",
                            requested_resource
                        );
                    }
                }
            }
        }

        Ok(session_data)
    }

    /// Invalidate a session (logout)
    pub async fn invalidate_session(&self, token: &str) -> Result<(), SessionError> {
        self.invalidate_session_internal(token).await
    }

    async fn invalidate_session_internal(&self, token: &str) -> Result<(), SessionError> {
        // Get user_id before deleting for logging
        let user_id: Option<String> =
            sqlx::query_scalar("SELECT user_id FROM sessions WHERE session_id = $1")
                .bind(token)
                .fetch_optional(&self.pool)
                .await
                .unwrap_or(None);

        // Remove session
        let result = sqlx::query("DELETE FROM sessions WHERE session_id = $1")
            .bind(token)
            .execute(&self.pool)
            .await
            .map_err(|e| SessionError::ValidationFailed(format!("Database error: {}", e)))?;

        if result.rows_affected() == 0 {
            return Err(SessionError::SessionNotFound);
        }

        if let Some(uid) = user_id {
            self.auditor
                .log_event(
                    SecurityEvent::new(
                        SecurityEventType::SystemSecurityEvent,
                        SecuritySeverity::Low,
                        Some(uid.clone()),
                    )
                    .with_action("logout".to_string()),
                )
                .await;

            info!("Invalidated session for user {}", uid);
        }

        Ok(())
    }

    /// Cleanup expired sessions
    pub async fn cleanup_expired_sessions(&self) -> usize {
        let result = sqlx::query("DELETE FROM sessions WHERE expires_at < NOW()")
            .execute(&self.pool)
            .await;

        match result {
            Ok(res) => {
                let count = res.rows_affected() as usize;
                if count > 0 {
                    info!("Cleaned up {} expired sessions", count);
                }
                count
            }
            Err(e) => {
                warn!("Failed to cleanup expired sessions: {}", e);
                0
            }
        }
    }

    /// Get active session count for a user
    pub async fn get_user_session_count(&self, user_id: &str) -> usize {
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sessions WHERE user_id = $1 AND expires_at > NOW()",
        )
        .bind(user_id)
        .fetch_one(&self.pool)
        .await
        .unwrap_or(0);

        count as usize
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
        let nonce = Nonce::from(nonce_bytes);

        // Encrypt
        let ciphertext = self
            .cipher
            .encrypt(&nonce, plaintext.as_ref())
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
        let nonce = Nonce::from(encrypted.nonce);

        // Decrypt
        let plaintext = self
            .cipher
            .decrypt(&nonce, encrypted.ciphertext.as_ref())
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
        let pool = sqlx::PgPool::connect_lazy(
            "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine",
        )
        .unwrap();
        Arc::new(SecurityAuditor::new(pool))
    }

    fn create_test_manager() -> SecureSessionManager {
        let key: [u8; 32] = rand::random();
        let pool = sqlx::PgPool::connect_lazy(
            "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine",
        )
        .unwrap();
        SecureSessionManager::new(pool, &key, 3600, 3, create_test_auditor()).unwrap()
    }

    #[tokio::test]
    async fn test_create_and_validate_session() {
        let manager = create_test_manager();

        let params = CreateSessionParams {
            user_id: "user123".to_string(),
            provider: "google".to_string(),
            email: Some("user@example.com".to_string()),
            name: Some("Test User".to_string()),
            is_admin: false,
            is_editor: false,
            ip_addr: "192.168.1.1".to_string(),
            user_agent: "Mozilla/5.0".to_string(),
            refresh_token: None,
            audience: None,
        };

        let token = manager.create_session(params).await.unwrap();

        let session = manager
            .validate_session(&token.token, "192.168.1.1", "Mozilla/5.0")
            .await
            .unwrap();

        assert_eq!(session.user_id, "user123");
        assert_eq!(session.provider, "google");
        assert!(!session.is_admin);
        assert!(!session.is_editor);
    }

    #[tokio::test]
    async fn test_session_fingerprint_validation() {
        let manager = create_test_manager();

        let params = CreateSessionParams {
            user_id: "user123".to_string(),
            provider: "google".to_string(),
            email: None,
            name: None,
            is_admin: false,
            is_editor: false,
            ip_addr: "192.168.1.1".to_string(),
            user_agent: "Mozilla/5.0".to_string(),
            refresh_token: None,
            audience: None,
        };

        let token = manager.create_session(params).await.unwrap();

        // Different user agent should fail
        let result = manager
            .validate_session(&token.token, "192.168.1.1", "Chrome/90.0")
            .await;
        assert!(matches!(result, Err(SessionError::FingerprintMismatch)));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_concurrent_session_limit() {
        // Add timeout to prevent hanging
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            let manager = create_test_manager();
            let user_id = "user_concurrent";

            // Clean up any existing sessions for this user to ensure clean state
            let pool = sqlx::PgPool::connect_lazy(
                "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine",
            )
            .unwrap();
            let _ = sqlx::query("DELETE FROM sessions WHERE user_id = $1")
                .bind(user_id)
                .execute(&pool)
                .await;

            // Create 4 sessions (limit is 3)
            for _i in 0..4 {
                let params = CreateSessionParams {
                    user_id: user_id.to_string(),
                    provider: "google".to_string(),
                    email: None,
                    name: None,
                    is_admin: false,
                    is_editor: false,
                    ip_addr: "192.168.1.1".to_string(),
                    user_agent: "Mozilla/5.0".to_string(),
                    refresh_token: None,
                    audience: None,
                };
                manager.create_session(params).await.unwrap();
            }

            let count = manager.get_user_session_count(user_id).await;
            assert_eq!(count, 3); // Should be limited to 3
        })
        .await;

        assert!(
            result.is_ok(),
            "Test timed out - possible deadlock in session manager"
        );
    }

    #[tokio::test]
    async fn test_session_invalidation() {
        let manager = create_test_manager();

        let params = CreateSessionParams {
            user_id: "user123".to_string(),
            provider: "google".to_string(),
            email: None,
            name: None,
            is_admin: false,
            is_editor: false,
            ip_addr: "192.168.1.1".to_string(),
            user_agent: "Mozilla/5.0".to_string(),
            refresh_token: None,
            audience: None,
        };

        let token = manager.create_session(params).await.unwrap();

        // Validate session exists
        manager
            .validate_session(&token.token, "192.168.1.1", "Mozilla/5.0")
            .await
            .unwrap();

        // Invalidate session
        manager.invalidate_session(&token.token).await.unwrap();

        // Should fail after invalidation
        let result = manager
            .validate_session(&token.token, "192.168.1.1", "Mozilla/5.0")
            .await;
        assert!(matches!(result, Err(SessionError::SessionNotFound)));
    }
}
