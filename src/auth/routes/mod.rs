//! The `/auth/*` routes: signing in, the account page, sessions, delegation
//! consent, elevation, and the engine's own OAuth 2.1 authorization server
//! (`oauth2.rs`). The routers are here; each flow is its own file.
use crate::auth::AuthManager;
use crate::auth::client_registration::{ClientRegistrationManager, register_client_handler};
use crate::auth::metadata::{
    MetadataConfig, metadata_handler, protected_resource_metadata_handler,
};
use axum::{
    Router,
    routing::{get, post},
};
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};

use sqlx::PgPool;
mod account;
mod common;
mod delegation;
mod elevation;
mod login;
mod oauth2;
mod session;
pub use account::*;
pub use common::*;
pub use delegation::*;
pub use elevation::*;
pub use login::*;
pub use oauth2::*;
pub use session::*;

/// Create authentication router with all routes
pub fn create_auth_router(auth_manager: Arc<AuthManager>) -> Router {
    Router::new()
        .route("/login", get(login_page))
        .route("/account", get(account_page))
        .route("/login/{provider}", get(start_login))
        .route("/callback/{provider}", get(oauth_callback))
        .route("/guest", post(start_guest))
        .route("/local/register", post(register_local))
        .route("/local/login", post(login_local))
        .route("/local/password", post(change_password_route))
        .route("/local/claim", post(claim_account))
        .route("/local/recovery_codes", post(recovery_codes_route))
        .route("/local/recover", post(recover_account))
        .route("/sessions", get(list_sessions_route))
        .route("/sessions/revoke", post(revoke_session_route))
        .route("/delegate", get(delegate_page).post(delegate_route))
        .route("/elevate", get(elevate_page).post(elevate_route))
        .route("/elevate/drop", post(elevate_drop_route))
        .route("/delegations/revoke", post(revoke_delegation_route))
        .route("/delegations/unlink", post(unlink_sender_route))
        .route("/logout", get(logout).post(logout))
        .route("/refresh", post(refresh_session))
        .route("/status", get(auth_status))
        .with_state(auth_manager)
}

/// Create OAuth2 metadata and registration router
pub fn create_oauth2_router(
    metadata_config: Arc<MetadataConfig>,
    registration_manager: Option<Arc<ClientRegistrationManager>>,
    auth_manager: Arc<AuthManager>,
    pool: PgPool,
) -> Router {
    let metadata_router = Router::new()
        .route(
            "/.well-known/oauth-authorization-server",
            get(metadata_handler),
        )
        .route(
            crate::auth::metadata::PROTECTED_RESOURCE_PATH,
            get(protected_resource_metadata_handler),
        )
        .route(
            &format!(
                "{}/{{*resource}}",
                crate::auth::metadata::PROTECTED_RESOURCE_PATH
            ),
            get(protected_resource_metadata_handler),
        )
        .with_state(metadata_config.clone());

    // Add OAuth 2.0 protocol endpoints
    // Enable CORS for token endpoint to allow MCP clients on localhost
    let cors = CorsLayer::new()
        .allow_origin(Any) // Allow requests from any origin (needed for localhost MCP clients)
        .allow_methods([
            axum::http::Method::GET,
            axum::http::Method::POST,
            axum::http::Method::OPTIONS,
        ])
        .allow_headers(Any);

    // Taken before the manager is moved into the OAuth2 state: registration is
    // unauthenticated, so its per-address budget is the only thing bounding it.
    let registration_security = auth_manager.security_context();

    let oauth2_state = OAuth2State::new(auth_manager, pool, metadata_config);

    let oauth2_protocol_router = Router::new()
        // Under the reserved /auth prefix, and advertised in the authorization
        // server metadata. Clients discover these per RFC 8414.
        .route(AUTHORIZE_PATH, get(oauth2_authorize))
        .route(CONSENT_PATH, post(oauth2_consent))
        .route(TOKEN_PATH, post(oauth2_token))
        .layer(cors)
        .with_state(oauth2_state);

    // Add dynamic client registration endpoint if enabled
    let router = metadata_router.merge(oauth2_protocol_router);

    if let Some(manager) = registration_manager {
        let registration_router = Router::new()
            .route(REGISTRATION_PATH, post(register_client_handler))
            .with_state(crate::auth::client_registration::ClientRegistrationState {
                manager,
                security: registration_security,
            });

        router.merge(registration_router)
    } else {
        router
    }
}

#[cfg(test)]
mod tests;
