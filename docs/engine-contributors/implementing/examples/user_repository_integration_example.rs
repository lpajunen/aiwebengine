// Example integration of user_repository with OAuth authentication
// This shows how to modify src/auth/routes.rs to use the user repository

use crate::auth::{AuthError, AuthManager};
use crate::user_repository;
use axum::{
    extract::{ConnectInfo, Query, State},
    response::{IntoResponse, Redirect},
};
use std::net::SocketAddr;
use std::sync::Arc;

// OAuth callback parameters
#[derive(serde::Deserialize)]
struct CallbackParams {
    code: String,
    state: String,
}

// Example of updated OAuth callback handler
async fn handle_oauth_callback(
    State(auth_manager): State<Arc<AuthManager>>,
    Query(params): Query<CallbackParams>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> Result<impl IntoResponse, AuthError> {
    let provider_name = "google"; // Extract from path or state
    let ip_addr = addr.ip().to_string();
    
    // 1. Validate OAuth state (CSRF protection)
    auth_manager
        .validate_oauth_state(&params.state, &ip_addr)
        .await?;
    
    // 2. Exchange authorization code for tokens
    let token_response = auth_manager
        .exchange_code(provider_name, &params.code)
        .await?;
    
    // 3. Get user information from OAuth provider
    let user_info = auth_manager
        .get_user_info(provider_name, &token_response.access_token)
        .await?;
    
    // 4. UPSERT USER TO REPOSITORY
    // This is the key integration point!
    let user_id = user_repository::upsert_user(
        user_info.email.clone().unwrap_or_default(),
        user_info.name.clone(),
        provider_name.to_string(),
        user_info.id.clone(), // Provider-specific user ID
    )
    .map_err(|e| {
        AuthError::Internal(format!("Failed to upsert user: {}", e))
    })?;
    
    // 5. Get user from repository to check roles
    let user = user_repository::get_user(&user_id).map_err(|e| {
        AuthError::Internal(format!("Failed to get user: {}", e))
    })?;
    
    // Check if user has administrator role
    let is_admin = user.has_role(&user_repository::UserRole::Administrator);
    
    // 6. Create session with repository user ID and roles
    let session_token = auth_manager
        .create_session(
            user_id.clone(), // Use repository-generated user ID
            provider_name.to_string(),
            user_info.email,
            user_info.name,
            is_admin, // Set admin flag from repository role
            ip_addr,
            "Mozilla/5.0".to_string(), // Extract from headers
        )
        .await?;
    
    // 7. Set session cookie and redirect
    let cookie = format!(
        "session={}; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=604800",
        session_token.token
    );
    
    Ok((
        axum::http::StatusCode::SEE_OTHER,
        [
            (axum::http::header::SET_COOKIE, cookie),
            (axum::http::header::LOCATION, "/".to_string()),
        ],
        "Redirecting...",
    ))
}

// Example of a role management endpoint (admin only)
async fn update_user_role(
    auth_user: crate::auth::AuthUser, // From middleware
    axum::Json(payload): axum::Json<UpdateRoleRequest>,
) -> Result<impl IntoResponse, AuthError> {
    // 1. Check if requester has administrator privileges
    if !auth_user.is_admin {
        return Err(AuthError::Forbidden(
            "Only administrators can modify user roles".to_string(),
        ));
    }
    
    // 2. Get target user
    let target_user = user_repository::get_user(&payload.user_id).map_err(|e| {
        AuthError::Internal(format!("Failed to get user: {}", e))
    })?;
    
    // 3. Update role based on action
    match payload.action.as_str() {
        "add" => {
            user_repository::add_user_role(
                &payload.user_id,
                payload.role.parse().map_err(|_| {
                    AuthError::InvalidRequest("Invalid role".to_string())
                })?,
            )
            .map_err(|e| {
                AuthError::Internal(format!("Failed to add role: {}", e))
            })?;
        }
        "remove" => {
            let role = payload.role.parse().map_err(|_| {
                AuthError::InvalidRequest("Invalid role".to_string())
            })?;
            user_repository::remove_user_role(&payload.user_id, &role).map_err(|e| {
                AuthError::Internal(format!("Failed to remove role: {}", e))
            })?;
        }
        _ => {
            return Err(AuthError::InvalidRequest(
                "Invalid action. Use 'add' or 'remove'".to_string(),
            ))
        }
    }
    
    Ok(axum::Json(serde_json::json!({
        "success": true,
        "user_id": payload.user_id,
        "action": payload.action,
        "role": payload.role,
    })))
}

#[derive(serde::Deserialize)]
struct UpdateRoleRequest {
    user_id: String,
    action: String, // "add" or "remove"
    role: String,   // "Editor" or "Administrator"
}

// Helper to parse role string
impl std::str::FromStr for user_repository::UserRole {
    type Err = String;
    
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "authenticated" => Ok(user_repository::UserRole::Authenticated),
            "editor" => Ok(user_repository::UserRole::Editor),
            "administrator" | "admin" => Ok(user_repository::UserRole::Administrator),
            _ => Err(format!("Unknown role: {}", s)),
        }
    }
}

// Example: List all users (admin only)
async fn list_all_users(
    auth_user: crate::auth::AuthUser,
) -> Result<impl IntoResponse, AuthError> {
    // Check admin privileges
    if !auth_user.is_admin {
        return Err(AuthError::Forbidden(
            "Only administrators can list users".to_string(),
        ));
    }
    
    // Get all users from repository
    let users = user_repository::list_users().map_err(|e| {
        AuthError::Internal(format!("Failed to list users: {}", e))
    })?;
    
    // Convert to JSON response
    let user_list: Vec<_> = users
        .iter()
        .map(|user| {
            serde_json::json!({
                "id": user.id,
                "email": user.email,
                "name": user.name,
                "roles": user.roles.iter().map(|r| format!("{:?}", r)).collect::<Vec<_>>(),
                "created_at": format!("{:?}", user.created_at),
                "providers": user.providers.iter().map(|p| &p.provider_name).collect::<Vec<_>>(),
            })
        })
        .collect();
    
    Ok(axum::Json(serde_json::json!({
        "users": user_list,
        "total": users.len(),
    })))
}

// Example: Get current user profile
async fn get_current_user(
    auth_user: crate::auth::AuthUser,
) -> Result<impl IntoResponse, AuthError> {
    // Get full user data from repository
    let user = user_repository::get_user(&auth_user.user_id).map_err(|e| {
        AuthError::Internal(format!("Failed to get user: {}", e))
    })?;
    
    Ok(axum::Json(serde_json::json!({
        "id": user.id,
        "email": user.email,
        "name": user.name,
        "roles": user.roles.iter().map(|r| format!("{:?}", r)).collect::<Vec<_>>(),
        "is_admin": user.has_role(&user_repository::UserRole::Administrator),
        "is_editor": user.has_role(&user_repository::UserRole::Editor),
        "created_at": format!("{:?}", user.created_at),
        "providers": user.providers.iter().map(|p| serde_json::json!({
            "provider": p.provider_name,
            "first_auth": format!("{:?}", p.first_auth_at),
            "last_auth": format!("{:?}", p.last_auth_at),
        })).collect::<Vec<_>>(),
    })))
}

// Router setup example
pub fn create_user_management_router() -> axum::Router {
    use axum::routing::{get, post};
    
    axum::Router::new()
        .route("/me", get(get_current_user))
        .route("/list", get(list_all_users))
        .route("/role", post(update_user_role))
    // Note: These routes should be protected by authentication middleware
}
