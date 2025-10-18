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

/// Thread-safe secrets manager
///
/// Stores secrets in memory with read-write lock for concurrent access.
/// Secrets can be loaded from configuration files or environment variables.
#[derive(Clone)]
pub struct SecretsManager {
    secrets: Arc<RwLock<HashMap<String, String>>>,
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
    /// Looks for environment variables prefixed with `SECRET_`.
    /// For example: `SECRET_ANTHROPIC_API_KEY` becomes `anthropic_api_key`
    ///
    /// # Example
    ///
    /// ```bash
    /// export SECRET_ANTHROPIC_API_KEY="sk-ant-api03-..."
    /// export SECRET_SENDGRID_API_KEY="SG.xyz..."
    /// ```
    pub fn load_from_env(&self) {
        let mut count = 0;

        for (key, value) in std::env::vars() {
            if key.starts_with("SECRET_") {
                // Convert SECRET_ANTHROPIC_API_KEY to anthropic_api_key
                let secret_id = key.strip_prefix("SECRET_").unwrap().to_lowercase();

                if !value.is_empty() {
                    self.set(secret_id.clone(), value);
                    count += 1;
                    info!(secret_id = %secret_id, "Loaded secret from environment");
                }
            }
        }

        if count > 0 {
            info!(count = count, "Loaded secrets from environment variables");
        } else {
            debug!("No secrets found in environment variables (looking for SECRET_* prefix)");
        }
    }

    /// Load secrets from a HashMap (typically from config file)
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

    /// Get a secret value by identifier
    ///
    /// Returns `None` if the secret doesn't exist.
    /// This method should only be called from Rust code for injection purposes.
    ///
    /// # Arguments
    ///
    /// * `identifier` - The secret identifier (e.g., "anthropic_api_key")
    ///
    /// # Security Note
    ///
    /// This method is NOT exposed to JavaScript. JavaScript can only check
    /// existence via `exists()` or list identifiers via `list_identifiers()`.
    pub fn get(&self, identifier: &str) -> Option<String> {
        let secrets = self.secrets.read().unwrap();
        secrets.get(identifier).cloned()
    }

    /// Set or update a secret
    ///
    /// # Arguments
    ///
    /// * `identifier` - The secret identifier (e.g., "anthropic_api_key")
    /// * `value` - The secret value
    pub fn set(&self, identifier: String, value: String) {
        let mut secrets = self.secrets.write().unwrap();
        secrets.insert(identifier, value);
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

        for value in secrets.values() {
            if value.len() >= 8 {
                result = result.replace(value, "[REDACTED]");
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

        assert_eq!(manager.get("test_key"), Some("test_value".to_string()));
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
