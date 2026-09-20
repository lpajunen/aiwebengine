use super::validation::Capability;
use std::collections::HashSet;

impl Capability {
    /// The name this capability is asked for by, from JavaScript and over the
    /// wire. Snake case, as [`crate::delegation::Scope`]'s names are.
    pub fn as_str(&self) -> &'static str {
        match self {
            Capability::ReadScripts => "read_scripts",
            Capability::WriteScripts => "write_scripts",
            Capability::DeleteScripts => "delete_scripts",
            Capability::ReadAssets => "read_assets",
            Capability::WriteAssets => "write_assets",
            Capability::DeleteAssets => "delete_assets",
            Capability::DeleteLogs => "delete_logs",
            Capability::ViewLogs => "view_logs",
            Capability::ManageStreams => "manage_streams",
            Capability::ManageGraphQL => "manage_graphql",
            Capability::ReadScriptData => "read_script_data",
            Capability::WriteScriptData => "write_script_data",
            Capability::ManageScriptDatabase => "manage_script_database",
            Capability::AdministerEngine => "administer_engine",
            Capability::UseNetwork => "use_network",
            Capability::ReadSecrets => "read_secrets",
            Capability::WriteSecrets => "write_secrets",
            Capability::ReadStorage => "read_storage",
            Capability::WriteStorage => "write_storage",
            Capability::EnqueueTasks => "enqueue_tasks",
            Capability::SendMessages => "send_messages",
        }
    }

    /// An unknown name is not a capability. Refused rather than dropped: a
    /// caller asking for `read_only` deserves to be told no such thing exists,
    /// where silently ignoring it would hand them a context they did not ask
    /// for and believe they had checked.
    pub fn parse(value: &str) -> Option<Self> {
        Self::all()
            .into_iter()
            .find(|capability| capability.as_str() == value.trim())
    }

    pub fn all() -> [Capability; 21] {
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
            Capability::ReadScriptData,
            Capability::WriteScriptData,
            Capability::ManageScriptDatabase,
            Capability::AdministerEngine,
            Capability::UseNetwork,
            Capability::ReadSecrets,
            Capability::WriteSecrets,
            Capability::ReadStorage,
            Capability::WriteStorage,
            Capability::EnqueueTasks,
            Capability::SendMessages,
        ]
    }
}

#[derive(Debug, Clone)]
pub struct UserContext {
    pub user_id: Option<String>,
    pub is_authenticated: bool,
    pub capabilities: HashSet<Capability>,
    /// True when this context was narrowed from another
    /// ([`UserContext::attenuated`]) rather than built from a tier.
    ///
    /// Nothing is authorized on it — the capability set is the whole of the
    /// answer to what may be done. It is here so that a refusal and an audit
    /// line can say *why* a context that belongs to an administrator is
    /// refusing something an administrator may do, which is otherwise the most
    /// confusing message the engine can produce.
    pub attenuated: bool,
}

impl UserContext {
    pub fn anonymous() -> Self {
        Self {
            user_id: None,
            is_authenticated: false,
            capabilities: Self::anonymous_capabilities(),
            attenuated: false,
        }
    }

    pub fn authenticated(user_id: String) -> Self {
        Self {
            user_id: Some(user_id),
            is_authenticated: true,
            capabilities: Self::authenticated_capabilities(),
            attenuated: false,
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
            attenuated: false,
        }
    }

    pub fn admin(user_id: String) -> Self {
        Self {
            user_id: Some(user_id),
            is_authenticated: true,
            capabilities: Self::admin_capabilities(),
            attenuated: false,
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
            // The script-side capabilities an anonymous caller already had.
            // `fetch`, `secretStorage` and `scriptStorage` were reachable from
            // any context that had them installed — which, for a script route
            // serving a signed-out visitor, is this one. Naming them did not
            // widen this tier; leaving them out would have narrowed it, and
            // every public script calling an API would have stopped working.
            Capability::UseNetwork,
            Capability::ReadSecrets,
            Capability::WriteSecrets,
            Capability::ReadStorage,
            Capability::WriteStorage,
            Capability::EnqueueTasks,
            Capability::SendMessages,
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
        let mut capabilities = Self::anonymous_capabilities();
        capabilities.extend([
            Capability::ViewLogs,
            Capability::ManageStreams,
            Capability::ReadScriptData,
            Capability::WriteScriptData,
        ]);
        capabilities
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

    /// This same identity, holding no more than `keep`.
    ///
    /// The intersection, never the union: a context can only ever be narrowed
    /// by this, whatever is asked for. That is what makes it safe to expose to
    /// JavaScript at all — a script calling it with a set it does not hold
    /// gets less than it asked for rather than more than it had. (The JS
    /// surface refuses that call outright, because asking for a capability you
    /// do not hold is a bug worth naming rather than silently narrowing; the
    /// intersection here is the floor under that check, not a substitute for
    /// it.)
    ///
    /// The identity does not change. Attenuation says what may be done, not
    /// who is doing it, so `user_id` carries through — ownership checks,
    /// per-person secret resolution and `personalStorage` all go on resolving
    /// against the same account. A narrowed context that changed identity
    /// would silently move a script's writes into a different person's rows.
    pub fn attenuated(&self, keep: impl IntoIterator<Item = Capability>) -> Self {
        let requested: HashSet<Capability> = keep.into_iter().collect();
        Self {
            user_id: self.user_id.clone(),
            is_authenticated: self.is_authenticated,
            capabilities: self
                .capabilities
                .intersection(&requested)
                .cloned()
                .collect(),
            attenuated: true,
        }
    }

    /// What `keep` asks for that this context does not hold.
    ///
    /// Empty means the request is a narrowing and nothing else. Callers use it
    /// to refuse before narrowing, so a script asking for `write_script_data`
    /// from a context that never had it is told so rather than handed a
    /// sub-execution that quietly cannot write.
    pub fn unheld(&self, keep: impl IntoIterator<Item = Capability>) -> Vec<Capability> {
        let mut missing: Vec<Capability> = keep
            .into_iter()
            .filter(|capability| !self.has_capability(capability))
            .collect();
        missing.sort_by_key(|capability| capability.as_str());
        missing.dedup();
        missing
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
    ///
    /// "Change nothing" is about the *solution* — its scripts, its assets,
    /// its schema. The script-side capabilities this tier also holds
    /// (`WriteStorage`, `WriteSecrets`) are what a script route does while
    /// serving this visitor, and each is separately bounded: the two personal
    /// stores need an authenticated user before they touch anything, which an
    /// anonymous caller by definition does not have.
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
        assert!(user.has_capability(&Capability::ReadScriptData));
        assert!(user.has_capability(&Capability::WriteScriptData));

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
        assert!(user.has_capability(&Capability::ReadScriptData));
        assert!(user.has_capability(&Capability::WriteScriptData));
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

    /// The tiers nest. Every one of them is built from the one below, and
    /// this is the assertion that keeps it that way: a capability added to a
    /// lower tier by hand rather than by extension would otherwise silently
    /// leave a higher one holding less than the tier beneath it.
    #[test]
    fn each_tier_holds_everything_the_one_below_it_does() {
        let anonymous = UserContext::anonymous();
        let authenticated = UserContext::authenticated("u".to_string());
        let editor = UserContext::editor("u".to_string());
        let admin = UserContext::admin("u".to_string());

        for (lower, higher, names) in [
            (&anonymous, &authenticated, "anonymous ⊆ authenticated"),
            (&authenticated, &editor, "authenticated ⊆ editor"),
            (&editor, &admin, "editor ⊆ admin"),
        ] {
            for capability in &lower.capabilities {
                assert!(
                    higher.has_capability(capability),
                    "{}: {:?} is missing",
                    names,
                    capability
                );
            }
        }
    }

    /// The claim that made it safe to add the script-side capabilities at
    /// all: naming them took nothing away from anybody. Each of these gated
    /// nothing before, so a caller who could reach the API has to hold the
    /// name for it — and the anonymous tier is the one that matters, because
    /// a script route serving a signed-out visitor runs in exactly this
    /// context, and could always `fetch` and reach `scriptStorage`.
    #[test]
    fn naming_what_a_script_does_took_nothing_away_from_anybody() {
        let anonymous = UserContext::anonymous();

        for held in [
            Capability::UseNetwork,
            Capability::ReadSecrets,
            Capability::WriteSecrets,
            Capability::ReadStorage,
            Capability::WriteStorage,
            Capability::EnqueueTasks,
            Capability::SendMessages,
        ] {
            assert!(
                anonymous.has_capability(&held),
                "a signed-out visitor's request could already do this: {:?}",
                held
            );
        }

        // And what it could not do before, it still cannot. The script
        // database was never reachable without a session.
        assert!(!anonymous.has_capability(&Capability::ReadScriptData));
        assert!(!anonymous.has_capability(&Capability::WriteScriptData));
    }

    /// Every name round-trips, and there is exactly one name per value. A
    /// duplicate would make `parse` answer with whichever came first in
    /// `all()`, silently granting one capability where another was asked for.
    #[test]
    fn every_capability_has_one_name_and_round_trips_through_it() {
        let mut seen = std::collections::HashSet::new();
        for capability in Capability::all() {
            let name = capability.as_str();
            assert!(seen.insert(name), "two capabilities are called '{}'", name);
            assert_eq!(Capability::parse(name), Some(capability));
        }
        assert_eq!(seen.len(), Capability::all().len());
    }

    /// A name nothing gates on would be a promise the engine does not keep,
    /// so an unknown one is refused rather than dropped.
    #[test]
    fn an_unknown_name_is_not_a_capability() {
        assert_eq!(Capability::parse("read_only"), None);
        assert_eq!(Capability::parse("use_script_database"), None);
        assert_eq!(Capability::parse(""), None);
    }

    /// Asking to keep what you do not hold is reported, and asking to keep
    /// what you do hold reports nothing.
    #[test]
    fn unheld_names_only_what_is_missing() {
        let user = UserContext::authenticated("u".to_string());

        assert!(user.unheld([Capability::ReadScriptData]).is_empty());
        assert_eq!(
            user.unheld([Capability::ReadScriptData, Capability::WriteScripts]),
            vec![Capability::WriteScripts]
        );
    }
}
