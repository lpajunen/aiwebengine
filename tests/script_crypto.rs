//! The cryptography the engine does on a script's behalf.
//!
//! `src/security/script_crypto.rs` pins the arithmetic against the RFC
//! vectors. What these cover is the part that only exists once the binding is
//! installed: that the secret is resolved host-side and never crosses into the
//! runtime, that the capability gate is on the two calls that resolve one, and
//! that a missing secret is an error rather than a verification that quietly
//! fails forever.

mod common;

use common::{setup_env, test_mutex};

use aiwebengine::repository;
use aiwebengine::script_eval::{EvalReport, EvalRequest, eval_blocking};
use aiwebengine::security::UserContext;
use serde_json::{Value, json};

/// Run `source` as an administrator against a script that does nothing itself.
async fn run_as(uri: &str, source: &str, user: UserContext) -> EvalReport {
    repository::upsert_script(uri, "function init() {}").expect("script should be stored");
    let request = EvalRequest {
        timeout_ms: Some(15_000),
        rollback: false,
        ..EvalRequest::new(uri.to_string(), source.to_string(), user)
    };
    tokio::task::spawn_blocking(move || eval_blocking(request))
        .await
        .expect("evaluation panicked")
}

async fn value(uri: &str, source: &str) -> Value {
    let report = run_as(uri, source, UserContext::admin("crypto".to_string())).await;
    assert!(report.ok, "turn failed: {:?}", report.outcome.error);
    report.outcome.value.expect("the turn produced a value")
}

/// A webhook signature verifies under the key the sender used, and does not
/// under anything else. The whole of what the API is for, through the binding.
#[tokio::test(flavor = "multi_thread")]
async fn a_signature_verifies_through_the_named_secret() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://crypto/verify";
    repository::upsert_script(uri, "function init() {}").expect("script should be stored");
    repository::set_script_secret_item(uri, "WEBHOOK_KEY", "s3cret").expect("secret stored");

    // The signature a sender holding the same key would have produced.
    let signature = aiwebengine::security::script_crypto::hmac_encoded(
        aiwebengine::security::script_crypto::Digest::Sha256,
        b"s3cret",
        b"the body",
        aiwebengine::security::script_crypto::Encoding::Hex,
    );

    let out = value(
        uri,
        &format!(
            r#"
            ({{
                good: crypto.hmacVerify({{
                    secretName: "WEBHOOK_KEY",
                    message: "the body",
                    signature: "{signature}",
                }}),
                wrongBody: crypto.hmacVerify({{
                    secretName: "WEBHOOK_KEY",
                    message: "a different body",
                    signature: "{signature}",
                }}),
                garbage: crypto.hmacVerify({{
                    secretName: "WEBHOOK_KEY",
                    message: "the body",
                    signature: "not a signature",
                }}),
            }})
            "#
        ),
    )
    .await;

    assert_eq!(out["good"], json!(true));
    assert_eq!(out["wrongBody"], json!(false));
    assert_eq!(out["garbage"], json!(false));
}

/// The shared-secret header shape, and the property the whole API exists for:
/// the comparison happens without the value ever being in JavaScript. There is
/// no binding that would hand it over — `secretStorage` has no read — so what
/// this asserts is that the answer is right without one.
#[tokio::test(flavor = "multi_thread")]
async fn a_shared_secret_is_compared_without_being_handed_over() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://crypto/shared";
    repository::upsert_script(uri, "function init() {}").expect("script should be stored");
    repository::set_script_secret_item(uri, "TELEGRAM_WEBHOOK_SECRET", "abc123")
        .expect("secret stored");

    let out = value(
        uri,
        r#"
        ({
            right: crypto.secretEquals("TELEGRAM_WEBHOOK_SECRET", "abc123"),
            wrong: crypto.secretEquals("TELEGRAM_WEBHOOK_SECRET", "abc124"),
            shorter: crypto.secretEquals("TELEGRAM_WEBHOOK_SECRET", "abc"),
            empty: crypto.secretEquals("TELEGRAM_WEBHOOK_SECRET", ""),
            // There is no read, so this is the only way the value is reachable.
            noRead: typeof secretStorage.getSecret,
        })
        "#,
    )
    .await;

    assert_eq!(out["right"], json!(true));
    assert_eq!(out["wrong"], json!(false));
    assert_eq!(out["shorter"], json!(false));
    assert_eq!(out["empty"], json!(false));
    assert_eq!(
        out["noRead"],
        json!("undefined"),
        "the engine must still have no way to hand a secret to JavaScript"
    );
}

/// A secret nobody stored is an error, not a `false`.
///
/// Answering `false` would make an unfinished deployment look exactly like an
/// endpoint under permanent attack — every delivery refused, the log agreeing,
/// and nothing anywhere naming the missing key.
#[tokio::test(flavor = "multi_thread")]
async fn a_missing_secret_is_an_error_rather_than_a_refusal() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://crypto/missing",
        r#"
        try {
            crypto.secretEquals("NEVER_STORED", "anything");
            "not refused";
        } catch (e) {
            String((e && e.message) || e);
        }
        "#,
    )
    .await;

    let message = out.as_str().unwrap_or_default();
    assert!(
        message.contains("NEVER_STORED"),
        "the refusal should name the secret: {message}"
    );
    assert!(
        message.contains("write_secret"),
        "the refusal should say how to fix it: {message}"
    );
}

/// Resolving a secret takes `read_secrets`, so a narrowed execution cannot
/// turn a comparison into an oracle. Computing does not, so the same execution
/// can still generate a token and compare two strings it already holds.
#[tokio::test(flavor = "multi_thread")]
async fn narrowing_takes_the_secret_half_and_leaves_the_rest() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://crypto/narrowed";
    repository::upsert_script(uri, "function init() {}").expect("script should be stored");
    repository::set_script_secret_item(uri, "KEY", "value").expect("secret stored");

    let out = value(
        uri,
        r#"
        const run = sandbox.run(`
            const out = { compute: null, refused: null };
            out.compute = crypto.constantTimeEqual("a", "a") &&
                crypto.randomToken(16).length === 32;
            try {
                crypto.secretEquals("KEY", "value");
                out.refused = "not refused";
            } catch (e) {
                out.refused = String((e && e.message) || e);
            }
            out;
        `, { capabilities: ["read_assets"] });
        ({ value: run.value, error: run.error })
        "#,
    )
    .await;

    assert_eq!(out["error"], Value::Null, "the sub-execution should run");
    assert_eq!(
        out["value"]["compute"],
        json!(true),
        "randomness and comparison need no capability"
    );
    assert!(
        out["value"]["refused"]
            .as_str()
            .unwrap_or_default()
            .contains("read_secrets"),
        "resolving a secret should be refused by name: {}",
        out["value"]["refused"]
    );
}

/// A token is unguessable by construction, and asking for one that is not is
/// refused rather than quietly widened.
#[tokio::test(flavor = "multi_thread")]
async fn a_token_is_long_enough_or_it_is_refused() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let out = value(
        "test://crypto/token",
        r#"
        function refusal(fn) {
            try { fn(); return "not refused"; }
            catch (e) { return String((e && e.message) || e); }
        }
        ({
            uuid: crypto.randomUUID(),
            defaultLength: crypto.randomToken().length,
            base64: crypto.randomToken(32, "base64"),
            distinct: crypto.randomToken() !== crypto.randomToken(),
            tooShort: refusal(function () { return crypto.randomToken(4); }),
            unknownEncoding: refusal(function () { return crypto.randomToken(32, "rot13"); }),
        })
        "#,
    )
    .await;

    assert_eq!(
        out["uuid"].as_str().unwrap_or_default().len(),
        36,
        "randomUUID should be a v4 UUID"
    );
    assert_eq!(out["defaultLength"], json!(64), "32 bytes as hex");
    assert_eq!(out["distinct"], json!(true));
    assert!(
        out["base64"].as_str().unwrap_or_default().len() >= 40,
        "32 bytes as base64: {}",
        out["base64"]
    );
    assert!(
        out["tooShort"].as_str().unwrap_or_default().contains("16"),
        "a short token should name the floor: {}",
        out["tooShort"]
    );
    assert!(
        out["unknownEncoding"]
            .as_str()
            .unwrap_or_default()
            .contains("rot13"),
        "an unknown encoding should name what was asked for: {}",
        out["unknownEncoding"]
    );
}

/// An algorithm the engine does not implement is refused rather than defaulted
/// to one it does. Verifying with the wrong algorithm answers `false` for
/// every delivery, which reads as an attack rather than as a typo.
#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_algorithm_is_refused_rather_than_defaulted() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://crypto/algorithm";
    repository::upsert_script(uri, "function init() {}").expect("script should be stored");
    repository::set_script_secret_item(uri, "KEY", "value").expect("secret stored");

    let out = value(
        uri,
        r#"
        try {
            crypto.hmacVerify({
                secretName: "KEY",
                message: "m",
                signature: "00",
                algorithm: "md5",
            });
            "not refused";
        } catch (e) {
            String((e && e.message) || e);
        }
        "#,
    )
    .await;

    let message = out.as_str().unwrap_or_default();
    assert!(
        message.contains("md5"),
        "should name what was asked: {message}"
    );
    assert!(
        message.contains("sha256"),
        "should name what there is: {message}"
    );
}
