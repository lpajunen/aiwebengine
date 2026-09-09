//! What a browser holds while an OAuth2 login is away at the provider.
//!
//! The `state` parameter exists to tie the callback the engine receives back
//! to the authorization request the engine sent (RFC 6749 §10.12), which takes
//! something that is remembered between the two. What used to stand in for
//! that memory was the client's address: the state was
//! `provider:ip:random`, and the callback was accepted when the provider and
//! the address in it matched the request carrying it.
//!
//! That is wrong in both directions. It rejects logins that are fine — an
//! address is a property of the network path, not of the browser, so a client
//! that reaches the callback over a different route than it reached the login
//! page is refused. The clearest case is a dual-stack client, where the
//! address is not even representable in that format: an IPv6 address contains
//! colons, so `google:2001:db8::1:8134` splits into eight fields and the state
//! is refused before anything is compared. Happy Eyeballs picks a family per
//! connection, which is exactly the intermittent failure this replaces —
//! retrying the login opens a new connection and often lands on the other
//! family. And it accepts callbacks that are not: `random` was never checked
//! against anything, so any address a caller could name was a valid state for
//! it, and the whole parameter proved nothing.
//!
//! What is remembered instead is a cookie the engine sets when the login
//! starts. The `state` sent to the provider is an opaque nonce, and the
//! callback is accepted only when the browser presents a cookie holding that
//! same nonce — a value an attacker cannot obtain, because it was never sent
//! to them, and cannot write, because the cookie is `HttpOnly` and host-only.
//! Nothing about the network path is consulted. `SameSite=Lax` is what lets
//! this work at all: the provider's redirect back is a top-level GET, which is
//! the navigation Lax permits a cookie on.
//!
//! The redirect target travels in the cookie rather than in the state. It used
//! to be base64 inside the state parameter, which meant a value the engine had
//! handed nobody, arriving from the URL bar, decoded and followed — held to a
//! local path only by [`super::routes::safe_redirect_target`]. In the cookie it
//! is a value only the engine has ever written.
//!
//! Several logins may be in flight at once, because a person with a slow
//! provider opens a second tab. The cookie holds up to [`MAX_PENDING`] of
//! them, newest last, so starting a second login does not invalidate the first.

use base64::Engine;
use serde::{Deserialize, Serialize};
use subtle::ConstantTimeEq;

use super::host_scoped_cookie_name;

/// How long a started login stays completable.
///
/// Long enough to cover a provider that asks for a password, a second factor
/// and a consent screen; short enough that a nonce left in a cookie on a
/// shared machine is not a standing invitation. The cookie's `Max-Age` is the
/// same value, so the browser drops it on the same schedule the engine would.
pub const PENDING_TTL_SECS: i64 = 900;

/// How many logins may be in flight in one browser at once.
const MAX_PENDING: usize = 3;

/// The cookie's name before host scoping.
const COOKIE_NAME: &str = "auth_oauth_state";

/// One login that has been sent to a provider and not yet come back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PendingLogin {
    /// The value sent to the provider as `state`, and the only part of this
    /// that the browser's URL ever carries.
    pub nonce: String,

    /// Which provider the flow was started against. Checked at the callback
    /// because the provider comes from the callback's path, and a nonce issued
    /// for one provider should not complete a flow for another.
    pub provider: String,

    /// Where to send the browser once the session exists.
    #[serde(default)]
    pub redirect: Option<String>,

    /// Unix seconds this login was started at.
    pub issued_at: i64,
}

impl PendingLogin {
    /// Start a login: a fresh nonce and the moment it was issued.
    pub fn new(provider: &str, redirect: Option<String>, now: i64) -> Self {
        Self {
            nonce: new_nonce(),
            provider: provider.to_string(),
            redirect,
            issued_at: now,
        }
    }

    /// Whether this login is still completable at `now`.
    pub fn is_live(&self, now: i64) -> bool {
        now < self.issued_at.saturating_add(PENDING_TTL_SECS)
    }
}

/// A nonce with enough entropy that guessing it is not an attack: 256 bits,
/// which is what the whole scheme rests on now that nothing else is checked.
fn new_nonce() -> String {
    let bytes: [u8; 32] = rand::random();
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The cookie's real name, `__Host-` prefixed wherever the browser will accept
/// it — the same reasoning as the session cookie's, and for a value that binds
/// a login to one host it matters just as much. See [`host_scoped_cookie_name`].
pub fn cookie_name(secure: bool) -> String {
    host_scoped_cookie_name(COOKIE_NAME, secure)
}

/// Read the pending logins a request's cookies carry.
///
/// Anything unreadable — a truncated cookie, one written by an older version,
/// one somebody made up — is no pending logins rather than an error. The
/// caller's next step is to look for a matching nonce, and an empty list
/// simply has none.
pub fn decode(value: &str) -> Vec<PendingLogin> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Vec<PendingLogin>>(&bytes).ok())
        .unwrap_or_default()
}

/// The cookie value carrying `pending`, newest last.
pub fn encode(pending: &[PendingLogin]) -> String {
    let json = serde_json::to_vec(pending).unwrap_or_else(|_| b"[]".to_vec());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json)
}

/// Add `login` to whatever is already in flight, dropping expired entries and
/// keeping the newest [`MAX_PENDING`].
pub fn push(existing: Vec<PendingLogin>, login: PendingLogin, now: i64) -> Vec<PendingLogin> {
    let mut kept: Vec<PendingLogin> = existing
        .into_iter()
        .filter(|entry| entry.is_live(now) && entry.nonce != login.nonce)
        .collect();

    kept.push(login);

    let overflow = kept.len().saturating_sub(MAX_PENDING);
    kept.drain(..overflow);
    kept
}

/// The pending login a callback's `state` names, if the browser holds one.
///
/// The nonce is compared in constant time. That is cheap here and the habit is
/// worth keeping: this value is the whole of the check, and a comparison that
/// stops at the first differing byte answers questions about a secret.
pub fn take(
    pending: &[PendingLogin],
    state: &str,
    provider: &str,
    now: i64,
) -> Option<PendingLogin> {
    pending
        .iter()
        .find(|entry| {
            entry.provider == provider
                && entry.is_live(now)
                && bool::from(entry.nonce.as_bytes().ct_eq(state.as_bytes()))
        })
        .cloned()
}

/// `Set-Cookie` carrying the logins still in flight.
pub fn set_cookie(value: &str, secure: bool) -> String {
    format!(
        "{}={}; Path=/; HttpOnly; SameSite=Lax; Max-Age={}{}",
        cookie_name(secure),
        value,
        PENDING_TTL_SECS,
        if secure { "; Secure" } else { "" }
    )
}

/// `Set-Cookie` that removes the cookie, sent when the login that completed
/// was the last one in flight.
///
/// A callback that matched nothing leaves the cookie alone: what it holds may
/// still be another tab's login, and clearing it would turn one failed
/// callback into a second failed login.
pub fn clear_cookie(secure: bool) -> String {
    format!(
        "{}=; Path=/; HttpOnly; SameSite=Lax; Max-Age=0{}",
        cookie_name(secure),
        if secure { "; Secure" } else { "" }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_700_000_000;

    #[test]
    fn a_started_login_completes() {
        let login = PendingLogin::new("google", Some("/dashboard".to_string()), NOW);
        let pending = push(Vec::new(), login.clone(), NOW);

        let round_tripped = decode(&encode(&pending));
        let taken = take(&round_tripped, &login.nonce, "google", NOW + 30)
            .expect("the browser's own nonce completes the login it started");

        assert_eq!(taken.redirect.as_deref(), Some("/dashboard"));
    }

    /// The failure this module exists to remove: nothing about the address a
    /// request arrived from is consulted, so a client whose route changed
    /// between the two requests still completes.
    #[test]
    fn nothing_here_depends_on_the_client_address() {
        let login = PendingLogin::new("google", None, NOW);
        let pending = push(Vec::new(), login.clone(), NOW);

        assert!(take(&pending, &login.nonce, "google", NOW + 300).is_some());
    }

    #[test]
    fn a_nonce_the_browser_never_received_is_refused() {
        let login = PendingLogin::new("google", None, NOW);
        let pending = push(Vec::new(), login, NOW);

        assert!(take(&pending, &new_nonce(), "google", NOW).is_none());
    }

    #[test]
    fn a_nonce_issued_for_another_provider_is_refused() {
        let login = PendingLogin::new("google", None, NOW);
        let pending = push(Vec::new(), login.clone(), NOW);

        assert!(take(&pending, &login.nonce, "microsoft", NOW).is_none());
    }

    #[test]
    fn a_login_left_open_expires() {
        let login = PendingLogin::new("google", None, NOW);
        let pending = push(Vec::new(), login.clone(), NOW);

        assert!(take(&pending, &login.nonce, "google", NOW + PENDING_TTL_SECS).is_none());
    }

    /// Two tabs, because a person whose provider is slow opens one.
    #[test]
    fn concurrent_logins_do_not_evict_each_other() {
        let first = PendingLogin::new("google", Some("/one".to_string()), NOW);
        let second = PendingLogin::new("google", Some("/two".to_string()), NOW + 5);

        let pending = push(
            push(Vec::new(), first.clone(), NOW),
            second.clone(),
            NOW + 5,
        );

        assert_eq!(
            take(&pending, &first.nonce, "google", NOW + 10)
                .and_then(|entry| entry.redirect)
                .as_deref(),
            Some("/one")
        );
        assert_eq!(
            take(&pending, &second.nonce, "google", NOW + 10)
                .and_then(|entry| entry.redirect)
                .as_deref(),
            Some("/two")
        );
    }

    #[test]
    fn the_oldest_login_goes_when_more_than_three_are_open() {
        let oldest = PendingLogin::new("google", None, NOW);
        let mut pending = push(Vec::new(), oldest.clone(), NOW);
        for offset in 1..=MAX_PENDING as i64 {
            pending = push(
                pending,
                PendingLogin::new("google", None, NOW + offset),
                NOW + offset,
            );
        }

        assert_eq!(pending.len(), MAX_PENDING);
        assert!(take(&pending, &oldest.nonce, "google", NOW + 10).is_none());
    }

    #[test]
    fn expired_logins_are_dropped_when_a_new_one_starts() {
        let stale = PendingLogin::new("google", None, NOW);
        let fresh = PendingLogin::new("google", None, NOW + PENDING_TTL_SECS + 1);

        let pending = push(
            push(Vec::new(), stale.clone(), NOW),
            fresh.clone(),
            NOW + PENDING_TTL_SECS + 1,
        );

        assert_eq!(pending, vec![fresh]);
    }

    #[test]
    fn a_cookie_from_nowhere_carries_no_logins() {
        assert!(decode("").is_empty());
        assert!(decode("not-base64-$$$").is_empty());
        assert!(decode(&base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("{}")).is_empty());
    }

    /// The cookie is `HttpOnly` — a script cannot read the nonce, and where a
    /// browser enforces `__Host-`, a sibling host cannot write one.
    #[test]
    fn the_cookie_is_scoped_and_unreadable_by_scripts() {
        let secure = set_cookie("value", true);
        assert!(secure.starts_with("__Host-auth_oauth_state="));
        assert!(secure.contains("HttpOnly"));
        assert!(secure.contains("SameSite=Lax"));
        assert!(secure.contains("; Secure"));

        // Over plain HTTP a `__Host-` cookie is discarded, taking sign-in with
        // it, so local development keeps the bare name.
        let insecure = set_cookie("value", false);
        assert!(insecure.starts_with("auth_oauth_state="));
        assert!(!insecure.contains("Secure"));
    }

    #[test]
    fn clearing_expires_the_same_cookie() {
        assert!(clear_cookie(true).starts_with("__Host-auth_oauth_state=;"));
        assert!(clear_cookie(true).contains("Max-Age=0"));
    }
}
