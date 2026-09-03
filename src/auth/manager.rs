/// Authentication Manager
///
/// Central orchestrator for authentication operations, coordinating providers,
/// sessions, and security infrastructure.
use crate::auth::{
    AuthError, AuthSecurityContext, AuthSessionManager, OAuth2Provider, OAuth2ProviderConfig,
    OAuth2TokenResponse, OAuth2UserInfo, ProviderFactory,
};
use chrono::Utc;
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

/// The `__Host-` cookie name prefix.
///
/// See [`host_scoped_cookie_name`] for why the engine applies it.
pub const HOST_COOKIE_PREFIX: &str = "__Host-";

/// Derive the session cookie's real name, applying the `__Host-` prefix when
/// the browser will accept it.
///
/// Cookies are scoped by domain suffix, not by origin: a cookie set with
/// `Domain=example.com` is sent to every host under it, at any depth, and any
/// one of those hosts can set it. So on a deployment serving several hosts off
/// one registrable domain, a host running solution-author scripts can set a
/// session cookie that a management host would then read as its own — the
/// sibling never needed to be trusted for this to work. `SameSite` does not
/// help: every host under one registrable domain is the *same site*, whatever
/// the policy says.
///
/// The engine never sends a `Domain` attribute, so its session cookie is
/// host-only already. What this prefix adds is that the *browser* enforces it:
/// a cookie whose name starts with `__Host-` is rejected outright unless it is
/// `Secure`, has `Path=/`, and carries no `Domain`. A sibling host cannot
/// shadow it with a domain-wide cookie of the same name, because the browser
/// will not store what that host tried to set. The convention becomes a
/// guarantee that does not depend on every future handler remembering it.
///
/// The prefix is applied only when the cookie is `Secure`. Over plain HTTP —
/// local development — a `__Host-` cookie is discarded by the browser, taking
/// sign-in with it, so an insecure cookie keeps the bare name. That also means
/// a production config copied to a dev machine still logs in: the prefix is
/// stripped rather than honoured when `secure` is off.
pub fn host_scoped_cookie_name(configured: &str, secure: bool) -> String {
    let bare = configured
        .strip_prefix(HOST_COOKIE_PREFIX)
        .unwrap_or(configured);

    if secure {
        format!("{HOST_COOKIE_PREFIX}{bare}")
    } else {
        bare.to_string()
    }
}

/// Authentication manager configuration
#[derive(Debug, Clone)]
pub struct AuthManagerConfig {
    /// Base URL for redirect URIs
    pub base_url: String,

    /// Session cookie name
    pub session_cookie_name: String,

    /// Session cookie secure flag
    pub cookie_secure: bool,

    /// Session timeout in seconds
    pub session_timeout: u64,

    /// Absolute maximum session age in seconds (30 days default)
    pub max_session_age: u64,

    /// Which internal-credential flows this engine accepts.
    pub internal: crate::auth::config::InternalAuthConfig,
}

impl Default for AuthManagerConfig {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:3000".to_string(),
            session_cookie_name: "auth_session".to_string(),
            cookie_secure: true,
            session_timeout: 3600 * 24 * 7,  // 7 days
            max_session_age: 3600 * 24 * 30, // 30 days
            internal: crate::auth::config::InternalAuthConfig::default(),
        }
    }
}

/// Central authentication manager
pub struct AuthManager {
    config: AuthManagerConfig,
    providers: HashMap<String, Arc<Box<dyn OAuth2Provider>>>,
    /// Providers bound to a specific request host, keyed by (host, provider).
    ///
    /// Each entry is a separate provider instance carrying the redirect URI for
    /// that host, so an OAuth flow started on `manage.example.com` comes back to
    /// `manage.example.com` and sets its session cookie there. Instances are
    /// built at startup from configuration, so a request's Host header is only
    /// ever used as a lookup key here — never to construct a redirect URI.
    host_providers: HashMap<(String, String), Arc<Box<dyn OAuth2Provider>>>,
    session_manager: Arc<AuthSessionManager>,
    security_context: Arc<AuthSecurityContext>,
    api_key: Option<String>,
}

const SESSION_REFRESH_WINDOW_SECONDS: i64 = 300;

/// Resolve a request's `Host` header to the realm name a session is checked
/// against.
///
/// Every entry point hands this its raw header and nothing else, so the
/// mapping from what a client claims to what the engine recognises happens
/// once. `canonical_host` folds an unknown host onto the default, which is the
/// same answer the router gives it — a request the engine will not route
/// anywhere else must not authenticate anywhere else either.
fn realm_host(host: Option<&str>) -> String {
    crate::hosts::canonical_host(host)
}

/// An identity that has already been established, on its way to becoming a
/// session. What is left after the provider-specific part of a login is done.
struct SessionRequest {
    user_id: String,
    provider: String,
    email: Option<String>,
    name: Option<String>,
    ip_addr: String,
    user_agent: String,
    refresh_token: Option<String>,
}

impl AuthManager {
    /// Create a new authentication manager
    pub fn new(
        config: AuthManagerConfig,
        session_manager: Arc<AuthSessionManager>,
        security_context: Arc<AuthSecurityContext>,
        api_key: Option<String>,
    ) -> Self {
        Self {
            config,
            providers: HashMap::new(),
            host_providers: HashMap::new(),
            session_manager,
            security_context,
            api_key,
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

    /// Register an OAuth2 provider bound to one request host.
    ///
    /// `host` is matched against the request's Host header, so it must be the
    /// hostname on its own, or `hostname:port` when the port is non-default.
    pub fn register_provider_for_host(
        &mut self,
        host: &str,
        provider_name: &str,
        provider_config: OAuth2ProviderConfig,
    ) -> Result<(), AuthError> {
        let provider = ProviderFactory::create_provider(provider_name, provider_config)?;
        self.host_providers.insert(
            (host.to_lowercase(), provider_name.to_string()),
            Arc::new(provider),
        );
        Ok(())
    }

    /// Get a registered provider
    pub fn get_provider(&self, provider_name: &str) -> Option<Arc<Box<dyn OAuth2Provider>>> {
        self.providers.get(provider_name).cloned()
    }

    /// Get the provider to use for a request arriving on `host`.
    ///
    /// Falls back to the host-independent registration when the host has no
    /// dedicated instance, which keeps single-host deployments and requests
    /// with an unrecognised Host header on the configured base URL.
    pub fn get_provider_for_host(
        &self,
        host: Option<&str>,
        provider_name: &str,
    ) -> Option<Arc<Box<dyn OAuth2Provider>>> {
        if let Some(host) = host
            && let Some(provider) = self
                .host_providers
                .get(&(host.to_lowercase(), provider_name.to_string()))
        {
            return Some(Arc::clone(provider));
        }

        // Falling back is correct, but on a deployment that configures extra
        // hosts it usually means one was missed: the user lands back on the
        // base URL and their session cookie is set on the wrong host. Say so,
        // rather than letting it look like the flow simply misbehaved.
        if !self.host_providers.is_empty() {
            tracing::warn!(
                "No {} OAuth2 provider registered for host {:?}; falling back to the \
                 base URL redirect URI, so this login will complete on a different host. \
                 Registered hosts: {:?}. Add the host to server.additional_base_urls \
                 (and its redirect URI to the provider) if it should log in on its own.",
                provider_name,
                host.unwrap_or("<no Host header>"),
                self.hosts_with_providers()
            );
        }

        self.get_provider(provider_name)
    }

    /// Hosts that have at least one dedicated provider instance registered.
    pub fn hosts_with_providers(&self) -> Vec<String> {
        let mut hosts: Vec<String> = self
            .host_providers
            .keys()
            .map(|(host, _)| host.clone())
            .collect();
        hosts.sort();
        hosts.dedup();
        hosts
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
    /// * `host` - Host the login was started on, selecting the redirect URI
    ///
    /// # Returns
    /// Tuple of (authorization_url, csrf_state_token)
    pub async fn start_login(
        &self,
        provider_name: &str,
        ip_addr: &str,
        host: Option<&str>,
    ) -> Result<(String, String), AuthError> {
        let provider = self
            .get_provider_for_host(host, provider_name)
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
    /// * `host` - Host the login was started on, selecting the redirect URI
    ///
    /// # Returns
    /// Tuple of (authorization_url, csrf_state_token)
    pub async fn start_login_with_redirect(
        &self,
        provider_name: &str,
        ip_addr: &str,
        redirect_url: String,
        host: Option<&str>,
    ) -> Result<(String, String), AuthError> {
        let provider = self
            .get_provider_for_host(host, provider_name)
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
    /// * `host` - Host the callback arrived on; must be the host the flow was
    ///   started on, since the token exchange has to repeat the same
    ///   redirect URI the authorization request used
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
        host: Option<&str>,
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
            .get_provider_for_host(host, provider_name)
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
            crate::hosts::canonical_host(host),
        )
        .await
        .map_err(|e| {
            tracing::error!("Failed to upsert user: {}", e);
            AuthError::Internal(format!("Failed to create/update user: {}", e))
        })?;

        self.establish_session(SessionRequest {
            user_id,
            provider: provider_name.to_string(),
            email: Some(user_info.email.clone()),
            name: user_info.name.clone(),
            ip_addr: ip_addr.to_string(),
            user_agent: user_agent.to_string(),
            refresh_token: tokens.refresh_token.clone(),
        })
        .await
    }

    /// Turn an established identity into a session.
    ///
    /// Everything an OAuth callback does after the provider has vouched for
    /// someone — read the roles that were stored, mint the session, record the
    /// success — is the same work whoever vouched. Keeping it in one place is
    /// what lets a guest, a local password and a federated login differ only in
    /// how the identity was established.
    ///
    /// Roles are read from the repository rather than taken from the caller, so
    /// no entry point can mint a session more privileged than the stored user.
    async fn establish_session(&self, request: SessionRequest) -> Result<String, AuthError> {
        let SessionRequest {
            user_id,
            provider,
            email,
            name,
            ip_addr,
            user_agent,
            refresh_token,
        } = request;

        let user = crate::user_repository::get_user_async(&user_id)
            .await
            .map_err(|e| {
                tracing::error!("User not found after upsert: {}", e);
                AuthError::Internal("User not found after creation".to_string())
            })?;

        let is_admin = user
            .roles
            .contains(&crate::user_repository::UserRole::Administrator);
        let is_editor = user
            .roles
            .contains(&crate::user_repository::UserRole::Editor);

        let session_token = self
            .session_manager
            .create_session(crate::auth::session::CreateAuthSessionParams {
                user_id: user_id.clone(),
                provider: provider.clone(),
                email,
                name,
                is_admin,
                is_editor,
                ip_addr: ip_addr.clone(),
                user_agent,
                refresh_token,
                audience: None, // Will be set for MCP endpoints
                // Read off the stored user rather than the login request, so a
                // sign-in on one host cannot mint a session that authenticates
                // on another.
                realm: user.realm.clone(),
            })
            .await?;

        self.security_context
            .log_auth_success(&user_id, &provider, Some(&ip_addr))
            .await;

        Ok(session_token.token)
    }

    /// Issue an identity and a session to a caller who presents no credential.
    ///
    /// The account is real — it owns storage and can be granted roles like any
    /// other — it simply has no way to be signed into again. That is what
    /// [`Self::claim_guest_account`] is for.
    pub async fn start_guest_session(
        &self,
        display_name: Option<String>,
        ip_addr: &str,
        user_agent: &str,
        host: Option<&str>,
    ) -> Result<String, AuthError> {
        if !self.config.internal.allow_guests {
            return Err(AuthError::GuestAuthDisabled);
        }

        if !self.security_context.check_auth_rate_limit(ip_addr).await {
            return Err(AuthError::RateLimitExceeded);
        }

        // The guest's provider_user_id is the only thing that identifies the
        // account, and nothing but the session cookie will ever present it, so
        // it is generated rather than chosen.
        let guest_id = uuid::Uuid::new_v4().to_string();
        let user_id = crate::user_repository::upsert_internal_user(
            display_name,
            crate::auth::local::GUEST_PROVIDER.to_string(),
            guest_id,
            crate::hosts::canonical_host(host),
        )
        .await
        .map_err(|e| {
            tracing::error!("Failed to create guest user: {}", e);
            AuthError::Internal(format!("Failed to create guest: {}", e))
        })?;

        self.establish_session(SessionRequest {
            user_id,
            provider: crate::auth::local::GUEST_PROVIDER.to_string(),
            email: None,
            name: None,
            ip_addr: ip_addr.to_string(),
            user_agent: user_agent.to_string(),
            refresh_token: None,
        })
        .await
    }

    /// Create an account named by a username and protected by a password.
    pub async fn register_local_account(
        &self,
        username: &str,
        password: &str,
        display_name: Option<String>,
        ip_addr: &str,
        user_agent: &str,
        host: Option<&str>,
    ) -> Result<String, AuthError> {
        if !self.config.internal.enabled {
            return Err(AuthError::LocalAuthDisabled);
        }
        if !self.config.internal.allow_registration {
            return Err(AuthError::LocalAuthDisabled);
        }

        if !self.security_context.check_auth_rate_limit(ip_addr).await {
            return Err(AuthError::RateLimitExceeded);
        }

        let normalized = crate::auth::local::validate_username(username)?;
        crate::auth::local::validate_password(password, self.config.internal.min_password_length)?;

        if crate::auth::local::username_exists(&normalized).await? {
            return Err(AuthError::UsernameTaken);
        }

        // The user row comes first: the credential references it, and a user
        // with no credential is a recoverable state (they can claim it) while a
        // credential pointing at nothing is not.
        let user_id = crate::user_repository::upsert_internal_user(
            display_name,
            crate::auth::local::LOCAL_PROVIDER.to_string(),
            normalized.clone(),
            crate::hosts::canonical_host(host),
        )
        .await
        .map_err(|e| {
            tracing::error!("Failed to create local user: {}", e);
            AuthError::Internal(format!("Failed to create account: {}", e))
        })?;

        crate::auth::local::attach_credential(
            &user_id,
            &normalized,
            password,
            self.config.internal.min_password_length,
        )
        .await?;

        // An account the operator named in configuration is an administrator
        // from its first moment, which is what gives a personal install an
        // owner at all.
        crate::user_repository::apply_bootstrap_admin_username(&user_id, &normalized)
            .await
            .map_err(|e| AuthError::Internal(format!("Failed to apply configured role: {}", e)))?;

        self.establish_session(SessionRequest {
            user_id,
            provider: crate::auth::local::LOCAL_PROVIDER.to_string(),
            email: None,
            name: None,
            ip_addr: ip_addr.to_string(),
            user_agent: user_agent.to_string(),
            refresh_token: None,
        })
        .await
    }

    /// Sign in against a credential this engine holds.
    pub async fn login_local(
        &self,
        username: &str,
        password: &str,
        ip_addr: &str,
        user_agent: &str,
    ) -> Result<String, AuthError> {
        if !self.config.internal.enabled {
            return Err(AuthError::LocalAuthDisabled);
        }

        if !self.security_context.check_auth_rate_limit(ip_addr).await {
            return Err(AuthError::RateLimitExceeded);
        }

        // And the account's own budget, which is what a guess spread across
        // many addresses runs into. Answered before the password is checked so
        // that an exhausted account costs an attacker an Argon2 hash of
        // nothing.
        if !self.security_context.account_login_allowed(username).await {
            return Err(AuthError::RateLimitExceeded);
        }

        let user_id = match crate::auth::local::verify_login(username, password).await {
            Ok(user_id) => user_id,
            Err(e) => {
                self.security_context
                    .record_account_login_failure(username)
                    .await;
                self.security_context
                    .log_auth_failure(
                        crate::auth::local::LOCAL_PROVIDER,
                        "Invalid username or password",
                        Some(ip_addr),
                    )
                    .await;
                return Err(e);
            }
        };

        // On every sign-in, not only at creation: the ordinary case is someone
        // who made an account first and wrote their username into the config
        // afterwards, and the account that already exists is exactly the one an
        // upsert would leave alone.
        crate::user_repository::apply_bootstrap_admin_username(
            &user_id,
            &crate::auth::local::normalize_username(username),
        )
        .await
        .map_err(|e| AuthError::Internal(format!("Failed to apply configured role: {}", e)))?;

        self.establish_session(SessionRequest {
            user_id,
            provider: crate::auth::local::LOCAL_PROVIDER.to_string(),
            email: None,
            name: None,
            ip_addr: ip_addr.to_string(),
            user_agent: user_agent.to_string(),
            refresh_token: None,
        })
        .await
    }

    /// Change an account's password, and end every session it had.
    ///
    /// Ending them is the point as much as the new password is: a password is
    /// changed because the old one may be known, and a session minted under the
    /// old one keeps working for up to `max_session_age` — thirty days by
    /// default — however good the new password is. The caller gets a fresh
    /// session back so that changing a password does not sign you out of the
    /// browser you changed it in.
    pub async fn change_local_password(
        &self,
        user_id: &str,
        current_password: &str,
        new_password: &str,
        ip_addr: &str,
        user_agent: &str,
    ) -> Result<String, AuthError> {
        if !self.config.internal.enabled {
            return Err(AuthError::LocalAuthDisabled);
        }

        if !self.security_context.check_auth_rate_limit(ip_addr).await {
            return Err(AuthError::RateLimitExceeded);
        }

        crate::auth::local::change_password(
            user_id,
            current_password,
            new_password,
            self.config.internal.min_password_length,
        )
        .await?;

        if let Err(e) = self
            .session_manager()
            .delete_all_sessions_for_user(user_id)
            .await
        {
            // The password is already changed. Say so loudly rather than
            // reporting a failure that would send someone to change it again.
            tracing::error!(
                "Password for {} changed but their sessions could not be ended: {}",
                user_id,
                e
            );
        }

        self.establish_session(SessionRequest {
            user_id: user_id.to_string(),
            provider: crate::auth::local::LOCAL_PROVIDER.to_string(),
            email: None,
            name: None,
            ip_addr: ip_addr.to_string(),
            user_agent: user_agent.to_string(),
            refresh_token: None,
        })
        .await
    }

    /// The sessions this account holds, newest use first.
    ///
    /// Nothing here authenticates: a session is named by a surrogate key, and
    /// the token stays where it is. Without this, the only thing a person could
    /// do about a session they did not recognise was change their password and
    /// end all of them, including the ones they wanted to keep.
    pub async fn list_sessions(
        &self,
        user_id: &str,
        current_token: &str,
    ) -> Result<Vec<crate::security::SessionSummary>, AuthError> {
        self.session_manager()
            .list_sessions_for_user(user_id, current_token)
            .await
    }

    /// End one session, and the refresh token that would mint it again.
    ///
    /// The second half is what makes this mean something for an API session. A
    /// session carrying an audience came from the token endpoint, and the
    /// client that holds it usually holds a refresh token too — deleting the
    /// session alone buys the length of one access token before the client
    /// quietly mints another. A browser session has no audience and no refresh
    /// family, so nothing else goes with it.
    ///
    /// `false` means this account has no such session, which is also the answer
    /// for an id belonging to somebody else.
    pub async fn revoke_session(
        &self,
        user_id: &str,
        id: uuid::Uuid,
        current_token: &str,
    ) -> Result<bool, AuthError> {
        let Some(session) = self
            .session_manager()
            .delete_session_for_user(user_id, id, current_token)
            .await?
        else {
            return Ok(false);
        };

        if let Some(audience) = session.audience.as_deref() {
            self.revoke_refresh_tokens(user_id, Some(audience)).await;
        }

        Ok(true)
    }

    /// End every session but the one asking, and every refresh token the
    /// account holds.
    ///
    /// "Everywhere else" has to include the ways back in that no session list
    /// shows. A refresh token is one of those: it is not a session, it does not
    /// appear in the list, and it mints sessions on demand. A client acting for
    /// this account will have to be authorized again, which is the point.
    pub async fn revoke_other_sessions(
        &self,
        user_id: &str,
        current_token: &str,
    ) -> Result<u64, AuthError> {
        let ended = self
            .session_manager()
            .delete_other_sessions_for_user(user_id, current_token)
            .await?;

        self.revoke_refresh_tokens(user_id, None).await;

        Ok(ended)
    }

    /// Drop refresh tokens, for one audience or for the whole account.
    ///
    /// Failure is logged rather than returned: the sessions are already gone,
    /// and answering with an error would tell somebody their sessions are still
    /// alive when they are not.
    async fn revoke_refresh_tokens(&self, user_id: &str, audience: Option<&str>) {
        let Some(db) = crate::database::get_global_database() else {
            tracing::error!(
                "Could not revoke refresh tokens for {}: no database",
                user_id
            );
            return;
        };

        let revoked = match audience {
            Some(audience) => {
                crate::auth::refresh_tokens::revoke_for_user_audience(db.pool(), user_id, audience)
                    .await
            }
            None => crate::auth::refresh_tokens::revoke_for_user(db.pool(), user_id).await,
        };

        match revoked {
            Ok(0) => {}
            Ok(count) => tracing::info!("Revoked {} refresh token(s) for {}", count, user_id),
            Err(e) => tracing::error!(
                "Sessions for {} were ended but their refresh tokens were not: {}",
                user_id,
                e
            ),
        }
    }

    /// Issue a fresh set of recovery codes, returning the only copy of them.
    ///
    /// Asks for the current password even though the caller holds a session,
    /// for the reason [`Self::change_local_password`] does: a set of recovery
    /// codes is a second way into the account, and a session someone else got
    /// hold of must not be able to mint one that outlives the owner changing
    /// their password.
    ///
    /// Replaces whatever set the account had. Reissuing because the old codes
    /// were seen by somebody has to actually take them away.
    pub async fn issue_recovery_codes(
        &self,
        user_id: &str,
        current_password: &str,
        ip_addr: &str,
    ) -> Result<Vec<String>, AuthError> {
        if !self.config.internal.enabled {
            return Err(AuthError::LocalAuthDisabled);
        }

        if !self.config.internal.allow_recovery_codes {
            return Err(AuthError::RecoveryCodesDisabled);
        }

        if !self.security_context.check_auth_rate_limit(ip_addr).await {
            return Err(AuthError::RateLimitExceeded);
        }

        crate::auth::local::verify_user_password(user_id, current_password).await?;
        crate::auth::local::issue_recovery_codes(user_id).await
    }

    /// Spend a recovery code: set a new password, end every session the account
    /// had, and sign the caller in.
    ///
    /// The way back for someone who has forgotten their password on a solution
    /// they merely use. `--set-password` answers the operator's case and cannot
    /// answer this one, and these accounts carry no verified address for a
    /// reset link to go to.
    ///
    /// Throttled the way [`Self::login_local`] is, per address and per account,
    /// because this is a second credential accepting guesses at the same
    /// account — and only failures spend the account's budget, so nobody can
    /// lock somebody out of their own recovery by burning it for them.
    ///
    /// Ending the account's sessions is not optional here. Recovery is the path
    /// somebody takes when they have lost control of the account, and the
    /// sessions that exist at that moment may be exactly the ones they are
    /// trying to be rid of.
    pub async fn recover_local_account(
        &self,
        username: &str,
        code: &str,
        new_password: &str,
        ip_addr: &str,
        user_agent: &str,
    ) -> Result<String, AuthError> {
        if !self.config.internal.enabled {
            return Err(AuthError::LocalAuthDisabled);
        }

        if !self.config.internal.allow_recovery_codes {
            return Err(AuthError::RecoveryCodesDisabled);
        }

        if !self.security_context.check_auth_rate_limit(ip_addr).await {
            return Err(AuthError::RateLimitExceeded);
        }

        if !self.security_context.account_login_allowed(username).await {
            return Err(AuthError::RateLimitExceeded);
        }

        let user_id = match crate::auth::local::redeem_recovery_code(
            username,
            code,
            new_password,
            self.config.internal.min_password_length,
        )
        .await
        {
            Ok(user_id) => user_id,
            Err(e) => {
                // A password this engine would refuse is the caller mistyping,
                // not somebody guessing; spending the account's budget on it
                // would let a person lock themselves out of their own recovery
                // with a short password.
                if !matches!(e, AuthError::WeakPassword(_)) {
                    self.security_context
                        .record_account_login_failure(username)
                        .await;
                    self.security_context
                        .log_auth_failure(
                            crate::auth::local::LOCAL_PROVIDER,
                            "Invalid recovery code",
                            Some(ip_addr),
                        )
                        .await;
                }
                return Err(e);
            }
        };

        if let Err(e) = self
            .session_manager()
            .delete_all_sessions_for_user(&user_id)
            .await
        {
            // The password is already changed. Say so loudly rather than
            // reporting a failure that would send someone back through
            // recovery to spend a second code.
            tracing::error!(
                "Password for {} was recovered but their sessions could not be ended: {}",
                user_id,
                e
            );
        }

        crate::user_repository::apply_bootstrap_admin_username(
            &user_id,
            &crate::auth::local::normalize_username(username),
        )
        .await
        .map_err(|e| AuthError::Internal(format!("Failed to apply configured role: {}", e)))?;

        self.establish_session(SessionRequest {
            user_id,
            provider: crate::auth::local::LOCAL_PROVIDER.to_string(),
            email: None,
            name: None,
            ip_addr: ip_addr.to_string(),
            user_agent: user_agent.to_string(),
            refresh_token: None,
        })
        .await
    }

    /// Attach a credential to an account that has none, keeping its `user_id`.
    ///
    /// The reason a guest is worth offering at all: whatever they built up
    /// while anonymous is still theirs once they have a way back in. The caller
    /// must already hold a session for `user_id` — this grants no new identity,
    /// it gives an existing one a way to sign in again.
    pub async fn claim_guest_account(
        &self,
        user_id: &str,
        username: &str,
        password: &str,
        ip_addr: &str,
    ) -> Result<String, AuthError> {
        if !self.config.internal.enabled {
            return Err(AuthError::LocalAuthDisabled);
        }

        if !self.security_context.check_auth_rate_limit(ip_addr).await {
            return Err(AuthError::RateLimitExceeded);
        }

        crate::auth::local::attach_credential(
            user_id,
            username,
            password,
            self.config.internal.min_password_length,
        )
        .await
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
        host: Option<&str>,
    ) -> Result<String, AuthError> {
        self.session_manager
            .get_session(session_token, ip_addr, user_agent, &realm_host(host))
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
        host: Option<&str>,
    ) -> Result<crate::auth::session::AuthSession, AuthError> {
        let host = realm_host(host);
        let session_data = self
            .session_manager
            .get_session_data(session_token, ip_addr, user_agent, &host)
            .await?;

        let seconds_until_expiry = (session_data.expires_at - Utc::now()).num_seconds();
        if seconds_until_expiry > SESSION_REFRESH_WINDOW_SECONDS {
            return Ok(session_data.into());
        }

        // Providers are used for identity only — the engine never calls a provider API
        // on the user's behalf, and no provider access token is retained past login
        // (see `SessionData`, which stores no access token). Session lifetime is
        // therefore governed solely by our own sliding timeout and absolute max age,
        // and renewal never contacts the IdP. Contacting it here bought nothing but a
        // per-renewal round trip that could destroy an otherwise valid session when a
        // provider expired the refresh token on its own schedule.
        self.session_manager
            .refresh_session(session_token, ip_addr, user_agent, &host, None)
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
        host: Option<&str>,
        resource: Option<&str>,
    ) -> Result<crate::auth::session::AuthSession, AuthError> {
        self.session_manager
            .validate_session_with_resource(
                session_token,
                ip_addr,
                user_agent,
                &realm_host(host),
                resource,
            )
            .await
    }

    /// Validate API key
    ///
    /// # Arguments
    /// * `api_key` - API key to validate
    ///
    /// # Returns
    /// true if API key is valid
    pub fn validate_api_key(&self, api_key: &str) -> bool {
        if let Some(configured_key) = &self.api_key {
            // Use constant time comparison to prevent timing attacks
            use subtle::ConstantTimeEq;
            let configured_bytes = configured_key.as_bytes();
            let provided_bytes = api_key.as_bytes();

            if configured_bytes.len() != provided_bytes.len() {
                return false;
            }

            configured_bytes.ct_eq(provided_bytes).into()
        } else {
            false
        }
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
        let pool = sqlx::PgPool::connect_lazy(
            "postgresql://aiwebengine:devpassword@localhost:5432/aiwebengine",
        )
        .unwrap();

        // Create security infrastructure
        let auditor = Arc::new(SecurityAuditor::new(Some(pool.clone())));
        let rate_limiter =
            Arc::new(RateLimiter::new(pool.clone()).with_security_auditor(Arc::clone(&auditor)));
        let threat_config = ThreatDetectionConfig::default();
        let _threat_detector = Arc::new(ThreatDetector::new(Some(pool.clone()), threat_config));
        let csrf_key: [u8; 32] = *b"test-csrf-secret-key-32-bytes!!!";
        let csrf = Arc::new(CsrfProtection::new(csrf_key, 3600));
        let encryption_key: [u8; 32] = *b"test-encryption-key-32-bytes!!!!";
        let encryption = Arc::new(DataEncryption::new(&encryption_key));

        let session_mgr = SecureSessionManager::new(
            pool.clone(),
            &encryption_key,
            3600,
            86400 * 30,
            10,
            Arc::clone(&auditor),
        )
        .unwrap();
        let session_mgr = Arc::new(session_mgr);

        let auth_session_mgr = Arc::new(AuthSessionManager::new(session_mgr));

        let security_context = Arc::new(AuthSecurityContext::new(
            Arc::clone(&auditor),
            rate_limiter,
            csrf,
            encryption,
        ));

        AuthManager::new(config, auth_session_mgr, security_context, None)
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
        let result = manager.start_login("nonexistent", "127.0.0.1", None).await;
        assert!(matches!(result, Err(AuthError::UnsupportedProvider(_))));
    }

    fn test_provider_config(redirect_uri: &str) -> OAuth2ProviderConfig {
        OAuth2ProviderConfig {
            client_id: "test-client".to_string(),
            client_secret: "test-secret".to_string(),
            scopes: vec!["openid".to_string(), "email".to_string()],
            redirect_uri: redirect_uri.to_string(),
            auth_url: None,
            token_url: None,
            userinfo_url: None,
            extra_params: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn test_host_provider_selects_matching_redirect_uri() {
        let mut manager = create_test_manager().await;
        manager
            .register_provider(
                "google",
                test_provider_config("https://example.com/callback"),
            )
            .expect("default registration should succeed");
        manager
            .register_provider_for_host(
                "manage.example.com",
                "google",
                test_provider_config("https://manage.example.com/callback"),
            )
            .expect("host registration should succeed");

        let (auth_url, _) = manager
            .start_login("google", "127.0.0.1", Some("manage.example.com"))
            .await
            .expect("login should start");
        assert!(
            auth_url.contains("manage.example.com%2Fcallback"),
            "authorization URL should carry the manage host redirect URI: {}",
            auth_url
        );
    }

    #[tokio::test]
    async fn test_host_lookup_is_case_insensitive() {
        let mut manager = create_test_manager().await;
        manager
            .register_provider_for_host(
                "Manage.Example.Com",
                "google",
                test_provider_config("https://manage.example.com/callback"),
            )
            .expect("host registration should succeed");

        assert!(
            manager
                .get_provider_for_host(Some("MANAGE.example.com"), "google")
                .is_some()
        );
        assert_eq!(manager.hosts_with_providers(), vec!["manage.example.com"]);
    }

    #[tokio::test]
    async fn test_unknown_host_falls_back_to_default_provider() {
        let mut manager = create_test_manager().await;
        manager
            .register_provider(
                "google",
                test_provider_config("https://example.com/callback"),
            )
            .expect("default registration should succeed");
        manager
            .register_provider_for_host(
                "manage.example.com",
                "google",
                test_provider_config("https://manage.example.com/callback"),
            )
            .expect("host registration should succeed");

        // A Host header naming somewhere we never registered must not steer the
        // flow anywhere new — it gets the configured base URL's provider.
        let (auth_url, _) = manager
            .start_login("google", "127.0.0.1", Some("attacker.test"))
            .await
            .expect("login should start");
        assert!(
            auth_url.contains("example.com%2Fcallback")
                && !auth_url.contains("attacker.test")
                && !auth_url.contains("manage.example.com"),
            "unknown host should fall back to the default redirect URI: {}",
            auth_url
        );
    }

    /// The prefix is what makes the host-only scoping enforceable by the
    /// browser rather than by every future handler remembering not to set a
    /// `Domain`.
    #[test]
    fn a_secure_cookie_is_host_scoped() {
        assert_eq!(
            host_scoped_cookie_name("aiwebengine_session", true),
            "__Host-aiwebengine_session"
        );
    }

    /// Over plain HTTP the browser discards a `__Host-` cookie, so applying the
    /// prefix there would break sign-in on a development machine instead of
    /// hardening it.
    #[test]
    fn an_insecure_cookie_keeps_the_bare_name() {
        assert_eq!(
            host_scoped_cookie_name("aiwebengine_session", false),
            "aiwebengine_session"
        );
    }

    /// A production config copied to a dev machine names the cookie with the
    /// prefix already on it. Honouring that verbatim over HTTP would leave the
    /// engine setting a cookie no browser stores.
    #[test]
    fn an_insecure_cookie_sheds_a_configured_prefix() {
        assert_eq!(
            host_scoped_cookie_name("__Host-aiwebengine_session", false),
            "aiwebengine_session"
        );
    }

    /// Deriving the name twice must not stack prefixes — the cookie the browser
    /// stores and the one the engine reads back have to be the same string.
    #[test]
    fn deriving_the_name_is_idempotent() {
        let once = host_scoped_cookie_name("aiwebengine_session", true);
        assert_eq!(host_scoped_cookie_name(&once, true), once);
    }
}
