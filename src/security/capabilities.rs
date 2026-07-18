use super::validation::Capability;
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};

/// Whether the engine runs in development mode (elevated anonymous capabilities).
/// Fail-closed: production capabilities apply unless development mode is
/// explicitly enabled via configuration or the AIWEBENGINE_MODE env var.
static DEVELOPMENT_MODE: AtomicBool = AtomicBool::new(false);

/// Set development mode from configuration (`security.development_mode`).
/// Called once at server startup, before any script executes.
pub fn set_development_mode(enabled: bool) {
    DEVELOPMENT_MODE.store(enabled, Ordering::Relaxed);
}

/// True when development mode is active. The AIWEBENGINE_MODE env var, when
/// set, takes precedence over the configured flag; when neither is present
/// the engine defaults to production (minimal anonymous capabilities).
pub fn is_development_mode() -> bool {
    match std::env::var("AIWEBENGINE_MODE") {
        Ok(mode) => mode == "development",
        Err(_) => DEVELOPMENT_MODE.load(Ordering::Relaxed),
    }
}

#[derive(Debug, Clone)]
pub struct UserContext {
    pub user_id: Option<String>,
    pub is_authenticated: bool,
    pub capabilities: HashSet<Capability>,
}

impl UserContext {
    pub fn anonymous() -> Self {
        Self {
            user_id: None,
            is_authenticated: false,
            capabilities: Self::anonymous_capabilities(),
        }
    }

    pub fn authenticated(user_id: String) -> Self {
        Self {
            user_id: Some(user_id),
            is_authenticated: true,
            capabilities: Self::authenticated_capabilities(),
        }
    }

    pub fn admin(user_id: String) -> Self {
        Self {
            user_id: Some(user_id),
            is_authenticated: true,
            capabilities: Self::admin_capabilities(),
        }
    }

    fn anonymous_capabilities() -> HashSet<Capability> {
        // In development mode, anonymous users get elevated permissions for testing
        // In production, they should have minimal read-only capabilities (REQ-AUTH-006)
        if is_development_mode() {
            // Development mode: elevated permissions for easier testing
            [
                Capability::ViewLogs,
                Capability::ReadScripts,
                Capability::WriteScripts,
                Capability::ReadAssets,
                Capability::WriteAssets,
                Capability::DeleteScripts, // Allow script deletion in dev mode
                Capability::DeleteAssets,  // Allow asset deletion in dev mode
                Capability::DeleteLogs,    // Allow log management in dev mode
                Capability::ManageGraphQL, // Allow GraphQL operations in dev mode
                Capability::ManageStreams, // Allow stream operations in dev mode
                Capability::ManageScriptDatabase, // Allow database schema operations in dev mode
            ]
            .into_iter()
            .collect()
        } else {
            // Production mode: minimal read-only capabilities
            [
                Capability::ReadScripts, // Read public scripts only
                Capability::ReadAssets,  // Read public assets only
            ]
            .into_iter()
            .collect()
        }
    }

    fn authenticated_capabilities() -> HashSet<Capability> {
        // Authenticated users can read/write most things
        [
            Capability::ReadScripts,
            Capability::WriteScripts,
            Capability::ReadAssets,
            Capability::WriteAssets,
            Capability::ViewLogs,
            Capability::ManageStreams,
            Capability::ManageScriptDatabase,
        ]
        .into_iter()
        .collect()
    }

    fn admin_capabilities() -> HashSet<Capability> {
        // Admins can do everything
        [
            Capability::ReadScripts,
            Capability::WriteScripts,
            Capability::DeleteScripts,
            Capability::ReadAssets,
            Capability::WriteAssets,
            Capability::DeleteAssets,
            Capability::DeleteLogs,
            Capability::ViewLogs,
            Capability::ManageStreams,
            Capability::ManageGraphQL,
            Capability::ManageScriptDatabase,
        ]
        .into_iter()
        .collect()
    }

    pub fn has_capability(&self, capability: &Capability) -> bool {
        self.capabilities.contains(capability)
    }

    pub fn require_capability(
        &self,
        capability: &Capability,
    ) -> Result<(), super::validation::SecurityError> {
        if self.has_capability(capability) {
            Ok(())
        } else {
            Err(super::validation::SecurityError::InsufficientCapabilities {
                required: vec![capability.clone()],
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serializes tests that mutate the process-global AIWEBENGINE_MODE env
    /// var and the development-mode flag, so they don't race under `cargo test`.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_anonymous_defaults_to_production_capabilities() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        // With no env var and no configured flag, the engine must fail closed.
        unsafe {
            std::env::remove_var("AIWEBENGINE_MODE");
        }
        set_development_mode(false);
        let user = UserContext::anonymous();
        assert!(user.has_capability(&Capability::ReadScripts));
        assert!(user.has_capability(&Capability::ReadAssets));
        assert!(!user.has_capability(&Capability::WriteScripts));
        assert!(!user.has_capability(&Capability::DeleteScripts));
        assert!(!user.has_capability(&Capability::ManageScriptDatabase));

        // The configuration flag enables development capabilities
        set_development_mode(true);
        let dev_user = UserContext::anonymous();
        assert!(dev_user.has_capability(&Capability::WriteScripts));
        set_development_mode(false);

        // The env var takes precedence over the configured flag when set
        unsafe {
            std::env::set_var("AIWEBENGINE_MODE", "development");
        }
        let env_user = UserContext::anonymous();
        assert!(env_user.has_capability(&Capability::WriteScripts));
        unsafe {
            std::env::remove_var("AIWEBENGINE_MODE");
        }
    }

    #[test]
    fn test_anonymous_user_capabilities() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        // Test development mode
        unsafe {
            std::env::set_var("AIWEBENGINE_MODE", "development");
        }
        let dev_user = UserContext::anonymous();

        assert!(!dev_user.is_authenticated);
        assert!(dev_user.user_id.is_none());
        assert!(dev_user.has_capability(&Capability::ViewLogs));
        assert!(dev_user.has_capability(&Capability::DeleteLogs)); // Allowed in dev mode
        assert!(dev_user.has_capability(&Capability::ReadScripts));
        assert!(dev_user.has_capability(&Capability::WriteScripts));
        assert!(dev_user.has_capability(&Capability::ReadAssets));
        assert!(dev_user.has_capability(&Capability::WriteAssets));
        assert!(dev_user.has_capability(&Capability::DeleteScripts)); // Allowed in dev mode
        assert!(dev_user.has_capability(&Capability::DeleteAssets)); // Allowed in dev mode

        // Test production mode
        unsafe {
            std::env::set_var("AIWEBENGINE_MODE", "production");
        }
        let prod_user = UserContext::anonymous();

        assert!(!prod_user.is_authenticated);
        assert!(prod_user.user_id.is_none());
        assert!(prod_user.has_capability(&Capability::ReadScripts)); // Read-only in production
        assert!(prod_user.has_capability(&Capability::ReadAssets));
        assert!(!prod_user.has_capability(&Capability::ViewLogs)); // No logs in production
        assert!(!prod_user.has_capability(&Capability::WriteScripts)); // No write in production
        assert!(!prod_user.has_capability(&Capability::DeleteScripts)); // No delete in production

        // Clean up so other tests see the unset-env default
        unsafe {
            std::env::remove_var("AIWEBENGINE_MODE");
        }
    }

    #[test]
    fn test_authenticated_user_capabilities() {
        let user = UserContext::authenticated("user123".to_string());

        assert!(user.is_authenticated);
        assert_eq!(user.user_id, Some("user123".to_string()));
        assert!(user.has_capability(&Capability::WriteScripts));
        assert!(!user.has_capability(&Capability::DeleteScripts));
        assert!(!user.has_capability(&Capability::DeleteLogs));
    }

    #[test]
    fn test_admin_user_capabilities() {
        let user = UserContext::admin("admin".to_string());

        assert!(user.is_authenticated);
        assert!(user.has_capability(&Capability::DeleteScripts));
        assert!(user.has_capability(&Capability::WriteScripts));
        assert!(user.has_capability(&Capability::ManageGraphQL));
        assert!(user.has_capability(&Capability::DeleteLogs));
    }

    #[test]
    fn test_capability_requirement() {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        // Ensure we're in development mode for this test
        unsafe {
            std::env::set_var("AIWEBENGINE_MODE", "development");
        }
        let user = UserContext::anonymous();

        // Should succeed for allowed capabilities in dev mode
        assert!(user.require_capability(&Capability::ViewLogs).is_ok());
        assert!(user.require_capability(&Capability::ReadScripts).is_ok());
        assert!(user.require_capability(&Capability::WriteScripts).is_ok());
        assert!(user.require_capability(&Capability::DeleteScripts).is_ok()); // OK in dev mode
        assert!(user.require_capability(&Capability::ManageGraphQL).is_ok()); // OK in dev mode for demo
        assert!(user.require_capability(&Capability::ManageStreams).is_ok()); // OK in dev mode for demo

        unsafe {
            std::env::remove_var("AIWEBENGINE_MODE");
        }
    }
}
