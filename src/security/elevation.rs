//! What a session may do *right now*, as distinct from what its account may
//! ever do.
//!
//! A session carries the roles it was minted with and nothing narrows them
//! afterwards. Sign in as an administrator and every request for the next
//! thirty days — every tab, every stolen cookie, every agent turn running as
//! you — holds [`Capability::AdministerEngine`]. The role answers *may this
//! person ever do this*; nothing answered *is this person doing it now, on
//! purpose*.
//!
//! This is that second answer, and it is deliberately the same mechanism as
//! the two narrowings the engine already has. [`crate::sandbox`] narrows an
//! execution, [`crate::delegation`] narrows a background run, and both are
//! [`UserContext::attenuated`], an intersection that can only take away. The
//! missing row was the one that applies to *everything*, because it sits on
//! the credential rather than on an execution — and every API in the engine
//! resolves a credential into a `UserContext` before it authorizes anything.
//!
//! See `docs/SESSION_ELEVATION.md` for the whole design, including the parts
//! not built yet: the elevation page, re-authentication, and the `scope=` an
//! OAuth token would carry.
//!
//! # The bundles are the questions the engine already asks
//!
//! [`Grade::capabilities`] is computed as the *difference between tiers*
//! rather than written out as a list. `Author` is exactly what an editor holds
//! beyond an ordinary user and `Administer` is exactly what an administrator
//! holds beyond an editor, so a capability added to a tier lands in the right
//! bundle with nobody remembering to put it there. A hand-written list would
//! be a second place to update and therefore a place to forget.
//!
//! # Nothing is gated by default
//!
//! [`Policy::gated`] is empty unless an operator names something, and an empty
//! policy is today's behaviour byte for byte. That is the one dial: there is
//! no separate `enabled`, because "enabled with nothing gated" and "disabled"
//! are the same engine and two ways to say it is one way too many.

use super::capabilities::UserContext;
use super::validation::Capability;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::sync::OnceLock;

/// A named bundle of authority a session can switch on.
///
/// Two, because the engine already asks exactly two questions of a caller who
/// wants to change something: may you author solutions at all, and may you act
/// on what you do not own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Grade {
    /// Create and change the scripts you own.
    ///
    /// Ownership is still checked separately at every write
    /// (`repository::user_owns_script`), so this alone never reaches somebody
    /// else's solution. That is why it can be a bundle rather than a list of
    /// scripts.
    Author,
    /// Act on scripts, users and secrets you do not own.
    Administer,
}

impl Grade {
    pub fn as_str(self) -> &'static str {
        match self {
            Grade::Author => "author",
            Grade::Administer => "administer",
        }
    }

    /// An unknown name is refused rather than dropped — the rule
    /// [`Capability::parse`] states, for the same reason: a policy naming
    /// `admin` would otherwise gate nothing while looking like it gated
    /// everything.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "author" => Some(Grade::Author),
            "administer" => Some(Grade::Administer),
            _ => None,
        }
    }

    /// What the elevation page says this switches on.
    ///
    /// Written for the person deciding, not for the developer asking — the
    /// rule [`crate::delegation::Scope::describe`] set, and the reason these
    /// three consent surfaces should share one vocabulary rather than three.
    pub fn describe(self) -> &'static str {
        match self {
            Grade::Author => "Create and change the scripts you own",
            Grade::Administer => "Act on scripts, users and secrets you do not own",
        }
    }

    /// Whether reaching this bundle takes the administrator role.
    ///
    /// Advisory: the ceiling is enforced by the intersection in
    /// [`UserContext::for_session`] whatever this says. It exists so the
    /// elevation page can show a bundle as unavailable with a reason, rather
    /// than offering somebody a button that would silently grant nothing.
    /// Both bundles take the editor role, so there is no question to ask
    /// about that one.
    pub fn requires_administrator(self) -> bool {
        matches!(self, Grade::Administer)
    }

    pub fn all() -> [Grade; 2] {
        [Grade::Author, Grade::Administer]
    }

    /// What this bundle carries, as the difference between two tiers.
    ///
    /// Derived rather than listed, so it cannot drift from the tiers it is
    /// defined in terms of.
    pub fn capabilities(self) -> HashSet<Capability> {
        let who = "elevation".to_string();
        let (wider, narrower) = match self {
            Grade::Author => (
                UserContext::editor(who.clone()),
                UserContext::authenticated(who),
            ),
            Grade::Administer => (UserContext::admin(who.clone()), UserContext::editor(who)),
        };
        wider
            .capabilities
            .difference(&narrower.capabilities)
            .cloned()
            .collect()
    }
}

/// How the person proved they were present when the elevation was granted.
///
/// Recorded for the audit line rather than checked: by the time an elevation
/// exists the proof has already been accepted. What it answers later is "was
/// somebody at a keyboard for this", which is the question asked after
/// something has gone wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    /// A local account re-entered its password.
    Password,
    /// A federated account was sent back to its provider.
    Provider,
    /// An OAuth2 consent screen, for a token rather than a browser.
    Consent,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Password => "password",
            Method::Provider => "provider",
            Method::Consent => "consent",
        }
    }
}

/// What a session holds beyond the floor, and until when.
///
/// Capability *names* rather than the enum, for the reason the session blob
/// generally prefers them: a capability renamed or removed drops out of an
/// existing session instead of failing to deserialise it, and dropping is the
/// direction that takes authority away.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Elevation {
    pub capabilities: Vec<String>,
    pub granted_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub method: Method,
}

impl Elevation {
    /// Grant `grades` for `minutes`, bounded by the configured ceiling.
    pub fn grant(grades: &[Grade], method: Method, minutes: i64, now: DateTime<Utc>) -> Self {
        let mut names: Vec<String> = grades
            .iter()
            .flat_map(|grade| grade.capabilities())
            .map(|capability| capability.as_str().to_string())
            .collect();
        names.sort_unstable();
        names.dedup();

        Self {
            capabilities: names,
            granted_at: now,
            expires_at: now + Duration::minutes(configured().bounded_minutes(minutes)),
            method,
        }
    }

    /// Whether this elevation is still in force.
    pub fn is_live(&self, now: DateTime<Utc>) -> bool {
        self.expires_at > now
    }

    /// The capabilities it carries, dropping any name this engine no longer
    /// knows.
    pub fn capabilities(&self) -> HashSet<Capability> {
        self.capabilities
            .iter()
            .filter_map(|name| Capability::parse(name))
            .collect()
    }

    /// How much longer it lasts, floored at zero.
    pub fn remaining(&self, now: DateTime<Utc>) -> Duration {
        (self.expires_at - now).max(Duration::zero())
    }
}

/// What an engine gates, and for how long.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    /// Bundles that require elevating. Empty — the default — is the engine as
    /// it behaved before any of this existed.
    pub gated: Vec<Grade>,
    /// The longest an elevation may last.
    pub max_minutes: i64,
    /// How recently the person must have authenticated to be granted one.
    pub reauth_window_secs: i64,
}

impl Default for Policy {
    fn default() -> Self {
        Self {
            gated: Vec::new(),
            // Long enough to finish a piece of work, short enough that a
            // stolen session is worth little. sudo's own timestamp is fifteen
            // minutes; an hour is the concession to a person editing a
            // solution rather than running one command.
            max_minutes: 60,
            // Two minutes. The window is "did they just type their password",
            // not "have they been here today".
            reauth_window_secs: 120,
        }
    }
}

impl Policy {
    /// Whether anything is gated at all.
    pub fn is_active(&self) -> bool {
        !self.gated.is_empty()
    }

    pub fn gates(&self, grade: Grade) -> bool {
        self.gated.contains(&grade)
    }

    /// A requested duration, held to the ceiling.
    ///
    /// Clamped rather than refused, unlike
    /// [`crate::security::script_crypto::random_token`]: asking for longer
    /// than the ceiling is asking for something the operator has already
    /// decided, and the caller gets what the operator allows rather than an
    /// error it can do nothing about. Asking for nothing at all is a bug in
    /// the caller and gets the ceiling too, since an elevation that expires
    /// before the redirect completes is indistinguishable from one that was
    /// never granted.
    pub fn bounded_minutes(&self, asked: i64) -> i64 {
        asked.clamp(1, self.max_minutes.max(1))
    }
}

/// The policy this engine was configured with, set once at startup.
static CONFIGURED: OnceLock<Policy> = OnceLock::new();

/// Records the configured policy. Returns false if it was already set.
pub fn configure(policy: Policy) -> bool {
    CONFIGURED.set(policy).is_ok()
}

/// The policy in effect: what was configured at startup, or the defaults —
/// which gate nothing.
pub fn configured() -> Policy {
    CONFIGURED.get().cloned().unwrap_or_default()
}

/// Which capabilities a session holds, given its tier and its elevation.
///
/// The composition, in one place:
///
/// - everything the tier carries that no *gated* bundle claims — so an engine
///   gating nothing returns the tier untouched, and one gating only
///   `administer` leaves an editor working exactly as before;
/// - plus whatever a live elevation names.
///
/// Then [`UserContext::for_session`] intersects that with the tier, which is
/// what keeps the ceiling in the repository where an administrator put it: an
/// elevation naming `administer_engine` adds nothing to an account that is not
/// an administrator.
pub fn held_capabilities(
    tier: &UserContext,
    elevation: Option<&Elevation>,
    now: DateTime<Utc>,
) -> HashSet<Capability> {
    held_under(&configured(), tier, elevation, now)
}

/// [`held_capabilities`] against a policy the caller names.
///
/// The policy is a `OnceLock` set at startup, which is right for the engine
/// and useless for a test that needs to describe more than one. So the
/// arithmetic takes its policy as an argument and the reader above supplies
/// the configured one — the same shape `log_retention::prune` uses, and for
/// the same reason.
pub fn held_under(
    policy: &Policy,
    tier: &UserContext,
    elevation: Option<&Elevation>,
    now: DateTime<Utc>,
) -> HashSet<Capability> {
    if !policy.is_active() {
        return tier.capabilities.clone();
    }

    let gated: HashSet<Capability> = policy
        .gated
        .iter()
        .flat_map(|grade| grade.capabilities())
        .collect();

    let mut keep: HashSet<Capability> = tier.capabilities.difference(&gated).cloned().collect();

    if let Some(live) = elevation.filter(|elevation| elevation.is_live(now)) {
        keep.extend(live.capabilities());
    }

    keep
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grades_capabilities(grades: &[Grade]) -> HashSet<Capability> {
        grades
            .iter()
            .flat_map(|grade| grade.capabilities())
            .collect()
    }

    /// The floor plus both bundles is exactly what an administrator holds.
    ///
    /// The test that keeps the bundles honest. A capability added to a tier
    /// and to neither bundle would be one nothing could ever elevate to and
    /// nothing could ever take away, and it would be invisible until somebody
    /// went looking for why a button did nothing.
    #[test]
    fn the_floor_and_both_bundles_are_the_whole_of_an_administrator() {
        let administrator = UserContext::admin("a".to_string());
        let floor = UserContext::authenticated("a".to_string());

        let mut rebuilt = floor.capabilities.clone();
        rebuilt.extend(grades_capabilities(&Grade::all()));

        assert_eq!(rebuilt, administrator.capabilities);
    }

    /// The bundles do not overlap, so elevating to one says nothing about the
    /// other.
    #[test]
    fn the_bundles_are_disjoint() {
        let author = Grade::Author.capabilities();
        let administer = Grade::Administer.capabilities();

        assert!(author.is_disjoint(&administer));
        assert!(author.contains(&Capability::WriteScripts));
        assert_eq!(
            administer,
            HashSet::from([Capability::AdministerEngine]),
            "administering is the one marker for acting on what you do not own"
        );
    }

    /// An engine that gates nothing is the engine that existed before this.
    #[test]
    fn gating_nothing_leaves_every_tier_untouched() {
        let now = Utc::now();
        for tier in [
            UserContext::anonymous(),
            UserContext::authenticated("u".to_string()),
            UserContext::editor("u".to_string()),
            UserContext::admin("u".to_string()),
        ] {
            assert_eq!(
                held_under(&Policy::default(), &tier, None, now),
                tier.capabilities,
                "an inactive policy must change nothing"
            );
        }
    }

    /// With a bundle gated, an unelevated session does not hold it, and a live
    /// elevation puts it back.
    #[test]
    fn a_gated_bundle_needs_an_elevation() {
        let now = Utc::now();
        let policy = Policy {
            gated: vec![Grade::Administer],
            ..Policy::default()
        };
        let administrator = UserContext::admin("u".to_string());

        let unelevated = held_under(&policy, &administrator, None, now);
        assert!(!unelevated.contains(&Capability::AdministerEngine));
        assert!(
            unelevated.contains(&Capability::WriteScripts),
            "gating one bundle must not disturb the other"
        );

        let elevation = Elevation {
            capabilities: vec![Capability::AdministerEngine.as_str().to_string()],
            granted_at: now,
            expires_at: now + Duration::minutes(30),
            method: Method::Password,
        };
        let elevated = held_under(&policy, &administrator, Some(&elevation), now);
        assert!(elevated.contains(&Capability::AdministerEngine));
    }

    /// An elevation that has run out is an elevation that is not there.
    #[test]
    fn an_expired_elevation_holds_nothing() {
        let now = Utc::now();
        let policy = Policy {
            gated: vec![Grade::Administer],
            ..Policy::default()
        };
        let expired = Elevation {
            capabilities: vec![Capability::AdministerEngine.as_str().to_string()],
            granted_at: now - Duration::hours(2),
            expires_at: now - Duration::minutes(1),
            method: Method::Password,
        };

        let held = held_under(
            &policy,
            &UserContext::admin("u".to_string()),
            Some(&expired),
            now,
        );

        assert!(!held.contains(&Capability::AdministerEngine));
        assert!(!expired.is_live(now));
        assert_eq!(expired.remaining(now), Duration::zero());
    }

    /// A name this engine does not know is dropped rather than failing the
    /// whole elevation — the direction that takes authority away.
    #[test]
    fn an_unknown_capability_name_is_simply_not_held() {
        let now = Utc::now();
        let elevation = Elevation {
            capabilities: vec![
                "write_scripts".to_string(),
                "use_script_database".to_string(),
            ],
            granted_at: now,
            expires_at: now + Duration::minutes(5),
            method: Method::Provider,
        };

        assert_eq!(
            elevation.capabilities(),
            HashSet::from([Capability::WriteScripts])
        );
    }

    /// A grant carries the bundle's capabilities and no more, sorted so that
    /// two equal grants are equal.
    #[test]
    fn a_grant_carries_what_was_asked_for() {
        let now = Utc::now();
        let grant = Elevation::grant(&[Grade::Author], Method::Password, 30, now);

        assert_eq!(grant.capabilities(), Grade::Author.capabilities());
        assert!(grant.is_live(now));
        assert!(!grant.is_live(now + Duration::minutes(31)));

        let mut sorted = grant.capabilities.clone();
        sorted.sort_unstable();
        assert_eq!(grant.capabilities, sorted, "names are stored sorted");
    }

    /// A duration is held to the ceiling rather than refused, and never
    /// reaches zero.
    #[test]
    fn a_requested_duration_is_bounded() {
        let policy = Policy {
            max_minutes: 60,
            ..Policy::default()
        };

        assert_eq!(policy.bounded_minutes(30), 30);
        assert_eq!(policy.bounded_minutes(600), 60);
        assert_eq!(policy.bounded_minutes(0), 1);
        assert_eq!(policy.bounded_minutes(-5), 1);
    }

    /// Every name round-trips, so a bundle added to one half cannot be missing
    /// from the other.
    #[test]
    fn grade_names_round_trip() {
        for grade in Grade::all() {
            assert_eq!(Grade::parse(grade.as_str()), Some(grade));
        }
        assert_eq!(Grade::parse("AUTHOR"), Some(Grade::Author));
        assert_eq!(Grade::parse("admin"), None);
        assert_eq!(Grade::parse(""), None);
    }
}
