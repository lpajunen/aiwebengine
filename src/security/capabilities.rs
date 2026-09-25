use super::validation::Capability;
use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

/// Which hosts an execution may reach, when that is narrower than "any".
///
/// [`Capability::UseNetwork`] names the verb and nothing else, so "may call the
/// network" has always meant "may call anything". That is the gap a capability
/// set cannot close on its own: **exfiltration needs no write capability**, so
/// a planning turn holding only reads can still put what it read into a URL. A
/// tool that runs model-authored code is the sharp case, because the untrusted
/// text and the network reach the same execution.
///
/// This is the destination dimension. `None` on a [`UserContext`] means
/// unrestricted, which is what every context built from a tier is and what the
/// engine did before this existed; `Some` means these hosts and nothing else,
/// checked against the request's host **and against every redirect hop** —
/// an open redirector on an allowed host is otherwise the way out.
///
/// It is not a capability, and is deliberately not spelled as one. A capability
/// is a verb held or not held; this is an argument to one. Which is why
/// `sandbox.run` takes `hosts` beside `capabilities` rather than inventing
/// names like `use_network:api.example.com`, a spelling that would have made
/// the set no longer a set of verbs and every `has_capability` call a parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkScope {
    /// Lower-cased host patterns. An entry beginning `*.` matches subdomains.
    hosts: BTreeSet<String>,
}

impl NetworkScope {
    /// Build a scope from caller-written host patterns.
    ///
    /// An empty scope is meaningful and permits nothing, which is what an
    /// execution that should make no requests at all wants. Withholding
    /// `use_network` says the same thing more directly and both work — this one
    /// gives a refusal that names the destination, which is the more useful
    /// message when a list was meant to have something in it.
    pub fn new<I, S>(hosts: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        Self {
            hosts: hosts
                .into_iter()
                .map(|host| host.as_ref().trim().to_ascii_lowercase())
                .filter(|host| !host.is_empty())
                .collect(),
        }
    }

    /// The patterns this scope holds.
    pub fn hosts(&self) -> impl Iterator<Item = &str> {
        self.hosts.iter().map(String::as_str)
    }

    /// Whether `host` is one this scope permits.
    ///
    /// Exact match, or a `*.example.com` entry against any subdomain of
    /// `example.com`. A wildcard deliberately does **not** match the bare
    /// parent — `*.example.com` permits `api.example.com` and not
    /// `example.com` — which is the rule CSP and CORS use and the one that
    /// makes a list say what it looks like it says. Name both when both are
    /// wanted.
    ///
    /// Ports are not part of it: a port distinguishes services rather than
    /// parties, and what is being bounded here is who may be talked to.
    pub fn permits(&self, host: &str) -> bool {
        let host = host
            .trim()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_ascii_lowercase();
        self.hosts
            .iter()
            .any(|pattern| Self::pattern_permits(pattern, &host))
    }

    fn pattern_permits(pattern: &str, host: &str) -> bool {
        match pattern.strip_prefix("*.") {
            // Measured rather than compared to the suffix alone, so a host
            // whose first label is empty — `.example.com` — cannot pass as a
            // subdomain of it.
            Some(suffix) => {
                host.len() > suffix.len() + 1
                    && host.ends_with(suffix)
                    && host.as_bytes()[host.len() - suffix.len() - 1] == b'.'
            }
            None => host == pattern,
        }
    }

    /// Whether this scope already covers everything `pattern` would permit.
    ///
    /// The question a narrowing asks, and it is not the same as [`permits`]:
    /// `*.example.com` as a *request* is covered only by a caller scope that
    /// itself holds `*.example.com` or a wildcard above it, never by one
    /// holding the single host `api.example.com`. Asking `permits` about the
    /// pattern text would have got this wrong in the direction that widens.
    ///
    /// [`permits`]: NetworkScope::permits
    pub fn covers_pattern(&self, pattern: &str) -> bool {
        let pattern = pattern.trim().to_ascii_lowercase();
        match pattern.strip_prefix("*.") {
            Some(suffix) => self.hosts.iter().any(|held| {
                held == &pattern
                    || held.strip_prefix("*.").is_some_and(|held_suffix| {
                        held_suffix == suffix || Self::pattern_permits(held, suffix)
                    })
            }),
            None => self.permits(&pattern),
        }
    }
}

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
            Capability::ManageMcp => "manage_mcp",
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

    pub fn all() -> [Capability; 20] {
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
            Capability::ManageMcp,
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
        ]
    }
}

/// The roles a credential carries, as every carrier of one spells them.
///
/// [`crate::auth::AuthUser`], [`crate::auth::AuthSession`] and
/// [`crate::auth::JsAuthContext`] are three views of one session, and each of
/// them held its own copy of the "which tier is this" match — four copies in
/// all, counting the one written inline in the dynamic-request path. That is
/// three too many for a rule which is about to grow a second half: a session
/// will carry the roles it was minted with *and* how much of them is switched
/// on right now (`docs/SESSION_ELEVATION.md`). A second half added to a rule
/// that lives in four places lands in three of them and is forgotten in the
/// fourth, and the one it is forgotten in is the one that keeps working.
///
/// `user_id` carries the option rather than the caller testing for one,
/// because a carrier can hold an id and still not be a principal —
/// [`crate::auth::JsAuthContext`] does, when `is_authenticated` is false. Such
/// a carrier passes `None` and gets the anonymous tier, which is what it did
/// before this existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SessionRoles<'a> {
    /// The account this credential belongs to, or `None` for a caller with no
    /// identity.
    pub user_id: Option<&'a str>,
    pub is_admin: bool,
    pub is_editor: bool,
    /// What this session switched on beyond the floor, if anything.
    ///
    /// Borrowed rather than owned because it is read once, here, to compute a
    /// capability set — nothing downstream needs the elevation itself, and a
    /// `UserContext` that carried one would be a second place to ask the same
    /// question.
    pub elevation: Option<&'a super::elevation::Elevation>,
}

impl SessionRoles<'_> {
    /// A caller with no identity. The [`Default`], named, so that
    /// `.unwrap_or_default()` at a call site reads as what it means.
    pub fn anonymous() -> Self {
        Self::default()
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
    /// Which hosts this execution may reach, when that is narrower than any.
    ///
    /// `None` on every context built from a tier, which is the behaviour that
    /// predates this field: `use_network` names the verb and permits any
    /// destination. Set only by a narrowing, and `Arc` because a context is
    /// cloned into every global closure at install time and the list is read
    /// rather than written.
    pub network_scope: Option<Arc<NetworkScope>>,
}

impl UserContext {
    pub fn anonymous() -> Self {
        Self {
            user_id: None,
            is_authenticated: false,
            capabilities: Self::anonymous_capabilities(),
            attenuated: false,
            network_scope: None,
        }
    }

    pub fn authenticated(user_id: String) -> Self {
        Self {
            user_id: Some(user_id),
            is_authenticated: true,
            capabilities: Self::authenticated_capabilities(),
            attenuated: false,
            network_scope: None,
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
            network_scope: None,
        }
    }

    pub fn admin(user_id: String) -> Self {
        Self {
            user_id: Some(user_id),
            is_authenticated: true,
            capabilities: Self::admin_capabilities(),
            attenuated: false,
            network_scope: None,
        }
    }

    /// The context a credential is entitled to.
    ///
    /// The one place the engine turns a session's roles into a tier. Every
    /// path that authenticates a caller — the engine's own HTTP endpoints, the
    /// native MCP tools, a script serving a request, and the `auth` object a
    /// script reads — arrives here, so a change to what a session may do is a
    /// change to one function rather than a change that has to be remembered
    /// four times.
    ///
    /// Administrator wins over editor because the sets nest: an administrator
    /// holds everything an editor does plus [`Capability::AdministerEngine`].
    ///
    /// No identity is the anonymous tier whatever the flags say. A carrier
    /// that has no `user_id` is not a principal, and one claiming to be an
    /// administrator without being anybody is a bug better answered with the
    /// smallest tier than with the largest.
    pub fn for_session(roles: SessionRoles<'_>) -> Self {
        Self::for_session_at(roles, chrono::Utc::now())
    }

    /// [`Self::for_session`] as of a given moment, so that a test can describe
    /// an elevation that has run out without waiting for one to.
    pub fn for_session_at(roles: SessionRoles<'_>, now: chrono::DateTime<chrono::Utc>) -> Self {
        let tier = match roles.user_id {
            Some(user_id) if roles.is_admin => Self::admin(user_id.to_string()),
            Some(user_id) if roles.is_editor => Self::editor(user_id.to_string()),
            Some(user_id) => Self::authenticated(user_id.to_string()),
            None => return Self::anonymous(),
        };

        // Nothing gated is the default and by far the common path, and it is
        // answered without touching the capability set at all. Going through
        // the composition anyway would clone the tier's set and rebuild it by
        // intersection on every request the engine serves, to arrive at the
        // set it started with.
        let policy = super::elevation::configured();
        if !policy.is_active() {
            return tier;
        }

        let held = super::elevation::held_under(&policy, &tier, roles.elevation, now);

        // The intersection is what keeps the ceiling in the repository, where
        // an administrator put it: an elevation naming `administer_engine`
        // adds nothing to an account that is not an administrator, whatever
        // was written into the session.
        //
        // `attenuated` is deliberately *not* set by this. It answers "was this
        // execution narrowed from the caller's own authority", which a session
        // at the tier it was minted with was not — and a refusal reading "this
        // execution was narrowed" would be wrong on every engine that gates
        // nothing. What an unelevated refusal should say is that the session
        // is unelevated, which is the elevation challenge's job.
        let mut narrowed = tier.attenuated(held);
        narrowed.attenuated = false;
        narrowed
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
            Capability::ManageMcp,
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
            // Carried forward untouched. Attenuating capabilities must never
            // *lift* a destination restriction, and a caller narrowing only the
            // verbs has said nothing about the destinations — so the answer to
            // "which hosts" is whatever it already was.
            network_scope: self.network_scope.clone(),
        }
    }

    /// The same narrowing, also bounding which hosts may be reached.
    ///
    /// `hosts` of `None` keeps whatever scope this context already has, which
    /// is what makes the two dimensions independent: narrowing the verbs does
    /// not widen the destinations and narrowing the destinations does not
    /// change the verbs.
    ///
    /// The caller is expected to have refused an out-of-scope request already
    /// (see [`UserContext::uncovered_hosts`]); this replaces rather than
    /// intersects, because computing a pattern set that means "everything both
    /// sides allow" is an algebra with edge cases, and refusing at the call is
    /// both simpler and the rule capabilities already follow.
    pub fn attenuated_to(
        &self,
        keep: impl IntoIterator<Item = Capability>,
        hosts: Option<NetworkScope>,
    ) -> Self {
        let mut narrowed = self.attenuated(keep);
        if let Some(hosts) = hosts {
            narrowed.network_scope = Some(Arc::new(hosts));
        }
        narrowed
    }

    /// Which of `hosts` this context could not reach itself.
    ///
    /// The destination counterpart of [`UserContext::unheld`], and refused for
    /// the same reason: a sub-execution asking to reach somewhere its caller
    /// cannot is a bug at the call, not a puzzling refusal from inside.
    ///
    /// Empty when this context has no scope, since an unrestricted caller
    /// covers everything.
    pub fn uncovered_hosts(&self, hosts: &NetworkScope) -> Vec<String> {
        let Some(scope) = &self.network_scope else {
            return Vec::new();
        };
        hosts
            .hosts()
            .filter(|pattern| !scope.covers_pattern(pattern))
            .map(str::to_string)
            .collect()
    }

    /// Whether this execution may reach `host`.
    ///
    /// True with no scope, which is every context built from a tier.
    pub fn may_reach(&self, host: &str) -> bool {
        self.network_scope
            .as_ref()
            .is_none_or(|scope| scope.permits(host))
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

    /// A wildcard matches subdomains and not the bare parent, which is the CSP
    /// and CORS rule and the one that makes a list say what it looks like it
    /// says.
    #[test]
    fn a_wildcard_matches_subdomains_only() {
        let scope = NetworkScope::new(["*.example.com", "api.other.test"]);

        assert!(scope.permits("a.example.com"));
        assert!(scope.permits("deep.nested.example.com"));
        assert!(scope.permits("API.OTHER.TEST"), "hosts compare case-folded");

        assert!(
            !scope.permits("example.com"),
            "a wildcard must not admit the parent; name it separately"
        );
        assert!(!scope.permits("other.test"), "an exact entry is exact");
        // The one that would matter: a suffix match rather than a label match
        // would admit this, and it is an attacker's domain.
        assert!(!scope.permits("example.com.evil.test"));
        assert!(
            !scope.permits(".example.com"),
            "an empty first label is not a subdomain"
        );
    }

    /// An empty scope is a real answer, not an absent one.
    #[test]
    fn an_empty_scope_permits_nothing() {
        let scope = NetworkScope::new(Vec::<String>::new());
        assert!(!scope.permits("example.com"));

        // And a context with no scope at all permits everything, which is
        // every tier and the behaviour that predates the field.
        assert!(UserContext::admin("a".to_string()).may_reach("anywhere.test"));
    }

    /// Covering a *pattern* is a different question from permitting a *host*,
    /// and getting them confused would widen rather than narrow.
    #[test]
    fn covering_a_wildcard_takes_a_wildcard() {
        let caller = NetworkScope::new(["api.example.com"]);
        assert!(caller.covers_pattern("api.example.com"));
        assert!(
            !caller.covers_pattern("*.example.com"),
            "holding one host does not cover every host under its parent"
        );

        let wide = NetworkScope::new(["*.example.com"]);
        assert!(wide.covers_pattern("*.example.com"));
        assert!(wide.covers_pattern("api.example.com"));
        assert!(
            wide.covers_pattern("*.api.example.com"),
            "a wildcard covers a narrower wildcard beneath it"
        );
        assert!(!wide.covers_pattern("example.com"));
    }

    /// Narrowing the verbs must not lift a destination bound.
    #[test]
    fn attenuating_capabilities_keeps_the_destination_bound() {
        let bounded = UserContext::admin("a".to_string()).attenuated_to(
            [Capability::UseNetwork],
            Some(NetworkScope::new(["a.test"])),
        );
        assert!(bounded.may_reach("a.test"));
        assert!(!bounded.may_reach("b.test"));

        // A further narrowing that says nothing about hosts inherits the bound
        // rather than clearing it.
        let deeper = bounded.attenuated([Capability::UseNetwork]);
        assert!(deeper.may_reach("a.test"));
        assert!(
            !deeper.may_reach("b.test"),
            "a narrowing that mentions no hosts must not widen them"
        );
    }

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
        assert!(!user.has_capability(&Capability::ManageMcp));
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
        assert!(user.has_capability(&Capability::ManageMcp));

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
        assert!(user.has_capability(&Capability::ManageMcp));
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
        assert!(user.require_capability(&Capability::ManageMcp).is_err());
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

    /// The tiers, from the one place that decides them. Each arm is the whole
    /// of what the four call sites used to spell for themselves.
    #[test]
    fn a_session_gets_the_tier_its_roles_name() {
        let administrator = UserContext::for_session(SessionRoles {
            user_id: Some("u"),
            is_admin: true,
            is_editor: false,
            elevation: None,
        });
        let editor = UserContext::for_session(SessionRoles {
            user_id: Some("u"),
            is_admin: false,
            is_editor: true,
            elevation: None,
        });
        let ordinary = UserContext::for_session(SessionRoles {
            user_id: Some("u"),
            is_admin: false,
            is_editor: false,
            elevation: None,
        });

        assert!(administrator.has_capability(&Capability::AdministerEngine));
        assert!(editor.has_capability(&Capability::WriteScripts));
        assert!(!editor.has_capability(&Capability::AdministerEngine));
        assert!(ordinary.has_capability(&Capability::ReadScripts));
        assert!(!ordinary.has_capability(&Capability::WriteScripts));
        assert_eq!(ordinary.user_id.as_deref(), Some("u"));
    }

    /// Administrator wins over editor, because the sets nest. A carrier
    /// setting both flags is ordinary — every sign-in that grants the
    /// administrator role grants the editor one beside it.
    #[test]
    fn administrator_outranks_editor_when_both_are_set() {
        let both = UserContext::for_session(SessionRoles {
            user_id: Some("u"),
            is_admin: true,
            is_editor: true,
            elevation: None,
        });

        assert!(both.has_capability(&Capability::AdministerEngine));
    }

    /// No identity is the anonymous tier whatever the flags claim.
    ///
    /// The flags travel beside the id in three different carrier structs, and
    /// one of them — `JsAuthContext` — can hold an id while not being
    /// authenticated. A carrier that is nobody and says it administers the
    /// engine is a bug, and the smallest tier is the right answer to it.
    #[test]
    fn a_caller_with_no_identity_is_anonymous_however_it_is_flagged() {
        let nobody = UserContext::for_session(SessionRoles::anonymous());
        let nobody_claiming_otherwise = UserContext::for_session(SessionRoles {
            user_id: None,
            is_admin: true,
            is_editor: true,
            elevation: None,
        });

        for context in [&nobody, &nobody_claiming_otherwise] {
            assert!(!context.is_authenticated);
            assert!(context.user_id.is_none());
            assert!(!context.has_capability(&Capability::AdministerEngine));
            assert!(!context.has_capability(&Capability::WriteScripts));
            assert!(context.has_capability(&Capability::ReadScripts));
        }
    }

    /// The default is the anonymous caller, which is what makes
    /// `.unwrap_or_default()` at a call site say what it means.
    #[test]
    fn the_default_roles_are_nobody() {
        assert_eq!(SessionRoles::default(), SessionRoles::anonymous());
        assert_eq!(SessionRoles::anonymous().user_id, None);
    }

    /// A tier built through `for_session` is not attenuated, so a refusal
    /// blames the tier rather than a narrowing that did not happen.
    ///
    /// This is the flag session elevation will set, so the unelevated case
    /// having it clear is what will make the elevated case legible.
    #[test]
    fn a_tier_from_a_session_is_not_attenuated() {
        let editor = UserContext::for_session(SessionRoles {
            user_id: Some("u"),
            is_admin: false,
            is_editor: true,
            elevation: None,
        });

        assert!(!editor.attenuated);
    }

    /// An engine that gates nothing hands a session its whole tier, elevation
    /// or no elevation. The default, and the behaviour that existed before
    /// any of this.
    #[test]
    fn with_nothing_gated_a_session_holds_its_whole_tier() {
        let administrator = UserContext::for_session(SessionRoles {
            user_id: Some("u"),
            is_admin: true,
            is_editor: true,
            elevation: None,
        });

        assert!(administrator.has_capability(&Capability::AdministerEngine));
        assert!(administrator.has_capability(&Capability::WriteScripts));
    }

    /// The ceiling stays in the repository. An elevation naming what the
    /// account's roles do not carry adds nothing, because the composition is
    /// an intersection — so writing one into a session is not a way to become
    /// an administrator.
    #[test]
    fn an_elevation_cannot_exceed_the_roles_it_sits_on() {
        let now = chrono::Utc::now();
        let overreaching = super::super::elevation::Elevation {
            capabilities: vec![
                Capability::AdministerEngine.as_str().to_string(),
                Capability::WriteScripts.as_str().to_string(),
            ],
            granted_at: now,
            expires_at: now + chrono::Duration::hours(1),
            method: super::super::elevation::Method::Password,
        };

        let ordinary = UserContext::for_session_at(
            SessionRoles {
                user_id: Some("u"),
                is_admin: false,
                is_editor: false,
                elevation: Some(&overreaching),
            },
            now,
        );

        assert!(!ordinary.has_capability(&Capability::AdministerEngine));
        assert!(!ordinary.has_capability(&Capability::WriteScripts));
        assert!(ordinary.has_capability(&Capability::ReadScripts));
    }

    /// An elevation belongs to the session and never to the identity: it does
    /// not change who the caller is, only what they may do.
    #[test]
    fn an_elevation_does_not_change_who_the_caller_is() {
        let now = chrono::Utc::now();
        let elevation = super::super::elevation::Elevation {
            capabilities: vec![Capability::AdministerEngine.as_str().to_string()],
            granted_at: now,
            expires_at: now + chrono::Duration::hours(1),
            method: super::super::elevation::Method::Provider,
        };

        let elevated = UserContext::for_session_at(
            SessionRoles {
                user_id: Some("u"),
                is_admin: true,
                is_editor: true,
                elevation: Some(&elevation),
            },
            now,
        );

        assert_eq!(elevated.user_id.as_deref(), Some("u"));
        assert!(elevated.is_authenticated);
        assert!(!elevated.attenuated);
    }
}
