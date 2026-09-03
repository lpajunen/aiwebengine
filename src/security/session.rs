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

/// Reduce a resource indicator to the two things that decide whether two of
/// them name the same endpoint: the host it lives on, and the path within it.
///
/// Callers write these in whatever form their client library favours — an
/// absolute URI from an RFC 8707 `resource` parameter, or the bare path the
/// engine defaults to — so comparing the strings as given would reject matching
/// pairs and, worse, accept differing ones by accident.
///
/// The host is kept. Two `/mcp` endpoints on two hosts of the same engine are
/// two different resources, and collapsing them is what let a token minted on a
/// solution's host reach the management host.
pub fn normalize_resource(resource: &str) -> String {
    // Drop the scheme; https and http reach the same endpoint here, and a
    // resource indicator is a name rather than a way to connect.
    let without_scheme = match resource.find("://") {
        Some(idx) => &resource[idx + 3..],
        None => resource,
    };

    // A query or fragment is not part of what is being named.
    let without_suffix = without_scheme
        .split(['?', '#'])
        .next()
        .unwrap_or(without_scheme);

    let (authority, path) = match without_suffix.find('/') {
        Some(idx) => (&without_suffix[..idx], &without_suffix[idx..]),
        None => (without_suffix, ""),
    };

    // Default ports name the same endpoint as no port at all.
    let authority = authority
        .strip_suffix(":443")
        .or_else(|| authority.strip_suffix(":80"))
        .unwrap_or(authority)
        .to_lowercase();

    let path = path.trim_end_matches('/');

    format!("{}{}", authority, path)
}

/// Delete every session belonging to a user, and every refresh token that
/// could mint another one.
///
/// A free function because the callers that most need it hold a database pool
/// and no session manager: roles change in the user repository, and a role that
/// has changed has to reach sessions that were minted before it did.
///
/// The refresh tokens go with them. Ending the sessions alone would revoke
/// nothing durable — a client holding a refresh token mints a new session on
/// its next call, and the role or realm just taken away comes straight back
/// with it.
pub async fn delete_sessions_for_user(pool: &PgPool, user_id: &str) -> Result<u64, sqlx::Error> {
    let result = sqlx::query("DELETE FROM sessions WHERE user_id = $1")
        .bind(user_id)
        .execute(pool)
        .await?;

    crate::auth::refresh_tokens::revoke_for_user(pool, user_id).await?;

    Ok(result.rows_affected())
}

/// Whether a session's audience authorizes the resource being requested.
///
/// Exact match after normalization. Deliberately not a prefix or suffix test:
/// a token for `/mcp` must not reach anything nested under it, and a token for
/// one host must not reach the same path on another.
pub fn resources_match(audience: &str, requested: &str) -> bool {
    normalize_resource(audience) == normalize_resource(requested)
}

/// Whether a session established for `realm` authenticates on `host`.
///
/// Delegates the rule to [`crate::user_repository::realm_authorizes_host`]; a
/// session with no realm recorded predates realm scoping and authorizes
/// nothing.
pub fn realm_authorizes_host(realm: Option<&str>, host: &str) -> bool {
    match realm {
        Some(realm) => crate::user_repository::realm_authorizes_host(realm, host),
        None => false,
    }
}

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

    #[error("Session is not authorized for this resource")]
    WrongAudience,

    #[error("Session does not authenticate on this host")]
    WrongRealm,

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
    /// The host this session's account is a principal on, copied from the user
    /// when the session was minted. `None` marks a session established before
    /// realms existed, which authorizes nothing — sign in again.
    #[serde(default)]
    pub realm: Option<String>,
}

/// One of an account's sessions, as its owner is allowed to see it.
///
/// Deliberately not [`SessionData`]. That carries the session token in
/// `session_id`, and a list of a person's sessions that hands back their tokens
/// would be a way to escalate one stolen session into all of them. What
/// identifies a row here is the `sessions.id` surrogate key, which is useless
/// to anyone who cannot already authenticate as this account.
///
/// There is no device name because the engine does not keep one: the fingerprint
/// stores a *hash* of the User-Agent, enough to notice it changed and not enough
/// to say what it was. What a person recognises a session by is the address it
/// was started from and when it was last used.
#[derive(Debug, Clone, Serialize)]
pub struct SessionSummary {
    /// Surrogate key. Names the session for revocation, authenticates nothing.
    pub id: uuid::Uuid,
    pub provider: String,
    /// Where it was minted from.
    pub ip_addr: String,
    pub created_at: DateTime<Utc>,
    pub last_access: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    /// Present on an API token and absent on a browser session, which is the
    /// difference worth showing: one is a program acting as you, the other is a
    /// browser you signed in with.
    pub audience: Option<String>,
    /// The session the request asking for this list arrived on.
    pub current: bool,
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
    /// The host the account is a principal on, read from the user record.
    pub realm: String,
}

/// What a session was minted against, so a token presented from somewhere else
/// can be noticed.
///
/// Two facts and no policy. Deciding what a mismatch *means* belongs to
/// [`SecureSessionManager`], which knows whether the session is a browser
/// cookie or an API token and what the operator configured — a fingerprint that
/// answered that question itself is how the rule ended up stamped into every
/// stored session and impossible to change afterwards.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionFingerprint {
    pub ip_addr: String,
    pub user_agent_hash: String,
    /// Stamped by versions that decided strictness per session. Read by
    /// nothing: the manager's configuration decides now, so an operator turning
    /// strict validation on does not have to wait for every session minted
    /// under the old setting to age out.
    #[serde(default)]
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
        hex::encode(hasher.finalize())
    }

    /// Whether this is the address the session was minted from.
    pub fn ip_matches(&self, ip_addr: &str) -> bool {
        ip_addr == self.ip_addr
    }

    /// Whether this is the client the session was minted for.
    pub fn user_agent_matches(&self, user_agent: &str) -> bool {
        Self::hash_user_agent(user_agent) == self.user_agent_hash
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
    /// Absolute maximum session age duration
    max_session_age: Duration,
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
        max_session_age_seconds: i64,
        max_concurrent_sessions: usize,
        auditor: Arc<SecurityAuditor>,
    ) -> Result<Self, SessionError> {
        let cipher = Aes256Gcm::new(encryption_key.into());

        Ok(Self {
            pool,
            cipher,
            max_concurrent_sessions,
            session_timeout: Duration::seconds(session_timeout_seconds),
            max_session_age: Duration::seconds(max_session_age_seconds),
            auditor,
            // Mobile-friendly by default; `security.strict_ip_validation` turns
            // it on for a deployment whose callers do not move.
            strict_ip_validation: false,
        })
    }

    /// Hold sessions to the address they were minted from.
    ///
    /// Worth having only because an address is now established from the
    /// connection rather than read out of a header the caller wrote: pinning to
    /// a claim pins nothing. Costly for a phone, which changes networks
    /// mid-session and would be signed out for it — and cheap for a personal
    /// install or an engine reached from fixed addresses, which is where an
    /// operator would turn it on.
    pub fn with_strict_ip_validation(mut self, strict: bool) -> Self {
        self.strict_ip_validation = strict;
        self
    }

    /// Whether this presentation of a session matches what it was minted
    /// against. Answers `true` when the address is new but the session stands.
    ///
    /// The old shape of this asked one question and then unpicked the answer
    /// with exceptions: a User-Agent mismatch was forgiven for a caller whose
    /// User-Agent *said* it was an editor or an MCP client, and any mismatch at
    /// all was forgiven once the address had changed — so the more of the
    /// fingerprint differed, the more likely the session was accepted. Both
    /// halves are set by whoever holds the token.
    async fn check_binding(
        &self,
        session_data: &SessionData,
        ip_addr: &str,
        user_agent: &str,
    ) -> Result<bool, SessionError> {
        let ip_matches = session_data.fingerprint.ip_matches(ip_addr);
        let user_agent_matches = session_data.fingerprint.user_agent_matches(user_agent);

        if ip_matches && user_agent_matches {
            return Ok(false);
        }

        self.auditor
            .log_event(
                SecurityEvent::new(
                    SecurityEventType::SuspiciousActivity,
                    SecuritySeverity::High,
                    Some(session_data.user_id.clone()),
                )
                .with_detail("reason", "Session fingerprint mismatch")
                .with_detail("ip_addr", ip_addr)
                .with_detail("expected_ip", &session_data.fingerprint.ip_addr)
                .with_detail("user_agent_matches", user_agent_matches.to_string()),
            )
            .await;

        // The address, when the operator asked for it to be binding. Worth
        // having now that an address is established from the connection rather
        // than read out of a header the caller wrote — before that, pinning to
        // an address pinned to a claim. Off by default, because a phone moving
        // between networks changes address mid-session and would otherwise be
        // signed out for it.
        if self.strict_ip_validation && !ip_matches {
            debug!(
                "Session for user {} was minted at {} and presented from {}",
                session_data.user_id, session_data.fingerprint.ip_addr, ip_addr
            );
            return Err(SessionError::FingerprintMismatch);
        }

        // The client. A browser does not change its User-Agent mid-session, so
        // for a cookie this is a real binding and a mismatch ends the session.
        //
        // An API token is different in kind: an audience says it was minted by
        // the token endpoint for programmatic use, and such a client does the
        // OAuth exchange in one HTTP stack and its API calls in another. Pinning
        // the User-Agent there breaks interop without adding security, since it
        // is self-reported either way — which is what the removed editor-name
        // sniffing was groping towards. What bounds an API token is its audience
        // and its realm, and both are checked on every request.
        let is_api_token = session_data.audience.is_some();
        if !user_agent_matches && !is_api_token {
            return Err(SessionError::FingerprintMismatch);
        }

        Ok(!ip_matches)
    }

    fn absolute_expiry_for_created_at(&self, created_at: DateTime<Utc>) -> DateTime<Utc> {
        created_at + self.max_session_age
    }

    fn next_sliding_expiry(&self, created_at: DateTime<Utc>, now: DateTime<Utc>) -> DateTime<Utc> {
        let sliding_expiry = now + self.session_timeout;
        let absolute_expiry = self.absolute_expiry_for_created_at(created_at);
        if sliding_expiry < absolute_expiry {
            sliding_expiry
        } else {
            absolute_expiry
        }
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
        let expires_at = self.next_sliding_expiry(now, now);

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
            realm: Some(params.realm.clone()),
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
        host: &str,
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

        // Enforce absolute maximum session age even if sliding timeout was refreshed.
        if self.absolute_expiry_for_created_at(session_data.created_at) < Utc::now() {
            self.invalidate_session(token).await?;
            return Err(SessionError::SessionExpired);
        }

        // Does this account authenticate on the host the request arrived on?
        //
        // Checked here, at the bottom, rather than in each middleware, because
        // a session token is accepted from an `Authorization: Bearer` header as
        // readily as from a cookie — and a bearer header carries none of the
        // host scoping the browser applies to a cookie. Every path that turns a
        // token into a principal comes through here, so this is the one place
        // that cannot be forgotten.
        if !realm_authorizes_host(session_data.realm.as_deref(), host) {
            debug!(
                "Session for user {} does not authenticate on host {} (realm {:?})",
                session_data.user_id, host, session_data.realm
            );
            return Err(SessionError::WrongRealm);
        }

        // A new address on a session that is otherwise sound is recorded, so the
        // fingerprint keeps describing where the session is being used from.
        if self
            .check_binding(&session_data, ip_addr, user_agent)
            .await?
        {
            warn!(
                "IP changed for user {} session (old: {}, new: {})",
                session_data.user_id, session_data.fingerprint.ip_addr, ip_addr
            );
            session_data.fingerprint.ip_addr = ip_addr.to_string();
        }

        // Update last access time and slide the expiry window so the user
        // stays logged in as long as they are actively using the app.
        let now = Utc::now();
        session_data.last_access = now;
        session_data.expires_at = self.next_sliding_expiry(session_data.created_at, now);

        // Re-encrypt and update session
        let encrypted = self.encrypt_session(&session_data)?;
        let encrypted_json = serde_json::to_value(&encrypted)
            .map_err(|e| SessionError::EncryptionError(e.to_string()))?;

        sqlx::query(
            "UPDATE sessions SET data = $1, expires_at = $2, last_accessed_at = NOW() WHERE session_id = $3",
        )
        .bind(encrypted_json)
        .bind(session_data.expires_at)
        .bind(token)
        .execute(&self.pool)
        .await
        .map_err(|e| SessionError::ValidationFailed(format!("Database error: {}", e)))?;

        debug!("Validated session for user {}", session_data.user_id);

        Ok(session_data)
    }

    /// Validate a session and check it was issued for the resource being asked
    /// for (RFC 8707).
    ///
    /// A session carries an audience when it was minted for programmatic use —
    /// the OAuth2 token endpoint sets one on every token it issues. A browser
    /// login carries none, because it was minted for a browser. So requiring an
    /// audience here is what separates the two: a session cookie is not an API
    /// credential, and presenting one as a bearer token is refused.
    ///
    /// That distinction matters because a bearer token is not bound by the
    /// cookie's host scoping. Without this, a session established on one host
    /// reaches `/mcp` on every host the process serves, including a management
    /// host the cookie would never have been sent to.
    ///
    /// # Arguments
    /// * `token` - Session token to validate
    /// * `ip_addr` - Client IP address
    /// * `user_agent` - Client user agent
    /// * `resource` - Resource being requested; `None` skips the check, for the
    ///   ordinary session paths that are not resource-scoped
    ///
    /// # Returns
    /// Session data if valid and authorized for the requested resource
    pub async fn validate_session_with_resource(
        &self,
        token: &str,
        ip_addr: &str,
        user_agent: &str,
        host: &str,
        resource: Option<&str>,
    ) -> Result<SessionData, SessionError> {
        // First validate the session normally
        let session_data = self
            .validate_session(token, ip_addr, user_agent, host)
            .await?;

        if let Some(requested_resource) = resource {
            let Some(audience) = &session_data.audience else {
                debug!(
                    "Session carries no audience and cannot be used for resource {}",
                    requested_resource
                );
                return Err(SessionError::WrongAudience);
            };

            if !resources_match(audience, requested_resource) {
                debug!(
                    "Session audience does not authorize this resource: session={}, requested={}",
                    audience, requested_resource
                );
                return Err(SessionError::WrongAudience);
            }
        }

        Ok(session_data)
    }

    /// Extend an existing session and optionally rotate its OAuth refresh token.
    ///
    /// This is used by engine-managed refresh flows to keep authenticated users signed in
    /// without forcing a full interactive login.
    pub async fn refresh_session(
        &self,
        token: &str,
        ip_addr: &str,
        user_agent: &str,
        host: &str,
        new_refresh_token: Option<String>,
    ) -> Result<SessionData, SessionError> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| SessionError::ValidationFailed(format!("Database error: {}", e)))?;

        let row: Option<(serde_json::Value, DateTime<Utc>)> = sqlx::query_as(
            "SELECT data, expires_at FROM sessions WHERE session_id = $1 FOR UPDATE",
        )
        .bind(token)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| SessionError::ValidationFailed(format!("Database error: {}", e)))?;

        let (encrypted_json, expires_at) = row.ok_or(SessionError::SessionNotFound)?;
        if expires_at < Utc::now() {
            let _ = tx.rollback().await;
            return Err(SessionError::SessionExpired);
        }

        let encrypted: EncryptedSessionData = serde_json::from_value(encrypted_json)
            .map_err(|e| SessionError::ValidationFailed(format!("Data corruption: {}", e)))?;

        let mut session_data = self.decrypt_session(&encrypted)?;

        // Do not allow refresh beyond absolute maximum session age.
        if self.absolute_expiry_for_created_at(session_data.created_at) < Utc::now() {
            let _ = tx.rollback().await;
            self.invalidate_session(token).await?;
            return Err(SessionError::SessionExpired);
        }

        // The same binding as an ordinary request. Refreshing does not
        // authenticate anything by itself, but a session that could not be used
        // from here should not be extended from here either — and without this,
        // `strict_ip_validation` would hold every request and no renewal.
        if self
            .check_binding(&session_data, ip_addr, user_agent)
            .await?
        {
            session_data.fingerprint.ip_addr = ip_addr.to_string();
        }

        // Refreshing reads and rewrites the session without going through
        // `validate_session`, so the realm has to be checked here too. A
        // session close enough to expiry to be renewed would otherwise be the
        // one way onto a host it does not authenticate on.
        if !realm_authorizes_host(session_data.realm.as_deref(), host) {
            let _ = tx.rollback().await;
            debug!(
                "Refusing to refresh session for user {} on host {} (realm {:?})",
                session_data.user_id, host, session_data.realm
            );
            return Err(SessionError::WrongRealm);
        }

        if let Some(token_value) = new_refresh_token {
            session_data.refresh_token = Some(token_value);
        }

        let now = Utc::now();
        session_data.last_access = now;
        session_data.expires_at = self.next_sliding_expiry(session_data.created_at, now);

        let encrypted = self.encrypt_session(&session_data)?;
        let encrypted_json = serde_json::to_value(&encrypted)
            .map_err(|e| SessionError::EncryptionError(e.to_string()))?;

        sqlx::query(
            "UPDATE sessions
             SET data = $1, expires_at = $2, last_accessed_at = NOW()
             WHERE session_id = $3",
        )
        .bind(encrypted_json)
        .bind(session_data.expires_at)
        .bind(token)
        .execute(&mut *tx)
        .await
        .map_err(|e| SessionError::ValidationFailed(format!("Database error: {}", e)))?;

        tx.commit()
            .await
            .map_err(|e| SessionError::ValidationFailed(format!("Database error: {}", e)))?;

        Ok(session_data)
    }

    /// Invalidate a session (logout)
    pub async fn invalidate_session(&self, token: &str) -> Result<(), SessionError> {
        self.invalidate_session_internal(token).await
    }

    /// Every session an account currently holds, newest use first.
    ///
    /// The rows are decrypted to read them, because everything worth showing —
    /// where a session was started from, whether it is an API token — lives
    /// inside the encrypted blob and not in the columns. A row that will not
    /// decrypt is skipped rather than failing the list: one unreadable session
    /// must not stop somebody seeing the other four and ending the one they do
    /// not recognise.
    ///
    /// `current_token` is the session the request arrived on, so the list can
    /// say which one that is. It is compared and never returned.
    pub async fn list_sessions_for_user(
        &self,
        user_id: &str,
        current_token: &str,
    ) -> Result<Vec<SessionSummary>, SessionError> {
        let rows: Vec<(uuid::Uuid, String, serde_json::Value, DateTime<Utc>)> = sqlx::query_as(
            "SELECT id, session_id, data, expires_at FROM sessions
             WHERE user_id = $1 AND expires_at > NOW()
             ORDER BY last_accessed_at DESC",
        )
        .bind(user_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| SessionError::ValidationFailed(format!("Database error: {}", e)))?;

        let mut sessions = Vec::with_capacity(rows.len());
        for (id, session_id, encrypted_json, expires_at) in rows {
            let encrypted: EncryptedSessionData = match serde_json::from_value(encrypted_json) {
                Ok(encrypted) => encrypted,
                Err(e) => {
                    warn!("Skipping an unreadable session row for {}: {}", user_id, e);
                    continue;
                }
            };

            let data = match self.decrypt_session(&encrypted) {
                Ok(data) => data,
                Err(e) => {
                    warn!(
                        "Skipping an undecryptable session row for {}: {}",
                        user_id, e
                    );
                    continue;
                }
            };

            sessions.push(SessionSummary {
                id,
                provider: data.provider,
                ip_addr: data.fingerprint.ip_addr,
                created_at: data.created_at,
                last_access: data.last_access,
                // The column rather than the copy inside the blob: sliding
                // expiry is written to the row, and the two disagree by design.
                expires_at,
                audience: data.audience,
                current: session_id == current_token,
            });
        }

        Ok(sessions)
    }

    /// End one of an account's sessions, named by the surrogate key its owner
    /// was shown.
    ///
    /// Scoped to `user_id` in the statement itself rather than by checking
    /// first: an id belonging to somebody else deletes nothing, and cannot be
    /// made to by a race between the check and the delete.
    ///
    /// Answers with what the revoked session was — nothing that authenticates,
    /// but including its audience, which is what decides whether a refresh
    /// token has to go with it. `None` means this account has no such session.
    pub async fn delete_session_for_user(
        &self,
        user_id: &str,
        id: uuid::Uuid,
        current_token: &str,
    ) -> Result<Option<SessionSummary>, SessionError> {
        let Some(session) = self
            .list_sessions_for_user(user_id, current_token)
            .await?
            .into_iter()
            .find(|session| session.id == id)
        else {
            return Ok(None);
        };

        let result = sqlx::query("DELETE FROM sessions WHERE id = $1 AND user_id = $2")
            .bind(id)
            .bind(user_id)
            .execute(&self.pool)
            .await
            .map_err(|e| SessionError::ValidationFailed(format!("Database error: {}", e)))?;

        if result.rows_affected() == 0 {
            return Ok(None);
        }

        info!("Session {} ended by its owner {}", id, user_id);
        Ok(Some(session))
    }

    /// End every session an account holds except the one asking.
    ///
    /// The control somebody reaches for when they have lost a device rather
    /// than a password: it does not need them to know which session is which,
    /// and it does not sign them out of the browser they are using to do it.
    pub async fn delete_other_sessions_for_user(
        &self,
        user_id: &str,
        current_token: &str,
    ) -> Result<u64, SessionError> {
        let result = sqlx::query("DELETE FROM sessions WHERE user_id = $1 AND session_id <> $2")
            .bind(user_id)
            .bind(current_token)
            .execute(&self.pool)
            .await
            .map_err(|e| SessionError::ValidationFailed(format!("Database error: {}", e)))?;

        Ok(result.rows_affected())
    }

    /// Tear down every session this user holds, and answer with how many.
    ///
    /// Logging out deletes one token, which is the wrong tool for everything
    /// that is not a person clicking log out. A role taken away, a password
    /// changed, an account deleted — each of those is a statement about what
    /// the account may do from now on, and roles are stamped into a session
    /// when it is minted rather than read per request. Without this, revoking
    /// an administrator leaves them administering until their session ages out,
    /// which is up to `max_session_age` — thirty days by default.
    pub async fn invalidate_all_sessions_for_user(
        &self,
        user_id: &str,
    ) -> Result<u64, SessionError> {
        let deleted = delete_sessions_for_user(&self.pool, user_id)
            .await
            .map_err(|e| SessionError::ValidationFailed(format!("Database error: {}", e)))?;

        if deleted > 0 {
            self.auditor
                .log_event(
                    SecurityEvent::new(
                        SecurityEventType::SystemSecurityEvent,
                        SecuritySeverity::Medium,
                        Some(user_id.to_string()),
                    )
                    .with_detail("action", "all_sessions_invalidated")
                    .with_detail("sessions", deleted.to_string()),
                )
                .await;
        }

        Ok(deleted)
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

    #[test]
    fn resource_names_survive_the_forms_clients_write_them_in() {
        // Scheme, default port, trailing slash and case are all noise.
        for equivalent in [
            "https://game.example.com/mcp",
            "http://game.example.com/mcp",
            "https://game.example.com:443/mcp",
            "https://GAME.example.com/mcp/",
            "game.example.com/mcp",
        ] {
            assert!(
                resources_match(equivalent, "game.example.com/mcp"),
                "{:?} names the same endpoint",
                equivalent
            );
        }
    }

    #[test]
    fn the_host_is_part_of_what_a_resource_names() {
        assert!(
            !resources_match("https://game.example.com/mcp", "manage.example.com/mcp"),
            "two hosts of one engine are two resources; collapsing them is what \
             let a solution's token reach the management surface"
        );
        assert!(
            !resources_match("game.example.com:8443/mcp", "game.example.com/mcp"),
            "a non-default port is part of the name"
        );
    }

    #[test]
    fn a_resource_does_not_authorize_what_is_nested_under_it() {
        assert!(!resources_match(
            "game.example.com/mcp",
            "game.example.com/mcp/admin"
        ));
        assert!(!resources_match(
            "game.example.com/mcp/admin",
            "game.example.com/mcp"
        ));
    }

    fn create_test_auditor() -> Arc<SecurityAuditor> {
        let pool = sqlx::PgPool::connect_lazy(
            "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine",
        )
        .unwrap();
        Arc::new(SecurityAuditor::new(Some(pool)))
    }

    fn create_test_manager() -> SecureSessionManager {
        let key: [u8; 32] = rand::random();
        let pool = sqlx::PgPool::connect_lazy(
            "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine",
        )
        .unwrap();
        SecureSessionManager::new(pool, &key, 3600, 86400 * 30, 3, create_test_auditor()).unwrap()
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
            realm: "test.example.com".to_string(),
        };

        let token = manager.create_session(params).await.unwrap();

        let session = manager
            .validate_session(
                &token.token,
                "192.168.1.1",
                "Mozilla/5.0",
                "test.example.com",
            )
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
            realm: "test.example.com".to_string(),
        };

        let token = manager.create_session(params).await.unwrap();

        // Different user agent should fail
        let result = manager
            .validate_session(
                &token.token,
                "192.168.1.1",
                "Chrome/90.0",
                "test.example.com",
            )
            .await;
        assert!(matches!(result, Err(SessionError::FingerprintMismatch)));
    }

    /// A bearer token from the OAuth2 token endpoint: the client that
    /// exchanged the code is not the one that calls the API, so its
    /// self-reported User-Agent is not a binding worth keeping.
    ///
    /// What identifies such a token is its audience, not the provider string
    /// beside it — every token that endpoint issues carries one, and a browser
    /// login carries none. Reading the provider instead was a name check on a
    /// field that says where an identity came from rather than what the
    /// credential is for.
    #[tokio::test]
    async fn test_oauth2_bearer_session_allows_user_agent_change() {
        let manager = create_test_manager();

        let params = CreateSessionParams {
            user_id: "user_oauth2_ua".to_string(),
            provider: "oauth2".to_string(),
            email: None,
            name: None,
            is_admin: false,
            is_editor: false,
            ip_addr: "192.168.1.1".to_string(),
            user_agent: "token-exchange-client/1.0".to_string(),
            refresh_token: None,
            audience: Some("test.example.com/mcp".to_string()),
            realm: "test.example.com".to_string(),
        };

        let token = manager.create_session(params).await.unwrap();

        let session = manager
            .validate_session(
                &token.token,
                "192.168.1.1",
                "claude-code/2.0 (external, cli)",
                "test.example.com",
            )
            .await
            .unwrap();
        assert_eq!(session.user_id, "user_oauth2_ua");
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
                    realm: "test.example.com".to_string(),
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
            realm: "test.example.com".to_string(),
        };

        let token = manager.create_session(params).await.unwrap();

        // Validate session exists
        manager
            .validate_session(
                &token.token,
                "192.168.1.1",
                "Mozilla/5.0",
                "test.example.com",
            )
            .await
            .unwrap();

        // Invalidate session
        manager.invalidate_session(&token.token).await.unwrap();

        // Should fail after invalidation
        let result = manager
            .validate_session(
                &token.token,
                "192.168.1.1",
                "Mozilla/5.0",
                "test.example.com",
            )
            .await;
        assert!(matches!(result, Err(SessionError::SessionNotFound)));
    }
}
