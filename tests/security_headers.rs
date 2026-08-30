//! Tests for the headers the engine sets on its own behalf.
//!
//! Two rules decide what goes where, and both are easier to break than to
//! notice: a header the response already set is never replaced, and the
//! Content-Security-Policy is only applied to pages the engine wrote. The
//! second is the one with teeth — `script-src 'self'` on every script-served
//! response would break any solution with an inline `<script>`, which is most
//! of them.

mod common;

use aiwebengine::security::engine_page_policy;

/// An engine page's policy names the specific inline blocks the engine wrote.
/// `'unsafe-inline'` would name those *and* anything injected beside them,
/// which is the whole thing a policy is for.
#[test]
fn an_engine_page_policy_admits_its_nonce_and_not_inline_generally() {
    let policy = engine_page_policy("test-nonce");

    assert!(policy.contains("script-src 'self' 'nonce-test-nonce'"));
    assert!(policy.contains("style-src 'self' 'nonce-test-nonce'"));
    assert!(
        !policy.contains("unsafe-inline"),
        "a nonce exists so that unsafe-inline does not have to"
    );
    assert!(!policy.contains("unsafe-eval"));
}

/// A sign-in form inside someone else's iframe is a clickjacking target, and a
/// form that can submit anywhere is a credential-harvesting one.
#[test]
fn an_engine_page_cannot_be_framed_or_retargeted() {
    let policy = engine_page_policy("n");
    assert!(policy.contains("frame-ancestors 'none'"));
    assert!(policy.contains("form-action 'self'"));
    assert!(policy.contains("base-uri 'none'"));
    assert!(policy.contains("object-src 'none'"));
}

#[test]
fn each_response_gets_its_own_nonce() {
    let first = aiwebengine::security::generate_nonce();
    let second = aiwebengine::security::generate_nonce();
    assert_ne!(
        first, second,
        "a nonce a caller can predict from a previous response is not a nonce"
    );
    assert!(first.len() >= 16, "and it has to be long enough to matter");
}
