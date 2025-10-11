// Authentication Module
// Provides OAuth2/OIDC authentication with session management

pub mod config;
pub mod error;
pub mod manager;
pub mod middleware;
pub mod providers;
pub mod routes;
pub mod security;
pub mod session;

// Future modules (to be implemented in later phases)
// pub mod js_api;

pub use config::{AuthConfig, ProviderConfig, ProvidersConfig};
pub use error::AuthError;
pub use manager::{AuthManager, AuthManagerConfig, AuthenticatedUser, CookieSameSite};
pub use middleware::{optional_auth_middleware, required_auth_middleware, AuthUser, AuthenticatedUser as AuthUserExtractor};
pub use providers::{OAuth2Provider, OAuth2ProviderConfig, OAuth2TokenResponse, OAuth2UserInfo, ProviderFactory};
pub use routes::create_auth_router;
pub use security::AuthSecurityContext;
pub use session::{AuthSession, AuthSessionManager};

// Re-export security types commonly used in auth
pub use crate::security::{
    CsrfProtection, DataEncryption, OAuthStateManager, SessionData,
    SessionToken, UserContext,
};
