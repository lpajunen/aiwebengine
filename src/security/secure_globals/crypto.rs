//! `crypto`: the cryptography a solution should not write for itself.

use super::*;
use crate::security::Capability;
use rquickjs::{Function, Result as JsResult};
use tracing::debug;

pub(super) const CRYPTO_PRELUDE: &str = include_str!("../../../assets/crypto_prelude.js");

/// What `crypto.hmacVerify` is handed, as JavaScript writes it.
///
/// `secretName` and not `secret`. The value never enters the runtime — that is
/// the whole of what this API is for — and a field called `secret` invites
/// somebody to pass one, which would work, and would silently give up the
/// property they came here for.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct HmacVerifyOptions {
    pub(super) secret_name: String,
    pub(super) message: String,
    pub(super) signature: String,
    #[serde(default)]
    pub(super) algorithm: Option<String>,
    #[serde(default)]
    pub(super) encoding: Option<String>,
}

impl SecureGlobalContext {
    /// `crypto` — the cryptography a solution should not be writing for itself.
    ///
    /// The engine tells scripts to verify their own webhook signatures and
    /// until now handed them nothing to do it with, so every solution that
    /// followed the documentation fetched its secret into JavaScript and
    /// compared it with `===`. See [`crate::security::script_crypto`] for why
    /// each of these exists; what happens *here* is the part that makes them
    /// worth having — the secret is resolved host-side and never crosses into
    /// the runtime.
    ///
    /// Two halves, gated differently, because they are different grants:
    ///
    /// - `randomUUID`, `randomToken` and `constantTimeEqual` take no
    ///   capability. Randomness is not authority and a comparison of two
    ///   strings the caller already holds reveals nothing it did not have.
    ///   Model-authored code inside `sandbox.run` may use them, and should:
    ///   the alternative is that it writes the comparison itself.
    ///
    /// - `secretEquals` and `hmacVerify` resolve a secret, so both require
    ///   [`Capability::ReadSecrets`] — the same gate `fetch` puts on
    ///   `{{secret:...}}` and for the same reason. A narrowed execution that
    ///   may not reach the account's credentials must not reach them through a
    ///   comparison either, and `run_js` withholding `read_secrets` is what
    ///   stops model-authored code turning this into an oracle.
    pub(super) fn setup_crypto_object(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
    ) -> JsResult<()> {
        use crate::security::script_crypto as crypto;

        let global = ctx.globals();
        let host = rquickjs::Object::new(ctx.clone())?;

        // Whose secrets this execution may resolve, decided exactly as
        // `setup_fetch_function` decides it: withheld from a delegated run
        // that was not granted `Scope::Secrets`, so the lookup falls back to
        // the script's own key rather than erroring. A webhook secret belongs
        // to the solution rather than to a person, so that fallback is the
        // normal path here rather than the degraded one.
        let user_id_for_secrets = if self
            .config
            .allows_delegated(crate::delegation::Scope::Secrets)
        {
            self.user_context.user_id.clone()
        } else {
            None
        };

        let random_uuid = Function::new(ctx.clone(), || -> String {
            uuid::Uuid::new_v4().to_string()
        })?;

        let random_token = Function::new(
            ctx.clone(),
            move |bytes: i64, encoding: String| -> JsResult<String> {
                let Some(encoding) = crypto::Encoding::parse(&encoding) else {
                    return Err(unknown_name_error(
                        "crypto.randomToken",
                        "encoding",
                        &encoding,
                        &["hex", "base64"],
                    ));
                };
                // Negative reaches here as a negative: refused as too short,
                // which is the same answer as zero and names the same fix.
                let asked = usize::try_from(bytes).unwrap_or(0);
                crypto::random_token(asked, encoding).map_err(|refusal| {
                    rquickjs::Error::new_from_js_message(
                        "crypto.randomToken",
                        "range_error",
                        &format!("crypto.randomToken: {}", refusal),
                    )
                })
            },
        )?;

        let constant_time_equal = Function::new(ctx.clone(), |a: String, b: String| -> bool {
            crypto::constant_time_eq(a.as_bytes(), b.as_bytes())
        })?;

        let user_ctx_secret_equals = self.user_context.clone();
        let uri_secret_equals = script_uri.to_string();
        let user_id_secret_equals = user_id_for_secrets.clone();
        let secret_equals = Function::new(
            ctx.clone(),
            move |secret_name: String, candidate: String| -> JsResult<bool> {
                if !user_ctx_secret_equals.has_capability(&Capability::ReadSecrets) {
                    return Err(capability_error(
                        "crypto.secretEquals",
                        &Capability::ReadSecrets,
                        &user_ctx_secret_equals,
                    ));
                }

                let secret = resolve_named_secret(
                    "crypto.secretEquals",
                    &uri_secret_equals,
                    &secret_name,
                    user_id_secret_equals.as_deref(),
                )?;

                Ok(crypto::constant_time_eq(
                    secret.as_bytes(),
                    candidate.as_bytes(),
                ))
            },
        )?;

        let user_ctx_hmac = self.user_context.clone();
        let uri_hmac = script_uri.to_string();
        let user_id_hmac = user_id_for_secrets;
        let hmac_verify =
            Function::new(ctx.clone(), move |options_json: String| -> JsResult<bool> {
                if !user_ctx_hmac.has_capability(&Capability::ReadSecrets) {
                    return Err(capability_error(
                        "crypto.hmacVerify",
                        &Capability::ReadSecrets,
                        &user_ctx_hmac,
                    ));
                }

                let options: HmacVerifyOptions =
                    serde_json::from_str(&options_json).map_err(|e| {
                        rquickjs::Error::new_from_js_message(
                            "crypto.hmacVerify",
                            "type_error",
                            &format!("crypto.hmacVerify: {}", e),
                        )
                    })?;

                let algorithm_name = options.algorithm.as_deref().unwrap_or("sha256");
                let Some(algorithm) = crypto::Digest::parse(algorithm_name) else {
                    return Err(unknown_name_error(
                        "crypto.hmacVerify",
                        "algorithm",
                        algorithm_name,
                        &["sha256", "sha512", "sha1"],
                    ));
                };

                let encoding_name = options.encoding.as_deref().unwrap_or("hex");
                let Some(encoding) = crypto::Encoding::parse(encoding_name) else {
                    return Err(unknown_name_error(
                        "crypto.hmacVerify",
                        "encoding",
                        encoding_name,
                        &["hex", "base64"],
                    ));
                };

                let secret = resolve_named_secret(
                    "crypto.hmacVerify",
                    &uri_hmac,
                    &options.secret_name,
                    user_id_hmac.as_deref(),
                )?;

                Ok(crypto::verify_hmac(
                    algorithm,
                    secret.as_bytes(),
                    options.message.as_bytes(),
                    &options.signature,
                    encoding,
                ))
            })?;

        host.set("randomUUID", random_uuid)?;
        host.set("randomToken", random_token)?;
        host.set("constantTimeEqual", constant_time_equal)?;
        host.set("secretEquals", secret_equals)?;
        host.set("hmacVerify", hmac_verify)?;
        global.set("__hostCrypto", host)?;

        crate::bytecode::eval_program(ctx, "engine://crypto-prelude", CRYPTO_PRELUDE).map_err(
            |e| {
                rquickjs::Error::new_from_js_message(
                    "crypto",
                    "prelude",
                    &format!("crypto prelude failed to load: {}", e),
                )
            },
        )?;

        debug!("crypto initialized for script: {}", script_uri);
        Ok(())
    }
}
