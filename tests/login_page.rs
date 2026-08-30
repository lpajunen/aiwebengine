//! Tests for the sign-in page.
//!
//! The page is what makes internal credentials reachable from a browser at
//! all: a solution posts JSON from its own UI, but a personal install has only
//! this. Every control here is a plain form, because the engine's configured
//! Content-Security-Policy names `script-src 'self'` with no inline allowance
//! and a sign-in page is the last place to depend on that being relaxed.

mod common;

use aiwebengine::auth::config::InternalAuthConfig;
use aiwebengine::auth::routes::render_internal_auth_forms;

fn config(enabled: bool, registration: bool, guests: bool) -> InternalAuthConfig {
    InternalAuthConfig {
        enabled,
        allow_registration: registration,
        allow_guests: guests,
        min_password_length: 12,
    }
}

fn render(config: &InternalAuthConfig, signing_up: bool) -> String {
    render_internal_auth_forms(config, "csrf-token-value", "/after", "%2Fafter", signing_up)
}

/// Off by default means off on the page too: an engine with no internal
/// credentials configured must not show a form that cannot work.
#[test]
fn nothing_is_rendered_when_nothing_is_enabled() {
    assert_eq!(render(&config(false, false, false), false), "");
}

#[test]
fn the_sign_in_form_posts_to_the_login_endpoint() {
    let html = render(&config(true, false, false), false);
    assert!(html.contains(r#"action="/auth/local/login""#));
    assert!(html.contains(r#"name="username""#));
    assert!(html.contains(r#"type="password""#));
    assert!(
        !html.contains("/auth/local/register"),
        "registration is off, so the page must not offer it"
    );
    assert!(
        !html.contains("/auth/guest"),
        "guests are off, so the page must not offer them"
    );
}

/// Without this the form is forgeable from any page on the internet, which is
/// how login CSRF signs a victim into an attacker's account.
#[test]
fn every_form_carries_a_csrf_token() {
    let html = render(&config(true, true, true), false);
    let forms = html.matches("<form").count();
    let tokens = html
        .matches(r#"name="csrf_token" value="csrf-token-value""#)
        .count();
    assert_eq!(
        forms, tokens,
        "each of the {} forms needs its own token field",
        forms
    );
    assert!(forms >= 2, "sign-in and guest should both be present");
}

#[test]
fn the_redirect_target_is_carried_through_every_form() {
    let html = render(&config(true, true, true), false);
    assert_eq!(
        html.matches(r#"name="redirect" value="/after""#).count(),
        html.matches("<form").count()
    );
}

#[test]
fn registration_is_offered_only_when_it_is_allowed() {
    let without = render(&config(true, false, false), false);
    assert!(!without.contains("signup=1"));

    let with = render(&config(true, true, false), false);
    assert!(
        with.contains("signup=1"),
        "a link to the sign-up form should appear"
    );
}

#[test]
fn the_sign_up_form_posts_to_the_register_endpoint() {
    let html = render(&config(true, true, false), true);
    assert!(html.contains(r#"action="/auth/local/register""#));
    assert!(
        html.contains(r#"autocomplete="new-password""#),
        "browsers should offer to generate a password, not fill an old one"
    );
    assert!(
        html.contains(r#"name="name""#),
        "sign-up takes a display name"
    );
}

/// Asking the browser for what the engine will accept means a short password
/// is refused before it crosses the network, not after.
#[test]
fn the_password_field_advertises_the_configured_minimum() {
    let html = render(&config(true, false, false), false);
    assert!(html.contains(r#"minlength="12""#));
}

/// Configuration cannot lower the floor the engine enforces, so the form must
/// not advertise a lower one either.
#[test]
fn the_password_field_never_advertises_less_than_the_floor() {
    let html = render(&config(true, false, false), false);
    assert!(!html.contains(r#"minlength="1""#));

    let mut low = config(true, false, false);
    low.min_password_length = 1;
    let html = render(&low, false);
    assert!(
        html.contains(r#"minlength="8""#),
        "the floor is 8, whatever configuration says"
    );
}

#[test]
fn guests_are_offered_on_their_own_when_passwords_are_off() {
    let html = render(&config(false, false, true), false);
    assert!(html.contains(r#"action="/auth/guest""#));
    assert!(
        !html.contains(r#"type="password""#),
        "no password form when internal sign-in is disabled"
    );
}
