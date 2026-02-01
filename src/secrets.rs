//! Secret Management Module
//!
//! This module provides secure secret storage and management for the aiwebengine.
//! Secrets are stored in-memory with thread-safe access and never exposed to JavaScript.
//!
//! # Security Principles
//!
//! 1. Secrets NEVER cross the Rust/JavaScript boundary
//! 2. JavaScript can only check existence or list identifiers
//! 3. Actual values are injected by Rust at point of use (HTTP requests, etc.)
//! 4. All secret access is logged for audit trail (identifier only, never values)
//! 5. Secrets are automatically redacted from logs and error messages

use globset::{Glob, GlobMatcher};
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};
use tracing::{debug, info, warn};

/// Global secrets manager instance
///
/// Initialized once at server startup and shared across all threads.
/// Access via `get_global_secrets_manager()` function.
static GLOBAL_SECRETS_MANAGER: OnceLock<Arc<SecretsManager>> = OnceLock::new();

/// Get the global secrets manager instance
///
/// Returns None if secrets have not been initialized yet (before server startup).
pub fn get_global_secrets_manager() -> Option<Arc<SecretsManager>> {
    GLOBAL_SECRETS_MANAGER.get().cloned()
}

/// Initialize the global secrets manager
///
/// Should be called once during server startup. Subsequent calls are ignored.
///
/// Returns true if this was the first initialization, false if already initialized.
pub fn initialize_global_secrets_manager(manager: Arc<SecretsManager>) -> bool {
    GLOBAL_SECRETS_MANAGER.set(manager).is_ok()
}

/// Secret entry with access control constraints
#[derive(Clone)]
struct SecretEntry {
    /// The actual secret value
    value: String,
    /// Allowed URL pattern (glob) - None means unrestricted
    allowed_url_pattern: Option<GlobMatcher>,
    /// Allowed script URI pattern (glob) - None means unrestricted
    allowed_script_pattern: Option<GlobMatcher>,
}

/// Thread-safe secrets manager
///
/// Stores secrets in memory with read-write lock for concurrent access.
/// Secrets can be loaded from configuration files or environment variables.
#[derive(Clone)]
pub struct SecretsManager {
    secrets: Arc<RwLock<HashMap<String, SecretEntry>>>,
}

impl SecretsManager {
    /// Create a new empty secrets manager
    pub fn new() -> Self {
        Self {
            secrets: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Load secrets from environment variables
    ///
    /// Supports two formats:
    /// 1. Simple: `SECRET_ANTHROPIC_API_KEY` becomes `anthropic_api_key` (unrestricted)
    /// 2. With constraints: `SECRET_NAME__ALLOW_<url_pattern>__SCRIPT_<script_pattern>`
    ///
    /// # Example
    ///
    /// ```bash
    /// # Unrestricted (backward compatible)
    /// export SECRET_ANTHROPIC_API_KEY="sk-ant-api03-..."
    ///
    /// # With URL and script constraints
    /// export SECRET_GITHUB_TOKEN__ALLOW_https://api.github.com/*__SCRIPT_/scripts/integrations/*="ghp_..."
    /// ```
    pub fn load_from_env(&self) {
        let mut count = 0;

        for (key, value) in std::env::vars() {
            if key.starts_with("SECRET_") && !value.is_empty() {
                match Self::parse_secret_env_var(&key, &value) {
                    Ok((secret_id, entry)) => {
                        let mut secrets = self.secrets.write().unwrap();
                        secrets.insert(secret_id.clone(), entry);
                        count += 1;
                        info!(secret_id = %secret_id, "Loaded secret from environment");
                    }
                    Err(e) => {
                        warn!(key = %key, error = %e, "Failed to parse secret environment variable");
                    }
                }
            }
        }

        if count > 0 {
            info!(count = count, "Loaded secrets from environment variables");
        } else {
            debug!("No secrets found in environment variables (looking for SECRET_* prefix)");
        }
    }

    /// Parse secret environment variable into identifier and entry
    fn parse_secret_env_var(key: &str, value: &str) -> Result<(String, SecretEntry), String> {
        let without_prefix = key.strip_prefix("SECRET_").unwrap();

        // Check if this uses the new format with __ALLOW_ and __SCRIPT_
        if let Some(allow_pos) = without_prefix.find("__ALLOW_") {
            // New format: SECRET_NAME__ALLOW_<url_pattern>__SCRIPT_<script_pattern>
            let secret_id = without_prefix[..allow_pos].to_lowercase();
            let after_allow = &without_prefix[allow_pos + 8..]; // Skip "__ALLOW_"

            let script_pos = after_allow
                .find("__SCRIPT_")
                .ok_or_else(|| "Missing __SCRIPT_ in secret definition".to_string())?;

            let url_pattern = &after_allow[..script_pos];
            let script_pattern = &after_allow[script_pos + 9..]; // Skip "__SCRIPT_"

            // Compile glob patterns
            let url_matcher = Glob::new(url_pattern)
                .map_err(|e| format!("Invalid URL pattern '{}': {}", url_pattern, e))?
                .compile_matcher();

            let script_matcher = Glob::new(script_pattern)
                .map_err(|e| format!("Invalid script pattern '{}': {}", script_pattern, e))?
                .compile_matcher();

            Ok((
                secret_id,
                SecretEntry {
                    value: value.to_string(),
                    allowed_url_pattern: Some(url_matcher),
                    allowed_script_pattern: Some(script_matcher),
                },
            ))
        } else {
            // Old format: SECRET_NAME (unrestricted, backward compatible)
            let secret_id = without_prefix.to_lowercase();
            Ok((
                secret_id,
                SecretEntry {
                    value: value.to_string(),
                    allowed_url_pattern: None,
                    allowed_script_pattern: None,
                },
            ))
        }
    }

    /// Load secrets from a HashMap (typically from config file)
    ///
    /// Note: Config file secrets are loaded without constraints (unrestricted).
    /// Use `load_from_toml_file()` for constrained secrets.
    ///
    /// # Arguments
    ///
    /// * `secrets` - HashMap of secret identifiers to values
    pub fn load_from_map(&self, secrets: HashMap<String, String>) {
        let count = secrets.len();

        for (key, value) in secrets {
            if !value.is_empty() {
                self.set(key.clone(), value);
                info!(secret_id = %key, "Loaded secret from configuration");
            }
        }

        if count > 0 {
            info!(count = count, "Loaded secrets from configuration");
        }
    }

    /// Load constrained secrets from a TOML file
    ///
    /// File format:
    /// ```toml
    /// [[secret]]
    /// identifier = "api_key"
    /// value = "secret_value"
    /// allowed_url_pattern = "https://api.example.com/*"
    /// allowed_script_pattern = "/scripts/specific/*"
    /// ```
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the TOML file
    pub fn load_from_toml_file(&self, path: &std::path::Path) -> Result<usize, String> {
        if !path.exists() {
            debug!(path = ?path, "Secrets TOML file does not exist, skipping");
            return Ok(0);
        }

        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("Failed to read secrets file: {}", e))?;

        let config: SecretsTomlConfig =
            toml::from_str(&content).map_err(|e| format!("Failed to parse secrets TOML: {}", e))?;

        let mut count = 0;
        for secret_def in config.secret {
            match self.load_secret_from_toml(secret_def) {
                Ok(identifier) => {
                    count += 1;
                    info!(secret_id = %identifier, "Loaded constrained secret from TOML");
                }
                Err(e) => {
                    warn!(error = %e, "Failed to load secret from TOML");
                }
            }
        }

        if count > 0 {
            info!(count = count, path = ?path, "Loaded constrained secrets from TOML");
        }

        Ok(count)
    }

    fn load_secret_from_toml(&self, def: SecretTomlDefinition) -> Result<String, String> {
        let url_matcher = if let Some(pattern) = def.allowed_url_pattern {
            Some(
                Glob::new(&pattern)
                    .map_err(|e| format!("Invalid URL pattern '{}': {}", pattern, e))?
                    .compile_matcher(),
            )
        } else {
            None
        };

        let script_matcher = if let Some(pattern) = def.allowed_script_pattern {
            Some(
                Glob::new(&pattern)
                    .map_err(|e| format!("Invalid script pattern '{}': {}", pattern, e))?
                    .compile_matcher(),
            )
        } else {
            None
        };

        let entry = SecretEntry {
            value: def.value,
            allowed_url_pattern: url_matcher,
            allowed_script_pattern: script_matcher,
        };

        let identifier = def.identifier.to_lowercase();
        let mut secrets = self.secrets.write().unwrap();
        secrets.insert(identifier.clone(), entry);

        Ok(identifier)
    }

    /// Get a secret value by identifier with constraint validation
    ///
    /// Returns `None` if the secret doesn't exist or constraints are violated.
    /// This method should only be called from Rust code for injection purposes.
    ///
    /// # Arguments
    ///
    /// * `identifier` - The secret identifier (e.g., "anthropic_api_key")
    /// * `target_url` - The URL where the secret will be sent (for validation)
    /// * `script_uri` - The script URI requesting the secret (for validation)
    ///
    /// # Security Note
    ///
    /// This method is NOT exposed to JavaScript. JavaScript can only check
    /// existence via `exists()` or list identifiers via `list_identifiers()`.
    pub fn get_with_constraints(
        &self,
        identifier: &str,
        target_url: &str,
        script_uri: Option<&str>,
    ) -> Result<String, SecretAccessError> {
        let secrets = self.secrets.read().unwrap();
        let entry = secrets
            .get(identifier)
            .ok_or_else(|| SecretAccessError::NotFound(identifier.to_string()))?;

        // Normalize URL for case-insensitive matching (lowercase scheme and host)
        let normalized_url = Self::normalize_url_for_matching(target_url);

        // Check URL constraint
        if let Some(ref url_matcher) = entry.allowed_url_pattern
            && !url_matcher.is_match(&normalized_url)
        {
            return Err(SecretAccessError::UrlConstraintViolation {
                secret_id: identifier.to_string(),
                attempted_url: target_url.to_string(),
            });
        }

        // Check script URI constraint
        if let Some(ref script_matcher) = entry.allowed_script_pattern {
            let uri = script_uri.ok_or_else(|| SecretAccessError::ScriptConstraintViolation {
                secret_id: identifier.to_string(),
                script_uri: "<unknown>".to_string(),
            })?;

            // Case-sensitive matching for script URIs
            if !script_matcher.is_match(uri) {
                return Err(SecretAccessError::ScriptConstraintViolation {
                    secret_id: identifier.to_string(),
                    script_uri: uri.to_string(),
                });
            }
        }

        Ok(entry.value.clone())
    }

    /// Get a secret value by identifier (deprecated - use get_with_constraints)
    ///
    /// Returns `None` if the secret doesn't exist.
    /// This method bypasses constraints and should only be used for backward compatibility.
    ///
    /// # Deprecated
    ///
    /// Use `get_with_constraints()` instead to enforce access control.
    #[deprecated(since = "0.1.0", note = "Use get_with_constraints for security")]
    pub fn get(&self, identifier: &str) -> Option<String> {
        let secrets = self.secrets.read().unwrap();
        secrets.get(identifier).map(|e| e.value.clone())
    }

    /// Normalize URL for case-insensitive matching
    ///
    /// Lowercases the scheme and host, preserves path case
    fn normalize_url_for_matching(url: &str) -> String {
        if let Ok(parsed) = url::Url::parse(url) {
            let scheme = parsed.scheme().to_lowercase();
            let host = parsed.host_str().unwrap_or("").to_lowercase();
            let port = parsed.port().map(|p| format!(":{}", p)).unwrap_or_default();
            let path = parsed.path();
            let query = parsed
                .query()
                .map(|q| format!("?{}", q))
                .unwrap_or_default();
            format!("{scheme}://{host}{port}{path}{query}")
        } else {
            // If parsing fails, lowercase the whole URL as fallback
            url.to_lowercase()
        }
    }

    /// Set or update a secret (without constraints)
    ///
    /// # Arguments
    ///
    /// * `identifier` - The secret identifier (e.g., "anthropic_api_key")
    /// * `value` - The secret value
    pub fn set(&self, identifier: String, value: String) {
        let mut secrets = self.secrets.write().unwrap();
        secrets.insert(
            identifier,
            SecretEntry {
                value,
                allowed_url_pattern: None,
                allowed_script_pattern: None,
            },
        );
    }

    /// Check if a secret exists
    ///
    /// # Arguments
    ///
    /// * `identifier` - The secret identifier to check
    ///
    /// # Returns
    ///
    /// `true` if the secret exists, `false` otherwise
    pub fn exists(&self, identifier: &str) -> bool {
        let secrets = self.secrets.read().unwrap();
        secrets.contains_key(identifier)
    }

    /// List all secret identifiers
    ///
    /// Returns a vector of secret identifiers (not values).
    /// Safe to expose to JavaScript for feature discovery.
    ///
    /// # Returns
    ///
    /// Vector of secret identifiers
    pub fn list_identifiers(&self) -> Vec<String> {
        let secrets = self.secrets.read().unwrap();
        secrets.keys().cloned().collect()
    }

    /// Delete a secret
    ///
    /// # Arguments
    ///
    /// * `identifier` - The secret identifier to delete
    ///
    /// # Returns
    ///
    /// `true` if the secret was deleted, `false` if it didn't exist
    pub fn delete(&self, identifier: &str) -> bool {
        let mut secrets = self.secrets.write().unwrap();
        secrets.remove(identifier).is_some()
    }

    /// Get the count of stored secrets
    ///
    /// # Returns
    ///
    /// Number of secrets currently stored
    pub fn count(&self) -> usize {
        let secrets = self.secrets.read().unwrap();
        secrets.len()
    }

    /// Clear all secrets
    ///
    /// # Warning
    ///
    /// This removes all secrets from memory. Use with caution.
    pub fn clear(&self) {
        let mut secrets = self.secrets.write().unwrap();
        secrets.clear();
        warn!("All secrets cleared from memory");
    }

    /// Check if a value looks like a secret (for redaction)
    ///
    /// This is a heuristic to identify potential secrets in logs.
    /// Checks for common secret patterns.
    ///
    /// # Arguments
    ///
    /// * `value` - The string to check
    ///
    /// # Returns
    ///
    /// `true` if the value looks like a secret
    pub fn looks_like_secret(value: &str) -> bool {
        // Common secret patterns
        let patterns = [
            "sk-",  // OpenAI, Anthropic
            "SG.",  // SendGrid
            "key_", // Stripe
            "api_key", "apikey", "secret", "token", "password", "Bearer ",
        ];

        let value_lower = value.to_lowercase();

        // Check for common patterns
        if patterns
            .iter()
            .any(|p| value_lower.contains(&p.to_lowercase()))
        {
            // Must be reasonably long to be a real secret
            if value.len() >= 20 {
                return true;
            }
        }

        // Check if it's a long alphanumeric string (likely a token)
        if value.len() >= 32 {
            let alphanumeric_count = value.chars().filter(|c| c.is_alphanumeric()).count();
            if alphanumeric_count as f32 / value.len() as f32 > 0.8 {
                return true;
            }
        }

        false
    }

    /// Redact a secret from a string
    ///
    /// Replaces the secret value with `[REDACTED]` in the text.
    ///
    /// # Arguments
    ///
    /// * `text` - The text that may contain secrets
    ///
    /// # Returns
    ///
    /// Text with secrets replaced by `[REDACTED]`
    pub fn redact(&self, text: &str) -> String {
        let secrets = self.secrets.read().unwrap();
        let mut result = text.to_string();

        for entry in secrets.values() {
            if entry.value.len() >= 8 {
                result = result.replace(&entry.value, "[REDACTED]");
            }
        }

        result
    }
}

impl Default for SecretsManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors that can occur when accessing secrets with constraints
#[derive(Debug, Clone)]
pub enum SecretAccessError {
    /// Secret not found
    NotFound(String),
    /// URL constraint violated
    UrlConstraintViolation {
        secret_id: String,
        attempted_url: String,
    },
    /// Script URI constraint violated
    ScriptConstraintViolation {
        secret_id: String,
        script_uri: String,
    },
}

impl std::fmt::Display for SecretAccessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound(id) => write!(f, "Secret '{}' not found", id),
            Self::UrlConstraintViolation {
                secret_id,
                attempted_url,
            } => {
                write!(
                    f,
                    "Secret '{}' not allowed for URL: {}",
                    secret_id, attempted_url
                )
            }
            Self::ScriptConstraintViolation {
                secret_id,
                script_uri,
            } => {
                write!(
                    f,
                    "Secret '{}' not allowed for script: {}",
                    secret_id, script_uri
                )
            }
        }
    }
}

impl std::error::Error for SecretAccessError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_secrets_manager() {
        let manager = SecretsManager::new();
        assert_eq!(manager.count(), 0);
    }

    #[test]
    fn test_set_and_get() {
        let manager = SecretsManager::new();
        manager.set("test_key".to_string(), "test_value".to_string());

        #[allow(deprecated)]
        let result = manager.get("test_key");
        assert_eq!(result, Some("test_value".to_string()));
        assert_eq!(manager.count(), 1);
    }

    #[test]
    fn test_exists() {
        let manager = SecretsManager::new();

        assert!(!manager.exists("test_key"));

        manager.set("test_key".to_string(), "test_value".to_string());

        assert!(manager.exists("test_key"));
        assert!(!manager.exists("nonexistent"));
    }

    #[test]
    fn test_list_identifiers() {
        let manager = SecretsManager::new();

        manager.set("key1".to_string(), "value1".to_string());
        manager.set("key2".to_string(), "value2".to_string());
        manager.set("key3".to_string(), "value3".to_string());

        let identifiers = manager.list_identifiers();

        assert_eq!(identifiers.len(), 3);
        assert!(identifiers.contains(&"key1".to_string()));
        assert!(identifiers.contains(&"key2".to_string()));
        assert!(identifiers.contains(&"key3".to_string()));
    }

    #[test]
    fn test_delete() {
        let manager = SecretsManager::new();

        manager.set("test_key".to_string(), "test_value".to_string());
        assert!(manager.exists("test_key"));

        assert!(manager.delete("test_key"));
        assert!(!manager.exists("test_key"));

        // Deleting non-existent key returns false
        assert!(!manager.delete("test_key"));
    }

    #[test]
    fn test_clear() {
        let manager = SecretsManager::new();

        manager.set("key1".to_string(), "value1".to_string());
        manager.set("key2".to_string(), "value2".to_string());

        assert_eq!(manager.count(), 2);

        manager.clear();

        assert_eq!(manager.count(), 0);
        assert!(!manager.exists("key1"));
        assert!(!manager.exists("key2"));
    }

    #[test]
    fn test_load_from_map() {
        let manager = SecretsManager::new();

        let mut secrets = HashMap::new();
        secrets.insert("api_key".to_string(), "sk-test123".to_string());
        secrets.insert("db_password".to_string(), "secret123".to_string());

        manager.load_from_map(secrets);

        assert_eq!(manager.count(), 2);
        assert!(manager.exists("api_key"));
        assert!(manager.exists("db_password"));
    }

    #[test]
    fn test_looks_like_secret() {
        // Should detect common secret patterns
        assert!(SecretsManager::looks_like_secret(
            "sk-ant-api03-1234567890abcdef"
        ));
        assert!(SecretsManager::looks_like_secret(
            "SG.1234567890abcdefghijklmnop"
        ));
        assert!(SecretsManager::looks_like_secret(
            "Bearer eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9"
        ));

        // Long random strings should be detected
        assert!(SecretsManager::looks_like_secret("a".repeat(40).as_str()));

        // Short strings or normal text should not be detected
        assert!(!SecretsManager::looks_like_secret("hello"));
        assert!(!SecretsManager::looks_like_secret("test"));
        assert!(!SecretsManager::looks_like_secret("sk-short"));
    }

    #[test]
    fn test_redact() {
        let manager = SecretsManager::new();

        manager.set("api_key".to_string(), "sk-test123456".to_string());
        manager.set("password".to_string(), "secret123".to_string());

        let text = "Using API key sk-test123456 and password secret123";
        let redacted = manager.redact(text);

        assert_eq!(redacted, "Using API key [REDACTED] and password [REDACTED]");
        assert!(!redacted.contains("sk-test123456"));
        assert!(!redacted.contains("secret123"));
    }

    #[test]
    fn test_thread_safety() {
        use std::thread;

        let manager = SecretsManager::new();
        let manager_clone = manager.clone();

        // Write from one thread
        let writer = thread::spawn(move || {
            for i in 0..100 {
                manager_clone.set(format!("key_{}", i), format!("value_{}", i));
            }
        });

        // Read from another thread
        let manager_clone2 = manager.clone();
        let reader = thread::spawn(move || {
            for i in 0..100 {
                manager_clone2.exists(&format!("key_{}", i));
            }
        });

        writer.join().unwrap();
        reader.join().unwrap();

        // Should have all 100 keys
        assert_eq!(manager.count(), 100);
    }
}

/// TOML configuration structures for constrained secrets
#[derive(Debug, Deserialize)]
struct SecretsTomlConfig {
    secret: Vec<SecretTomlDefinition>,
}

#[derive(Debug, Deserialize)]
struct SecretTomlDefinition {
    identifier: String,
    value: String,
    allowed_url_pattern: Option<String>,
    allowed_script_pattern: Option<String>,
}
