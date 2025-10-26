// Authentication Session Management
// High-level wrapper around SecureSessionManager with auth-specific logic

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::security::{SecureSessionManager, SessionData, SessionToken};

use super::error::AuthError;

/// Authentication session (user-facing)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuthSession {
    pub user_id: String,
    pub provider: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub picture: Option<String>,
    pub is_admin: bool,
    pub is_editor: bool,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

impl From<SessionData> for AuthSession {
    fn from(data: SessionData) -> Self {
        Self {
            user_id: data.user_id,
            provider: data.provider,
            email: data.email,
            name: data.name,
            picture: None, // Can be added later
            is_admin: data.is_admin,
            is_editor: data.is_editor,
            created_at: data.created_at,
            expires_at: data.expires_at,
        }
    }
}

/// High-level authentication session manager
pub struct AuthSessionManager {
    session_manager: Arc<SecureSessionManager>,
}

impl AuthSessionManager {
    pub fn new(session_manager: Arc<SecureSessionManager>) -> Self {
        Self { session_manager }
    }

    /// Create a new authenticated session
    pub async fn create_session(
        &self,
        user_id: String,
        provider: String,
        email: Option<String>,
        name: Option<String>,
        is_admin: bool,
        is_editor: bool,
        ip_addr: String,
        user_agent: String,
    ) -> Result<SessionToken, AuthError> {
        // Create session using secure session manager
        let params = crate::security::session::CreateSessionParams {
            user_id,
            provider,
            email,
            name,
            is_admin,
            is_editor,
            ip_addr,
            user_agent,
        };

        let token = self
            .session_manager
            .create_session(params)
            .await
            .map_err(AuthError::Session)?;

        Ok(token)
    }

    /// Validate and retrieve session
    pub async fn get_session(
        &self,
        token: &str,
        ip_addr: &str,
        user_agent: &str,
    ) -> Result<AuthSession, AuthError> {
        let session_data = self
            .session_manager
            .validate_session(token, ip_addr, user_agent)
            .await
            .map_err(AuthError::Session)?;

        Ok(AuthSession::from(session_data))
    }

    /// Delete session (logout)
    pub async fn delete_session(&self, token: &str) -> Result<(), AuthError> {
        self.session_manager
            .invalidate_session(token)
            .await
            .map_err(AuthError::Session)?;

        Ok(())
    }

    /// Get session count for a user
    pub async fn get_user_session_count(&self, user_id: &str) -> usize {
        self.session_manager.get_user_session_count(user_id).await
    }

    /// Check if user has reached concurrent session limit
    pub async fn can_create_session(&self, user_id: &str, max_sessions: usize) -> bool {
        self.get_user_session_count(user_id).await < max_sessions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::SecurityAuditor;

    async fn create_test_manager() -> AuthSessionManager {
        let auditor = Arc::new(SecurityAuditor::new());
        let encryption_key: [u8; 32] = *b"test-encryption-key-32-bytes!!!!";
        let session_manager =
            SecureSessionManager::new(&encryption_key, 3600, 10, Arc::clone(&auditor)).unwrap();
        let session_manager = Arc::new(session_manager);

        AuthSessionManager::new(session_manager)
    }

    #[tokio::test]
    async fn test_create_and_get_session() {
        let manager = create_test_manager().await;

        let token = manager
            .create_session(
                "user123".to_string(),
                "google".to_string(),
                Some("user@example.com".to_string()),
                Some("Test User".to_string()),
                false,
                false,
                "192.168.1.1".to_string(),
                "Mozilla/5.0".to_string(),
            )
            .await
            .unwrap();

        let session = manager
            .get_session(&token.token, "192.168.1.1", "Mozilla/5.0")
            .await
            .unwrap();

        assert_eq!(session.user_id, "user123");
        assert_eq!(session.provider, "google");
        assert_eq!(session.email, Some("user@example.com".to_string()));
        assert!(!session.is_admin);
        assert!(!session.is_editor);
    }

    #[tokio::test]
    async fn test_session_deletion() {
        let manager = create_test_manager().await;

        let token = manager
            .create_session(
                "user123".to_string(),
                "google".to_string(),
                None,
                None,
                false,
                false,
                "192.168.1.1".to_string(),
                "Mozilla/5.0".to_string(),
            )
            .await
            .unwrap();

        // Delete
        manager.delete_session(&token.token).await.unwrap();

        // Should fail to retrieve
        let result = manager
            .get_session(&token.token, "192.168.1.1", "Mozilla/5.0")
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_session_count() {
        let manager = create_test_manager().await;

        let user_id = "user123";

        // Create 2 sessions
        for i in 0..2 {
            manager
                .create_session(
                    user_id.to_string(),
                    "google".to_string(),
                    None,
                    None,
                    false,
                    false,
                    format!("192.168.1.{}", i),
                    "Mozilla/5.0".to_string(),
                )
                .await
                .unwrap();
        }

        let count = manager.get_user_session_count(user_id).await;
        assert_eq!(count, 2);

        // Can create more (limit is 3)
        assert!(manager.can_create_session(user_id, 3).await);
    }

    #[tokio::test]
    async fn test_admin_session() {
        let manager = create_test_manager().await;

        let token = manager
            .create_session(
                "admin123".to_string(),
                "google".to_string(),
                Some("admin@example.com".to_string()),
                Some("Admin User".to_string()),
                true,  // is_admin
                false, // is_editor
                "192.168.1.1".to_string(),
                "Mozilla/5.0".to_string(),
            )
            .await
            .unwrap();

        let session = manager
            .get_session(&token.token, "192.168.1.1", "Mozilla/5.0")
            .await
            .unwrap();

        assert!(session.is_admin);
    }
}
