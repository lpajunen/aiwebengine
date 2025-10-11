// Authentication Module
// Provides OAuth2/OIDC authentication with session management

pub mod config;
pub mod error;
pub mod security;
pub mod session;

// Future modules (to be implemented in later phases)
// pub mod middleware;
// pub mod providers;
// pub mod routes;
// pub mod js_api;

pub use config::{AuthConfig, ProviderConfig, ProvidersConfig};
pub use error::AuthError;
pub use security::AuthSecurityContext;
pub use session::{AuthSession, AuthSessionManager};

// Re-export security types commonly used in auth
pub use crate::security::{
    CsrfProtection, DataEncryption, OAuthStateManager, SessionData,
    SessionToken, UserContext,
};
