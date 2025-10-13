use super::validation::Capability;
use std::collections::HashSet;

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
        // Anonymous users can read and write (for development/testing)
        // In production, these should be restricted to authenticated users only
        [
            Capability::ViewLogs,
            Capability::ReadScripts,
            Capability::WriteScripts,
            Capability::ReadAssets,
        ]
        .into_iter()
        .collect()
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
            Capability::ViewLogs,
            Capability::ManageStreams,
            Capability::ManageGraphQL,
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

    #[test]
    fn test_anonymous_user_capabilities() {
        let user = UserContext::anonymous();

        assert!(!user.is_authenticated);
        assert!(user.user_id.is_none());
        assert!(user.has_capability(&Capability::ViewLogs));
        // Anonymous users now have ReadScripts and WriteScripts for development/testing
        assert!(user.has_capability(&Capability::ReadScripts));
        assert!(user.has_capability(&Capability::WriteScripts));
        assert!(user.has_capability(&Capability::ReadAssets));
        // But not delete capabilities
        assert!(!user.has_capability(&Capability::DeleteScripts));
        assert!(!user.has_capability(&Capability::DeleteAssets));
    }

    #[test]
    fn test_authenticated_user_capabilities() {
        let user = UserContext::authenticated("user123".to_string());

        assert!(user.is_authenticated);
        assert_eq!(user.user_id, Some("user123".to_string()));
        assert!(user.has_capability(&Capability::WriteScripts));
        assert!(!user.has_capability(&Capability::DeleteScripts));
    }

    #[test]
    fn test_admin_user_capabilities() {
        let user = UserContext::admin("admin".to_string());

        assert!(user.is_authenticated);
        assert!(user.has_capability(&Capability::DeleteScripts));
        assert!(user.has_capability(&Capability::WriteScripts));
        assert!(user.has_capability(&Capability::ManageGraphQL));
    }

    #[test]
    fn test_capability_requirement() {
        let user = UserContext::anonymous();

        // Should succeed for allowed capabilities
        assert!(user.require_capability(&Capability::ViewLogs).is_ok());
        assert!(user.require_capability(&Capability::ReadScripts).is_ok());
        assert!(user.require_capability(&Capability::WriteScripts).is_ok());

        // Should fail for disallowed capabilities (like delete)
        assert!(user.require_capability(&Capability::DeleteScripts).is_err());
        assert!(user.require_capability(&Capability::ManageGraphQL).is_err());
    }
}
