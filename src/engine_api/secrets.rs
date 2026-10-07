//! Managing a script's secrets.

use super::*;
use crate::repository;
use crate::security::UserContext;
use serde_json::{Value, json};

/// Why a secret operation was rejected.
#[derive(Debug)]
pub enum SecretAccessError {
    AccessDenied,
    Validation(String),
    Storage(String),
}

/// Cross-script secret management: admins and owners of the target script may
/// manage its secret keys. Secret values are write-only through this surface —
/// there is deliberately no read-value operation.
pub(super) fn can_manage_secrets(user: &UserContext, script_uri: &str) -> bool {
    is_admin_or_owner(user, script_uri)
}

/// List secret keys (not values) stored for a script.
pub fn list_secrets_authorized(
    user: &UserContext,
    script_uri: &str,
) -> Result<Vec<String>, SecretAccessError> {
    if !can_manage_secrets(user, script_uri) {
        return Err(SecretAccessError::AccessDenied);
    }
    Ok(repository::list_script_secrets(script_uri).unwrap_or_default())
}

/// Store a secret for a script.
pub fn set_secret_authorized(
    user: &UserContext,
    script_uri: &str,
    key: &str,
    value: &str,
) -> Result<(), SecretAccessError> {
    if !can_manage_secrets(user, script_uri) {
        return Err(SecretAccessError::AccessDenied);
    }
    if key.trim().is_empty() {
        return Err(SecretAccessError::Validation(
            "Key cannot be empty".to_string(),
        ));
    }
    if value.len() > 1_000_000 {
        return Err(SecretAccessError::Validation(
            "Value too large (>1MB)".to_string(),
        ));
    }
    repository::set_script_secret_item(script_uri, key, value)
        .map_err(|e| SecretAccessError::Storage(format!("{}", e)))
}

/// Remove one secret from a script. Returns whether the key existed.
pub fn remove_secret_authorized(
    user: &UserContext,
    script_uri: &str,
    key: &str,
) -> Result<bool, SecretAccessError> {
    if !can_manage_secrets(user, script_uri) {
        return Err(SecretAccessError::AccessDenied);
    }
    Ok(repository::remove_script_secret_item(script_uri, key))
}

/// Remove all secrets stored for a script.
pub fn clear_secrets_authorized(
    user: &UserContext,
    script_uri: &str,
) -> Result<(), SecretAccessError> {
    if !can_manage_secrets(user, script_uri) {
        return Err(SecretAccessError::AccessDenied);
    }
    repository::clear_script_secrets(script_uri)
        .map_err(|e| SecretAccessError::Storage(format!("{}", e)))
}

pub(super) fn secret_error_json(error: SecretAccessError) -> Value {
    match error {
        SecretAccessError::AccessDenied => refuse(
            Refusal::Forbidden,
            "Permission denied. You must be an administrator or owner of the script",
        ),
        SecretAccessError::Validation(details) => refuse(Refusal::BadRequest, details),
        SecretAccessError::Storage(details) => refuse(Refusal::Failed, details),
    }
}

pub(super) fn tool_list_secrets(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    match list_secrets_authorized(user, script) {
        Ok(keys) => json!({
            "script": script,
            "keys": keys,
            "count": keys.len(),
            "timestamp": iso_timestamp(),
        }),
        Err(error) => secret_error_json(error),
    }
}

pub(super) fn tool_write_secret(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let Some(key) = arg_str(args, "key") else {
        return missing_arg("key");
    };
    let Some(value) = arg_str(args, "value") else {
        return missing_arg("value");
    };
    match set_secret_authorized(user, script, key, value) {
        Ok(()) => json!({
            "success": true,
            "script": script,
            "key": key,
            "timestamp": iso_timestamp(),
        }),
        Err(error) => secret_error_json(error),
    }
}

pub(super) fn tool_delete_secret(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let Some(key) = arg_str(args, "key") else {
        return missing_arg("key");
    };
    match remove_secret_authorized(user, script, key) {
        Ok(true) => json!({
            "success": true,
            "script": script,
            "key": key,
            "timestamp": iso_timestamp(),
        }),
        Ok(false) => refuse(
            Refusal::NotFound,
            format!("Secret '{}' not found for script '{}'", key, script),
        ),
        Err(error) => secret_error_json(error),
    }
}

pub(super) fn tool_clear_secrets(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    match clear_secrets_authorized(user, script) {
        Ok(()) => json!({
            "success": true,
            "cleared": true,
            "script": script,
            "timestamp": iso_timestamp(),
        }),
        Err(error) => secret_error_json(error),
    }
}
