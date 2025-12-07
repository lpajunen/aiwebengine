//! Session and Authentication Security Tests
//!
//! This module contains all tests related to session management and authentication security:
//! - Session lifecycle management
//! - Session fingerprinting and validation
//! - IP change tolerance
//! - Concurrent session limits
//! - Session encryption and integrity
//! - CSRF token generation and validation
//! - OAuth state management
//! - Full authentication flow simulation
//! - Concurrent user isolation

// Integration tests for Phase 0.5 security modules
// Tests session management, CSRF protection, and data encryption

use aiwebengine::security::{
    CreateSessionParams, CsrfProtection, DataEncryption, FieldEncryptor, OAuthStateManager,
    SecureSessionManager, SecurityAuditor, SessionError,
};
use std::sync::Arc;

// ============================================================================
// Session Management Tests
// ============================================================================

#[tokio::test]
async fn test_session_lifecycle() {
    let key: [u8; 32] = rand::random();
    let pool = sqlx::PgPool::connect_lazy(
        "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine",
    )
    .unwrap();
    let auditor = Arc::new(SecurityAuditor::new(pool.clone()));
    let manager = SecureSessionManager::new(pool, &key, 3600, 3, auditor).unwrap();

    // Create session
    let params = CreateSessionParams {
        user_id: "user123".to_string(),
        provider: "google".to_string(),
        email: Some("user@example.com".to_string()),
        name: Some("Test User".to_string()),
        is_admin: false,
        is_editor: false,
        ip_addr: "192.168.1.1".to_string(),
        user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64)".to_string(),
        refresh_token: None,
        audience: None,
    };
    let token = manager.create_session(params).await.unwrap();

    // Validate session
    let session = manager
        .validate_session(
            &token.token,
            "192.168.1.1",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64)",
        )
        .await
        .unwrap();

    assert_eq!(session.user_id, "user123");
    assert_eq!(session.provider, "google");
    assert_eq!(session.email, Some("user@example.com".to_string()));

    // Invalidate session
    manager.invalidate_session(&token.token).await.unwrap();

    // Validation should fail after invalidation
    let result = manager
        .validate_session(
            &token.token,
            "192.168.1.1",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64)",
        )
        .await;

    assert!(matches!(result, Err(SessionError::SessionNotFound)));
}

#[tokio::test]
async fn test_session_fingerprint_user_agent_mismatch() {
    let key: [u8; 32] = rand::random();
    let pool = sqlx::PgPool::connect_lazy(
        "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine",
    )
    .unwrap();
    let auditor = Arc::new(SecurityAuditor::new(pool.clone()));
    let manager = SecureSessionManager::new(pool, &key, 3600, 3, auditor).unwrap();

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

#[tokio::test]
async fn test_session_ip_change_tolerance() {
    let key: [u8; 32] = rand::random();
    let pool = sqlx::PgPool::connect_lazy(
        "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine",
    )
    .unwrap();
    let auditor = Arc::new(SecurityAuditor::new(pool.clone()));
    let manager = SecureSessionManager::new(pool, &key, 3600, 3, auditor).unwrap();

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

    // IP change with same user agent should succeed (mobile-friendly)
    let result = manager
        .validate_session(&token.token, "192.168.1.2", "Mozilla/5.0")
        .await;

    assert!(result.is_ok());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_concurrent_session_limit() {
    // Add timeout to prevent hanging
    let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        let key: [u8; 32] = rand::random();
        let pool = sqlx::PgPool::connect_lazy(
            "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine",
        )
        .unwrap();
        let auditor = Arc::new(SecurityAuditor::new(pool.clone()));
        let manager = SecureSessionManager::new(pool, &key, 3600, 3, auditor).unwrap();
        let unique_user_id = format!("user_concurrent_{}", rand::random::<u32>());

        // Create 5 sessions (limit is 3)
        for i in 0..5 {
            let params = CreateSessionParams {
                user_id: unique_user_id.clone(),
                provider: "google".to_string(),
                email: None,
                name: None,
                is_admin: false,
                is_editor: false,
                ip_addr: format!("192.168.1.{}", i),
                user_agent: "Mozilla/5.0".to_string(),
                refresh_token: None,
                audience: None,
            };
            manager.create_session(params).await.unwrap();
        }

        // Should only have 3 sessions
        let count = manager.get_user_session_count(&unique_user_id).await;
        assert_eq!(count, 3);
    })
    .await;

    assert!(
        result.is_ok(),
        "Test timed out - possible deadlock in session manager"
    );
}

#[tokio::test]
async fn test_session_encryption() {
    let key: [u8; 32] = rand::random();
    let pool = sqlx::PgPool::connect_lazy(
        "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine",
    )
    .unwrap();
    let auditor = Arc::new(SecurityAuditor::new(pool.clone()));
    let manager = SecureSessionManager::new(pool, &key, 3600, 3, auditor).unwrap();

    // Create session with sensitive data
    let params = CreateSessionParams {
        user_id: "user123".to_string(),
        provider: "google".to_string(),
        email: Some("admin@example.com".to_string()),
        name: Some("Admin User".to_string()),
        is_admin: true,
        is_editor: false,
        ip_addr: "192.168.1.1".to_string(),
        refresh_token: None,
        audience: None,
        user_agent: "Mozilla/5.0".to_string(),
    };
    let token = manager.create_session(params).await.unwrap();

    // Validate and check data integrity
    let session = manager
        .validate_session(&token.token, "192.168.1.1", "Mozilla/5.0")
        .await
        .unwrap();

    assert_eq!(session.user_id, "user123");
    assert_eq!(session.email, Some("admin@example.com".to_string()));
    assert!(session.is_admin);
}

// ============================================================================
// CSRF Protection Tests
// ============================================================================

#[tokio::test]
async fn test_csrf_token_generation_and_validation() {
    let key: [u8; 32] = rand::random();
    let csrf = CsrfProtection::new(key, 3600);

    let token = csrf.generate_token(None).await;

    let result = csrf.validate_token(&token.token, None).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_csrf_token_session_binding() {
    let key: [u8; 32] = rand::random();
    let csrf = CsrfProtection::new(key, 3600);

    // Generate token bound to session
    let token = csrf.generate_token(Some("session123".to_string())).await;

    // Validate with correct session
    let result = csrf.validate_token(&token.token, Some("session123")).await;
    assert!(result.is_ok());

    // Generate new token for wrong session test
    let token = csrf.generate_token(Some("session123".to_string())).await;

    // Validate with wrong session
    let result = csrf
        .validate_token(&token.token, Some("wrong_session"))
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_csrf_token_stateless_validation() {
    let key: [u8; 32] = rand::random();
    let csrf = CsrfProtection::new(key, 3600);

    let token = csrf.generate_token(None).await;

    // First validation
    csrf.validate_token(&token.token, None).await.unwrap();

    // Invalidate is a no-op for stateless tokens
    csrf.invalidate_token(&token.token).await.unwrap();

    // Token is still valid (stateless design trade-off for load balancer support)
    let result = csrf.validate_token(&token.token, None).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_oauth_state_stateless_validation() {
    let key: [u8; 32] = rand::random();
    let oauth = OAuthStateManager::new(key);

    let state = oauth.generate_state(None).await;

    // First validation should succeed
    let result = oauth.validate_state(&state, None).await;
    assert!(result.is_ok());

    // Second validation also succeeds (stateless tokens can be reused within lifetime)
    // This is a trade-off for load balancer compatibility
    // The state still provides CSRF protection via HMAC and is short-lived
    let result = oauth.validate_state(&state, None).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_oauth_state_with_session_binding() {
    let key: [u8; 32] = rand::random();
    let oauth = OAuthStateManager::new(key);

    let state = oauth.generate_state(Some("session_abc".to_string())).await;

    // Correct session should work
    let result = oauth.validate_state(&state, Some("session_abc")).await;
    assert!(result.is_ok());

    // Generate new state for mismatch test
    let state = oauth.generate_state(Some("session_abc".to_string())).await;

    // Wrong session should fail
    let result = oauth.validate_state(&state, Some("session_xyz")).await;
    assert!(result.is_err());
}

// ============================================================================
// Data Encryption Tests
// ============================================================================

#[test]
fn test_field_encryption_decryption() {
    let key: [u8; 32] = rand::random();
    let encryption = DataEncryption::new(&key);

    let plaintext = "sensitive_oauth_token_12345";
    let encrypted = encryption.encrypt_field(plaintext).unwrap();
    let decrypted = encryption.decrypt_field(&encrypted).unwrap();

    assert_eq!(plaintext, decrypted);
}

#[test]
fn test_encryption_different_nonces() {
    let key: [u8; 32] = rand::random();
    let encryption = DataEncryption::new(&key);

    let plaintext = "same_data";
    let encrypted1 = encryption.encrypt_field(plaintext).unwrap();
    let encrypted2 = encryption.encrypt_field(plaintext).unwrap();

    // Different nonces should produce different ciphertexts
    assert_ne!(encrypted1.ciphertext, encrypted2.ciphertext);
    assert_ne!(encrypted1.nonce, encrypted2.nonce);

    // Both should decrypt to same plaintext
    assert_eq!(encryption.decrypt_field(&encrypted1).unwrap(), plaintext);
    assert_eq!(encryption.decrypt_field(&encrypted2).unwrap(), plaintext);
}

#[test]
fn test_binary_data_encryption() {
    let key: [u8; 32] = rand::random();
    let encryption = DataEncryption::new(&key);

    let binary_data = b"binary_secret_data_with_nulls\x00\x01\x02";
    let encrypted = encryption.encrypt_bytes(binary_data).unwrap();
    let decrypted = encryption.decrypt_bytes(&encrypted).unwrap();

    assert_eq!(binary_data, decrypted.as_slice());
}

#[test]
fn test_encryption_version_check() {
    let key: [u8; 32] = rand::random();
    let encryption = DataEncryption::new(&key);

    let plaintext = "test";
    let mut encrypted = encryption.encrypt_field(plaintext).unwrap();

    // Tamper with version
    encrypted.version = 99;

    let result = encryption.decrypt_field(&encrypted);
    assert!(result.is_err());
}

#[test]
fn test_field_encryptor_oauth_tokens() {
    let key: [u8; 32] = rand::random();
    let encryption = Arc::new(DataEncryption::new(&key));
    let encryptor = FieldEncryptor::new(encryption);

    // Test access token encryption
    let access_token = "ya29.a0AfH6SMBx...";
    let encrypted = encryptor.encrypt_access_token(access_token).unwrap();
    let decrypted = encryptor.decrypt_access_token(&encrypted).unwrap();
    assert_eq!(access_token, decrypted);

    // Test refresh token encryption
    let refresh_token = "1//0gHZs...";
    let encrypted = encryptor.encrypt_refresh_token(refresh_token).unwrap();
    let decrypted = encryptor.decrypt_refresh_token(&encrypted).unwrap();
    assert_eq!(refresh_token, decrypted);

    // Test client secret encryption
    let client_secret = "GOCSPX-AbCdEf...";
    let encrypted = encryptor.encrypt_client_secret(client_secret).unwrap();
    let decrypted = encryptor.decrypt_client_secret(&encrypted).unwrap();
    assert_eq!(client_secret, decrypted);
}

#[test]
fn test_password_based_encryption() {
    let password = "my_secure_password_2025";
    let salt = b"random_salt_value";

    let encryption = DataEncryption::from_password(password, salt).unwrap();

    let plaintext = "sensitive data";
    let encrypted = encryption.encrypt_field(plaintext).unwrap();
    let decrypted = encryption.decrypt_field(&encrypted).unwrap();

    assert_eq!(plaintext, decrypted);
}

// ============================================================================
// Integration Tests - Combined Components
// ============================================================================

#[tokio::test]
async fn test_full_auth_flow_simulation() {
    // Setup all components
    let key: [u8; 32] = rand::random();
    let pool = sqlx::PgPool::connect_lazy(
        "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine",
    )
    .unwrap();
    let auditor = Arc::new(SecurityAuditor::new(pool.clone()));
    let session_manager =
        Arc::new(SecureSessionManager::new(pool, &key, 3600, 3, auditor).unwrap());
    let oauth_state = Arc::new(OAuthStateManager::new(key));
    let encryption = Arc::new(DataEncryption::new(&key));
    let encryptor = FieldEncryptor::new(encryption);

    // 1. User initiates OAuth login
    let state = oauth_state.generate_state(None).await;
    println!("Generated OAuth state: {}", state);

    // 2. User returns from OAuth provider with state
    oauth_state
        .validate_state(&state, None)
        .await
        .expect("State validation failed");
    println!("OAuth state validated");

    // 3. Exchange code for tokens (simulated)
    let access_token = "ya29.oauth_access_token";
    let encrypted_token = encryptor.encrypt_access_token(access_token).unwrap();
    println!("Access token encrypted and stored");

    // 4. Create session
    let params = CreateSessionParams {
        user_id: "google_12345".to_string(),
        provider: "google".to_string(),
        email: Some("user@gmail.com".to_string()),
        name: Some("Test User".to_string()),
        is_admin: false,
        is_editor: false,
        ip_addr: "203.0.113.1".to_string(),
        user_agent: "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)".to_string(),
        refresh_token: None,
        audience: None,
    };
    let session_token = session_manager.create_session(params).await.unwrap();
    println!("Session created: {}", session_token.token);

    // 5. Subsequent request - validate session
    let session = session_manager
        .validate_session(
            &session_token.token,
            "203.0.113.1",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
        )
        .await
        .unwrap();
    println!("Session validated for user: {}", session.user_id);

    // 6. Access protected resource (decrypt token)
    let decrypted_token = encryptor.decrypt_access_token(&encrypted_token).unwrap();
    assert_eq!(access_token, decrypted_token);
    println!("Access token decrypted for API call");

    // 7. Logout
    session_manager
        .invalidate_session(&session_token.token)
        .await
        .unwrap();
    println!("Session invalidated");

    // 8. Verify session is gone
    let result = session_manager
        .validate_session(
            &session_token.token,
            "203.0.113.1",
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)",
        )
        .await;
    assert!(result.is_err());
    println!("Session cleanup verified");
}

#[tokio::test]
async fn test_concurrent_users_isolation() {
    let key: [u8; 32] = rand::random();
    let pool = sqlx::PgPool::connect_lazy(
        "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine",
    )
    .unwrap();
    let auditor = Arc::new(SecurityAuditor::new(pool.clone()));
    let session_manager =
        Arc::new(SecureSessionManager::new(pool, &key, 3600, 5, auditor).unwrap());

    // Create sessions for multiple users
    let users = vec!["alice", "bob", "charlie"];
    let mut tokens = vec![];

    for user in &users {
        let params = CreateSessionParams {
            user_id: user.to_string(),
            provider: "google".to_string(),
            email: None,
            name: None,
            is_admin: false,
            is_editor: false,
            ip_addr: format!("192.168.1.{}", user.len()),
            user_agent: "Mozilla/5.0".to_string(),
            refresh_token: None,
            audience: None,
        };
        let token = session_manager.create_session(params).await.unwrap();
        tokens.push((user, token));
    }

    // Verify each user's session is isolated
    for (user, token) in tokens {
        let session = session_manager
            .validate_session(
                &token.token,
                &format!("192.168.1.{}", user.len()),
                "Mozilla/5.0",
            )
            .await
            .unwrap();
        assert_eq!(&session.user_id, user);
    }
}
