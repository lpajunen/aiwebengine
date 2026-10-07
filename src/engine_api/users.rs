//! Administering accounts: roles and realms.

use super::*;
use crate::security::{
    Capability, SecurityEvent, SecurityEventType, SecuritySeverity, UserContext,
};
use serde_json::{Value, json};

// ----------------------------------------------------------------------------
// User administration
// ----------------------------------------------------------------------------

/// Why a user-administration operation was rejected.
#[derive(Debug)]
pub enum UserAdminError {
    AccessDenied,
    Validation(String),
    UserNotFound(String),
    LastAdministrator,
    Storage(String),
}

/// User administration is restricted to session-verified administrators.
///
/// Deliberately stricter than [`has_admin_capability`], which accepts any
/// holder of `AdministerEngine` however it was obtained. Requiring
/// authentication as well means only a `UserContext::admin` passes, and that is
/// built solely from a session whose `is_admin` flag is set (`lib.rs`), for both
/// the HTTP and MCP entry points — so no future widening of what an anonymous
/// caller holds can reach the user directory or role changes.
pub(super) fn is_user_admin(user: &UserContext) -> bool {
    user.is_authenticated && user.has_capability(&Capability::AdministerEngine)
}

/// Record an authorization failure against the user-administration surface.
pub(super) fn audit_user_admin_denied(user: &UserContext, action: &str) {
    let auditor = auditor();
    let user_id = user.user_id.clone();
    let action = action.to_string();
    tokio::task::spawn(async move {
        let _ = auditor
            .log_authz_failure(
                user_id,
                "user".to_string(),
                action,
                "Administrator".to_string(),
            )
            .await;
    });
}

/// Record a completed role change. Role changes are privilege escalations or
/// revocations, so they are logged at high severity like script deletion.
pub(super) fn audit_role_change(
    actor: &UserContext,
    target_user_id: &str,
    role: &str,
    action: &str,
) {
    let auditor = auditor();
    let actor_id = actor.user_id.clone();
    let (target, role, action) = (
        target_user_id.to_string(),
        role.to_string(),
        action.to_string(),
    );
    tokio::task::spawn(async move {
        let _ = auditor
            .log_event(
                SecurityEvent::new(
                    SecurityEventType::SystemSecurityEvent,
                    SecuritySeverity::High,
                    actor_id,
                )
                .with_resource("user".to_string())
                .with_action(action)
                .with_detail("target_user", &target)
                .with_detail("role", &role),
            )
            .await;
    });
}

/// Parse a role name accepted by the role endpoints.
pub(super) fn parse_user_role(
    role: &str,
) -> Result<crate::user_repository::UserRole, UserAdminError> {
    use crate::user_repository::UserRole;
    match role {
        "Authenticated" => Ok(UserRole::Authenticated),
        "Editor" => Ok(UserRole::Editor),
        "Administrator" => Ok(UserRole::Administrator),
        other => Err(UserAdminError::Validation(format!(
            "Invalid role: {}. Must be Editor, Administrator, or Authenticated",
            other
        ))),
    }
}

/// Look up a user, distinguishing "no such user" from a storage failure so the
/// caller can answer 404 rather than 500.
pub(super) fn lookup_user(user_id: &str) -> Result<crate::user_repository::User, UserAdminError> {
    crate::user_repository::get_user(user_id).map_err(|e| match e {
        // `db_get_user` reports a missing row as a validation error on `user_id`.
        crate::error::AppError::Validation { ref field, .. } if field == "user_id" => {
            UserAdminError::UserNotFound(user_id.to_string())
        }
        other => UserAdminError::Storage(format!("{}", other)),
    })
}

pub(super) fn role_names(user: &crate::user_repository::User) -> Vec<String> {
    user.roles.iter().map(|r| format!("{:?}", r)).collect()
}

pub(super) fn user_to_json(user: &crate::user_repository::User) -> Value {
    let millis = |t: std::time::SystemTime| {
        t.duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as f64
    };
    json!({
        "id": user.id,
        "email": user.email,
        "name": user.name,
        "roles": role_names(user),
        "realm": user.realm,
        "providers": user.providers
            .iter()
            .map(|p| p.provider_name.clone())
            .collect::<Vec<_>>(),
        "createdAt": millis(user.created_at),
        "updatedAt": millis(user.updated_at),
    })
}

/// List every user in the directory; administrators only.
///
/// An unauthorized caller is denied rather than handed an empty array —
/// "denied" and "no users" must not look alike.
pub fn list_users_authorized(user: &UserContext) -> Result<Vec<Value>, UserAdminError> {
    if !is_user_admin(user) {
        audit_user_admin_denied(user, "list");
        return Err(UserAdminError::AccessDenied);
    }
    crate::user_repository::list_users()
        .map(|users| users.iter().map(user_to_json).collect())
        .map_err(|e| UserAdminError::Storage(format!("{}", e)))
}

/// Move a user into a realm; administrators only. Returns the realm as stored.
///
/// The realm is the host an account is a principal on, and `*` means every
/// host. No sign-in path produces `*` — an account anyone can create must not
/// reach every host by existing — so this is how an administrator who needs to
/// work across hosts gets there, and it is deliberately a separate act from
/// granting a role.
///
/// Takes effect immediately: sessions carry the realm they were minted with, so
/// the ones the account already holds are ended by the write.
pub fn set_user_realm_authorized(
    actor: &UserContext,
    user_id: &str,
    realm: &str,
) -> Result<String, UserAdminError> {
    if !is_user_admin(actor) {
        audit_user_admin_denied(actor, "set_realm");
        return Err(UserAdminError::AccessDenied);
    }
    // Establish the user exists before mutating, so a typo'd id reads as 404.
    lookup_user(user_id)?;

    crate::user_repository::set_user_realm(user_id, realm)
        .map_err(|e| UserAdminError::Storage(format!("{}", e)))?;
    audit_role_change(actor, user_id, realm, "set_realm");

    Ok(lookup_user(user_id)?.realm)
}

/// Grant a role to a user; administrators only. Returns the user's resulting
/// role set. Granting a role the user already holds is a no-op, not an error.
pub fn add_user_role_authorized(
    actor: &UserContext,
    user_id: &str,
    role: &str,
) -> Result<Vec<String>, UserAdminError> {
    if !is_user_admin(actor) {
        audit_user_admin_denied(actor, "add_role");
        return Err(UserAdminError::AccessDenied);
    }
    let parsed = parse_user_role(role)?;
    // Establish the user exists before mutating, so a typo'd id reads as 404.
    lookup_user(user_id)?;

    crate::user_repository::add_user_role(user_id, parsed)
        .map_err(|e| UserAdminError::Storage(format!("{}", e)))?;
    audit_role_change(actor, user_id, role, "add_role");

    Ok(role_names(&lookup_user(user_id)?))
}

/// Revoke a role from a user; administrators only. Returns the user's
/// resulting role set.
///
/// Two roles cannot be revoked: `Authenticated` (every user has it by
/// definition) and the last remaining `Administrator` — locking the last
/// administrator out would leave the instance with no way to appoint another,
/// the same reasoning behind the last-owner guard on scripts.
pub fn remove_user_role_authorized(
    actor: &UserContext,
    user_id: &str,
    role: &str,
) -> Result<Vec<String>, UserAdminError> {
    use crate::user_repository::UserRole;

    if !is_user_admin(actor) {
        audit_user_admin_denied(actor, "remove_role");
        return Err(UserAdminError::AccessDenied);
    }
    let parsed = parse_user_role(role)?;
    if matches!(parsed, UserRole::Authenticated) {
        return Err(UserAdminError::Validation(
            "Cannot remove the Authenticated role".to_string(),
        ));
    }
    let target = lookup_user(user_id)?;

    if matches!(parsed, UserRole::Administrator)
        && target.has_role(&UserRole::Administrator)
        && count_administrators()? <= 1
    {
        return Err(UserAdminError::LastAdministrator);
    }

    crate::user_repository::remove_user_role(user_id, &parsed)
        .map_err(|e| UserAdminError::Storage(format!("{}", e)))?;
    audit_role_change(actor, user_id, role, "remove_role");

    Ok(role_names(&lookup_user(user_id)?))
}

pub(super) fn count_administrators() -> Result<usize, UserAdminError> {
    use crate::user_repository::UserRole;
    crate::user_repository::list_users()
        .map(|users| {
            users
                .iter()
                .filter(|u| u.has_role(&UserRole::Administrator))
                .count()
        })
        .map_err(|e| UserAdminError::Storage(format!("{}", e)))
}

pub(super) fn user_admin_error_json(error: UserAdminError) -> Value {
    match error {
        UserAdminError::AccessDenied => refuse(
            Refusal::Forbidden,
            "Permission denied. Administrator privileges are required",
        ),
        UserAdminError::UserNotFound(id) => {
            refuse(Refusal::NotFound, format!("User not found: {}", id))
        }
        UserAdminError::LastAdministrator => refuse(
            Refusal::Conflict,
            "Cannot remove the last administrator. Grant the Administrator role to another user first.",
        ),
        UserAdminError::Validation(details) => refuse(Refusal::BadRequest, details),
        UserAdminError::Storage(details) => refuse(Refusal::Failed, details),
    }
}

pub(super) fn tool_list_users(_args: &Value, user: &UserContext) -> Value {
    match list_users_authorized(user) {
        Ok(users) => json!({
            "users": users,
            "count": users.len(),
            "timestamp": iso_timestamp(),
        }),
        Err(error) => user_admin_error_json(error),
    }
}

pub(super) fn tool_add_user_role(args: &Value, user: &UserContext) -> Value {
    let Some(user_id) = arg_str(args, "user_id") else {
        return missing_arg("user_id");
    };
    let Some(role) = arg_str(args, "role") else {
        return missing_arg("role");
    };
    match add_user_role_authorized(user, user_id, role) {
        Ok(roles) => json!({
            "success": true,
            "userId": user_id,
            "role": role,
            "roles": roles,
            "timestamp": iso_timestamp(),
        }),
        Err(error) => user_admin_error_json(error),
    }
}

pub(super) fn tool_set_user_realm(args: &Value, user: &UserContext) -> Value {
    let Some(user_id) = arg_str(args, "user_id") else {
        return missing_arg("user_id");
    };
    let Some(realm) = arg_str(args, "realm") else {
        return missing_arg("realm");
    };
    match set_user_realm_authorized(user, user_id, realm) {
        Ok(realm) => json!({
            "success": true,
            "userId": user_id,
            "realm": realm,
            "timestamp": iso_timestamp(),
        }),
        Err(error) => user_admin_error_json(error),
    }
}

pub(super) fn tool_remove_user_role(args: &Value, user: &UserContext) -> Value {
    let Some(user_id) = arg_str(args, "user_id") else {
        return missing_arg("user_id");
    };
    let Some(role) = arg_str(args, "role") else {
        return missing_arg("role");
    };
    match remove_user_role_authorized(user, user_id, role) {
        Ok(roles) => json!({
            "success": true,
            "userId": user_id,
            "role": role,
            "roles": roles,
            "timestamp": iso_timestamp(),
        }),
        Err(error) => user_admin_error_json(error),
    }
}
