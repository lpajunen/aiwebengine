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

    /// A user who may manage the scripts they own. Ownership itself is checked
    /// separately (`repository::user_owns_script`); this tier only says the
    /// user is in the business of authoring solutions at all.
    pub fn editor(user_id: String) -> Self {
        Self {
            user_id: Some(user_id),
            is_authenticated: true,
            capabilities: Self::editor_capabilities(),
        }
    }

    pub fn admin(user_id: String) -> Self {
        Self {
            user_id: Some(user_id),
            is_authenticated: true,
            capabilities: Self::admin_capabilities(),
        }
    }

    /// What a caller with no identity holds: enough to be served a solution,
    /// and nothing that changes one.
    ///
    /// There is no second answer to this. A development mode used to hand this
    /// tier `AdministerEngine`, `WriteScripts` and the rest so a local instance
    /// could be driven without a login — which meant an engine bound to
    /// anything but loopback was administrable by whoever reached the port, and
    /// an `AIWEBENGINE_MODE` env var could turn it on in a deployment whose
    /// configuration said otherwise. Administering an engine now takes being an
    /// administrator: `auth.internal.bootstrap_admin_usernames` names one, and
    /// `--grant-role` appoints one with no server running.
    fn anonymous_capabilities() -> HashSet<Capability> {
        [
            Capability::ReadScripts, // Read public scripts only
            Capability::ReadAssets,  // Read public assets only
        ]
        .into_iter()
        .collect()
    }

    /// What someone *using* a solution holds. A script serving a request runs
    /// under the requesting user's context, so this set has to cover everything
    /// an ordinary request does — read the script and its assets, log, push
    /// stream messages, and read and write its own rows — while granting
    /// nothing that edits the solution itself. Authoring lives in
    /// [`Self::editor_capabilities`].
    fn authenticated_capabilities() -> HashSet<Capability> {
        [
            Capability::ReadScripts,
            Capability::ReadAssets,
            Capability::ViewLogs,
            Capability::ManageStreams,
            Capability::UseScriptDatabase,
        ]
        .into_iter()
        .collect()
    }

    /// What an author holds: everything a solution's users can do, plus the
    /// ability to change scripts, assets, and schema. `DeleteScripts` here
    /// means "delete a script I own" — callers pair it with an ownership check;
    /// acting on someone else's script needs `AdministerEngine`.
    fn editor_capabilities() -> HashSet<Capability> {
        let mut capabilities = Self::authenticated_capabilities();
        capabilities.extend([
            Capability::WriteScripts,
            Capability::DeleteScripts,
            Capability::WriteAssets,
            Capability::DeleteAssets,
            Capability::DeleteLogs,
            Capability::ManageGraphQL,
            Capability::ManageScriptDatabase,
        ]);
        capabilities
    }

    fn admin_capabilities() -> HashSet<Capability> {
        // Everything an editor may do, plus acting on what they do not own.
        let mut capabilities = Self::editor_capabilities();
        capabilities.insert(Capability::AdministerEngine);
        capabilities
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

    /// There is one anonymous tier and no way to widen it. What used to sit
    /// here — a configured flag and an `AIWEBENGINE_MODE` env var that
    /// outranked it, either of which handed anonymous callers the engine —
    /// is gone; administering the engine takes being an administrator.
    #[test]
    fn an_anonymous_caller_can_read_a_solution_and_change_nothing() {
        let user = UserContext::anonymous();

        assert!(user.has_capability(&Capability::ReadScripts));
        assert!(user.has_capability(&Capability::ReadAssets));

        for denied in [
            Capability::WriteScripts,
            Capability::DeleteScripts,
            Capability::WriteAssets,
            Capability::ManageStreams,
            Capability::ManageScriptDatabase,
            Capability::AdministerEngine,
        ] {
            assert!(
                !user.has_capability(&denied),
                "anonymous must not hold {:?}",
                denied
            );
        }
    }

    #[test]
    fn test_authenticated_user_capabilities() {
        let user = UserContext::authenticated("user123".to_string());

        assert!(user.is_authenticated);
        assert_eq!(user.user_id, Some("user123".to_string()));

        // What serving this user's requests needs.
        assert!(user.has_capability(&Capability::ReadScripts));
        assert!(user.has_capability(&Capability::ReadAssets));
        assert!(user.has_capability(&Capability::ViewLogs));
        assert!(user.has_capability(&Capability::ManageStreams));
        assert!(user.has_capability(&Capability::UseScriptDatabase));

        // Using a solution is not authoring one.
        assert!(!user.has_capability(&Capability::WriteScripts));
        assert!(!user.has_capability(&Capability::WriteAssets));
        assert!(!user.has_capability(&Capability::DeleteScripts));
        assert!(!user.has_capability(&Capability::DeleteLogs));
        assert!(!user.has_capability(&Capability::ManageScriptDatabase));
        assert!(!user.has_capability(&Capability::ManageGraphQL));
        assert!(!user.has_capability(&Capability::AdministerEngine));
    }

    #[test]
    fn test_editor_user_capabilities() {
        let user = UserContext::editor("author".to_string());

        assert!(user.is_authenticated);

        // Everything a solution's users can do, plus authoring.
        assert!(user.has_capability(&Capability::UseScriptDatabase));
        assert!(user.has_capability(&Capability::WriteScripts));
        assert!(user.has_capability(&Capability::DeleteScripts));
        assert!(user.has_capability(&Capability::WriteAssets));
        assert!(user.has_capability(&Capability::ManageScriptDatabase));
        assert!(user.has_capability(&Capability::ManageGraphQL));

        // But nothing that reaches another author's work. `DeleteScripts` here
        // means "the ones I own"; the ownership check is what bounds it.
        assert!(!user.has_capability(&Capability::AdministerEngine));
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

    /// `require_capability` reports what the tier holds, either way round.
    #[test]
    fn test_capability_requirement() {
        let user = UserContext::authenticated("user123".to_string());

        assert!(user.require_capability(&Capability::ReadScripts).is_ok());
        assert!(user.require_capability(&Capability::ViewLogs).is_ok());
        assert!(user.require_capability(&Capability::ManageStreams).is_ok());

        assert!(user.require_capability(&Capability::WriteScripts).is_err());
        assert!(user.require_capability(&Capability::DeleteScripts).is_err());
        assert!(user.require_capability(&Capability::ManageGraphQL).is_err());
    }
}
