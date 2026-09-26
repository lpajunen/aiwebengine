//! Who may read this, when the engine answers without running a handler.
//!
//! A capability names a verb the engine understands — `ReadScriptData`,
//! `UseNetwork`, `ManageStreams`. "This person may subscribe to
//! `/orders/1234/events`" is a fact about the *script's own data model*: whose
//! order that is. The engine cannot know it, and `script_owners` answers who
//! may edit a script rather than who may read a row. So per-resource
//! authorization has to be script code. It is not a mechanism competing with
//! capabilities; it is the layer capabilities structurally cannot reach.
//!
//! Where a handler already runs, the handler *is* the hook — it runs under the
//! requesting user's context and decides for itself. Where the engine moves
//! bytes without running one — a stream's connection, an asset route — the
//! script supplies a function that decides and the engine does the moving.
//! That is what keeps an authorized asset worth having: the decision is
//! JavaScript, the data path is not.
//!
//! This module is the shape those functions answer in, settled once so the two
//! surfaces cannot drift into two conventions.

use std::collections::HashMap;

/// The status a deny answers with when the callback does not name one.
const DEFAULT_DENY_STATUS: u16 = 403;

/// What a per-resource authorization callback decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessDecision {
    /// Allowed. The map is filter criteria for the surfaces that take them —
    /// a stream's connection — and is empty for those that do not.
    Allow(HashMap<String, String>),
    /// Refused, with the status to answer and an optional reason.
    Deny { status: u16, reason: Option<String> },
}

impl AccessDecision {
    /// Allowed, with no criteria.
    pub fn allow() -> Self {
        AccessDecision::Allow(HashMap::new())
    }

    /// Whether this decision refuses the caller.
    pub fn is_denied(&self) -> bool {
        matches!(self, AccessDecision::Deny { .. })
    }
}

/// Narrow a status a callback asked for to one a deny may actually answer.
///
/// Any 4xx is the caller's to choose: `401` to say "sign in", `403` to say
/// "not yours", `404` to say "and I will not tell you it exists", `429` to
/// throttle. A 5xx is not — a refusal is not the server failing, and
/// answering one would tell every client to retry something that will be
/// refused again. Anything outside the range is a bug in the callback, and it
/// is reported and treated as a plain refusal rather than allowed to become
/// an accidental success.
pub fn deny_status(requested: Option<i64>) -> u16 {
    match requested {
        None => DEFAULT_DENY_STATUS,
        Some(status) if (400..=499).contains(&status) => status as u16,
        Some(other) => {
            tracing::warn!(
                "An authorization callback asked to deny with status {}, which is not a client \
                 error; answering {} instead",
                other,
                DEFAULT_DENY_STATUS
            );
            DEFAULT_DENY_STATUS
        }
    }
}

/// The reason a refusal carries back to the client, if any.
///
/// Trimmed and bounded, because it is written by a script and returned to
/// whoever was refused. A refusal is a place where a solution accidentally
/// says more than it meant to, so this is the one string from the callback
/// that reaches an unauthenticated reader.
pub fn deny_reason(reason: Option<String>) -> Option<String> {
    const MAX_REASON_CHARS: usize = 200;
    reason
        .map(|text| {
            text.trim()
                .chars()
                .take(MAX_REASON_CHARS)
                .collect::<String>()
        })
        .filter(|text| !text.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deny_with_no_status_is_forbidden() {
        assert_eq!(deny_status(None), 403);
    }

    #[test]
    fn any_client_error_is_the_callers_to_choose() {
        assert_eq!(deny_status(Some(401)), 401);
        assert_eq!(deny_status(Some(404)), 404);
        assert_eq!(deny_status(Some(429)), 429);
    }

    /// A refusal is not the server failing. Answering 500 would tell the
    /// client to retry something that will be refused again, which is the
    /// behaviour this whole shape exists to replace.
    #[test]
    fn a_server_error_is_not_a_refusal_a_script_may_ask_for() {
        assert_eq!(deny_status(Some(500)), 403);
        assert_eq!(deny_status(Some(200)), 403);
        assert_eq!(deny_status(Some(0)), 403);
    }

    #[test]
    fn a_reason_is_trimmed_and_an_empty_one_is_no_reason() {
        assert_eq!(
            deny_reason(Some("  not yours  ".into())),
            Some("not yours".into())
        );
        assert_eq!(deny_reason(Some("   ".into())), None);
        assert_eq!(deny_reason(None), None);
    }

    #[test]
    fn a_reason_cannot_be_unbounded() {
        let long = "x".repeat(5_000);
        assert_eq!(
            deny_reason(Some(long)).map(|r| r.chars().count()),
            Some(200)
        );
    }
}
