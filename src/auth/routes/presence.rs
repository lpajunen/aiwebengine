//! Proving the person is at the keyboard before a session does something it
//! cannot take back: switching on rights (`/auth/elevate`) or deleting the
//! account (`/auth/account/delete`).
//!
//! A cookie is not proof. A stolen one, a stray tab or a script running as the
//! person all hold it, so both pages ask again: a local account types its
//! password, and a federated one must have signed in within
//! `security.elevation.reauth_window_secs`. See `docs/SESSION_ELEVATION.md`.

use super::*;
use crate::auth::AuthManager;
use crate::auth::session::AuthSession;
use crate::security::elevation::Method;

/// The form fields that ask for proof, and whether the form can be submitted.
///
/// `here` is where a fresh sign-in comes back to, and `asking` finishes the
/// sentence "… needs you to sign in again" for the page's own purpose.
pub(super) fn presence_fields(
    auth_manager: &AuthManager,
    session: &AuthSession,
    username: Option<&str>,
    host: Option<&str>,
    here: &str,
    asking: &str,
) -> (String, bool) {
    let recently = signed_in_recently(session);
    let fields = match (username, recently) {
        (Some(_), _) => r#"<label class="aw-field">Your password
                <input type="password" name="password" autocomplete="current-password" required>
            </label>"#
            .to_string(),
        (None, true) => r#"<p class="aw-explain">You signed in a moment ago, so that is proof
        enough this time.</p>"#
            .to_string(),
        // The provider can be asked to check them again, so offer that rather
        // than a sign-out: it is one click, it comes back here, and it is a
        // real challenge rather than a round trip answered from a cookie.
        // Without `prompt=login` the provider answers from its own session and
        // nobody proves anything, so a provider that cannot be asked is not
        // offered a button.
        (None, false) if auth_manager.can_force_reauthentication(host, &session.provider) => {
            format!(
                r#"<p class="aw-explain">{asking} needs you to sign in again — that is
        what asking is for.</p>
        <p class="aw-small"><a href="/auth/login/{provider_path}?prompt=login&amp;redirect={back}">Check
        me again with {provider}</a></p>"#,
                asking = html_escape::encode_text(asking),
                provider_path = html_attribute(&session.provider),
                provider = html_escape::encode_text(&session.provider),
                back = html_attribute(&urlencoding::encode(here)),
            )
        }
        (None, false) => format!(
            r#"<p class="aw-explain">This account signs in through {provider}, which the engine
        cannot ask to check you again. <a href="/auth/logout">Sign out and back in</a>, then
        come straight here.</p>"#,
            provider = html_escape::encode_text(&session.provider),
        ),
    };
    (fields, username.is_some() || recently)
}

/// Check the proof a form carried, and say how it was given.
///
/// A password when the account has one — checked against the credential
/// rather than trusted from the session, and throttled per address and per
/// account exactly as `login_local` is, since a guess here is worth more than
/// a sign-in. Otherwise a sign-in within the window, and deliberately not a
/// round trip to the provider: without `prompt=login` it answers from its own
/// cookie, and calling that re-authentication would claim what the engine
/// cannot back.
pub(super) async fn prove_presence(
    auth_manager: &AuthManager,
    session: &AuthSession,
    password: Option<&str>,
    ip_addr: &str,
) -> Result<Method, crate::auth::error::AuthError> {
    use crate::auth::error::AuthError;

    let username = crate::auth::local::username_for_user(&session.user_id)
        .await
        .map_err(|e| {
            tracing::error!("Could not read the credential to prove presence: {}", e);
            AuthError::Internal("could not check the credential".into())
        })?;

    let Some(username) = username else {
        return if signed_in_recently(session) {
            Ok(Method::Provider)
        } else {
            Err(AuthError::AuthenticationRequired)
        };
    };

    let security = auth_manager.security_context();
    if !security.check_auth_rate_limit(ip_addr).await
        || !security.account_login_allowed(&username).await
    {
        return Err(AuthError::RateLimitExceeded);
    }
    if crate::auth::local::verify_user_password(&session.user_id, password.unwrap_or_default())
        .await
        .is_err()
    {
        security.record_account_login_failure(&username).await;
        return Err(AuthError::InvalidCredentials);
    }
    Ok(Method::Password)
}

fn signed_in_recently(session: &AuthSession) -> bool {
    let window =
        chrono::Duration::seconds(crate::security::elevation::configured().reauth_window_secs);
    session
        .reauthenticated_at
        .is_some_and(|at| chrono::Utc::now() - at < window)
}
