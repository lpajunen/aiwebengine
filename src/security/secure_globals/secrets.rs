//! `secretStorage`: write-only from JavaScript, resolved host-side.

use super::*;
use crate::security::Capability;
use rquickjs::{Function, Result as JsResult};
use tracing::debug;

/// Builds `secretStorage` over `__hostSecrets`.
pub(super) const SECRETS_PRELUDE: &str = include_str!("../../../assets/secrets_prelude.js");

/// What `secretStorage`'s mutating methods throw in a delegated execution.
pub(super) const DELEGATED_SECRET_MANAGEMENT_REFUSAL: &str = "Secrets cannot be changed by background work acting on somebody's behalf. \
     Storing, replacing or deleting a key is something the person does in their own session.";

/// The value behind a secret's name, or a refusal naming the secret.
///
/// Missing throws rather than answering `false`. A verification that quietly
/// fails because nobody stored the key looks exactly like a verification that
/// failed because the request was forged — so a deployment that was never
/// finished would present as an endpoint under permanent attack, and the log
/// would agree. The `{{secret:...}}` rule, in the place it matters most: an
/// unresolvable secret is an error, because behaving as though it resolved is
/// worse than stopping.
pub(super) fn resolve_named_secret(
    api: &str,
    script_uri: &str,
    name: &str,
    user_id: Option<&str>,
) -> JsResult<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(rquickjs::Error::new_from_js_message(
            "crypto",
            "type_error",
            &format!("{}: a secret must be named", api),
        ));
    }

    crate::repository::resolve_secret_db(script_uri, name, user_id).ok_or_else(|| {
        rquickjs::Error::new_from_js_message(
            "crypto",
            "secret_not_found",
            &format!(
                "{}: this script has no secret named '{}' — store one with write_secret",
                api, name
            ),
        )
    })
}

impl SecureGlobalContext {
    /// Setup secret storage functions
    ///
    /// Exposes a JavaScript API for per-user secret management scoped to the current script.
    /// Installed as `__hostSecrets`, answering in the envelope
    /// `secrets_prelude.js` unwraps. Writes need a signed-in person and throw
    /// without one. Secrets are stored in the user_secrets table keyed by
    /// (script_uri, user_id, key).
    ///
    /// - secretStorage.exists(key): boolean
    /// - secretStorage.setSecret(key, value): undefined
    /// - secretStorage.removeSecret(key): boolean
    /// - secretStorage.clear(): undefined
    pub(super) fn setup_secrets_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
    ) -> JsResult<()> {
        let global = ctx.globals();
        let script_uri_owned = script_uri.to_string();

        // Whether this execution may see that the person has a secret stored.
        //
        // The value itself is never returned to JavaScript by any of this, so
        // what the `secrets` scope really gates is `fetch`'s substitution. This
        // is the smaller half of the same question: whether a script acting for
        // somebody who did not grant it may learn which keys they hold.
        //
        // Two separate narrowings meet here and both have to hold: what the
        // absent person authorised, and what this execution holds. They answer
        // different questions — "may this script act for them at all" and "was
        // this turn given the credentials" — and an execution that is both
        // delegated and attenuated is subject to each.
        let secrets_allowed = self
            .config
            .allows_delegated(crate::delegation::Scope::Secrets)
            && self.user_context.has_capability(&Capability::ReadSecrets);

        // Managing a credential is refused outright in a delegated execution,
        // whatever was granted.
        //
        // The consent page offers "use the API keys you have given this app",
        // and storing, replacing or deleting one is not using it. Background
        // work that could rotate or delete somebody's key while they are away
        // would be doing something nobody was asked about — so this is a
        // decision about the surface rather than a scope, and there is no
        // checkbox that turns it on.
        let may_manage_secrets = !self.config.is_delegated()
            && self.user_context.has_capability(&Capability::WriteSecrets);

        // Why it was refused, when it is. The two reasons are different enough
        // that one message for both would mislead: a delegated execution is
        // refused whatever it asks for, an attenuated one is refused because
        // it asked to be.
        let manage_refusal = if self.config.is_delegated() {
            DELEGATED_SECRET_MANAGEMENT_REFUSAL.to_string()
        } else {
            capability_refusal(
                "secretStorage",
                &Capability::WriteSecrets,
                &self.user_context,
            )
        };
        let manage_refusal_set = manage_refusal.clone();
        let manage_refusal_remove = manage_refusal.clone();
        const SIGN_IN: &str = "secretStorage: a person's secrets need them signed in";

        let secret_storage_obj = rquickjs::Object::new(ctx.clone())?;

        // secretStorage.exists(key) - Check if secret exists in user_secrets or script_secrets
        let script_uri_exists = script_uri_owned.clone();
        let exists_fn = Function::new(
            ctx.clone(),
            move |ctx: rquickjs::Ctx<'_>, key: String| -> String {
                let globals = ctx.globals();
                // Check user_secrets first (if authenticated, and if this
                // execution was authorised to reach that person's secrets).
                let found = (secrets_allowed
                    && get_auth_user_id(&globals).is_some_and(|user_id| {
                        crate::repository::get_user_secret_item(&script_uri_exists, &user_id, &key)
                            .is_some()
                    }))
                    // Fall back to script_secrets
                    || crate::repository::get_script_secret_item(&script_uri_exists, &key)
                        .is_some();
                host_ok(serde_json::Value::Bool(found))
            },
        )?;
        secret_storage_obj.set("exists", exists_fn)?;

        // secretStorage.setSecret(key, value) - Store a secret for current user
        let script_uri_set = script_uri_owned.clone();
        let set_secret_fn = Function::new(
            ctx.clone(),
            move |ctx: rquickjs::Ctx<'_>, key: String, value: String| -> String {
                if !may_manage_secrets {
                    return host_failure("Error", &manage_refusal_set);
                }
                let Some(user_id) = get_auth_user_id(&ctx.globals()) else {
                    return host_failure("Error", SIGN_IN);
                };
                if key.trim().is_empty() {
                    return host_failure("TypeError", "secretStorage.setSecret: the key is empty");
                }
                if value.len() > 1_000_000 {
                    return host_failure(
                        "RangeError",
                        "secretStorage.setSecret: the value is over 1MB",
                    );
                }
                match crate::repository::set_user_secret_item(
                    &script_uri_set,
                    &user_id,
                    &key,
                    &value,
                ) {
                    Ok(()) => host_ok(serde_json::Value::Null),
                    Err(e) => host_failure("Error", &format!("secretStorage.setSecret: {}", e)),
                }
            },
        )?;
        secret_storage_obj.set("setSecret", set_secret_fn)?;

        // secretStorage.removeSecret(key) - Remove a single secret for current user
        let script_uri_remove = script_uri_owned.clone();
        let remove_secret_fn = Function::new(
            ctx.clone(),
            move |ctx: rquickjs::Ctx<'_>, key: String| -> String {
                // Refused rather than answering `false`: "you may not" read as
                // "there was nothing there" is the kind of answer a caller
                // cannot tell from the truth.
                if !may_manage_secrets {
                    return host_failure("Error", &manage_refusal_remove);
                }
                let Some(user_id) = get_auth_user_id(&ctx.globals()) else {
                    return host_failure("Error", SIGN_IN);
                };
                host_ok(serde_json::Value::Bool(
                    crate::repository::remove_user_secret_item(&script_uri_remove, &user_id, &key),
                ))
            },
        )?;
        secret_storage_obj.set("removeSecret", remove_secret_fn)?;

        // secretStorage.clear() - Clear all secrets for current user in this script
        let script_uri_clear = script_uri_owned.clone();
        let clear_fn = Function::new(ctx.clone(), move |ctx: rquickjs::Ctx<'_>| -> String {
            if !may_manage_secrets {
                return host_failure("Error", &manage_refusal);
            }
            let Some(user_id) = get_auth_user_id(&ctx.globals()) else {
                return host_failure("Error", SIGN_IN);
            };
            match crate::repository::clear_user_secrets(&script_uri_clear, &user_id) {
                Ok(()) => host_ok(serde_json::Value::Null),
                Err(e) => host_failure("Error", &format!("secretStorage.clear: {}", e)),
            }
        })?;
        secret_storage_obj.set("clear", clear_fn)?;

        global.set("__hostSecrets", secret_storage_obj)?;
        crate::bytecode::eval_program(ctx, "engine://secrets-prelude", SECRETS_PRELUDE).map_err(
            |e| {
                rquickjs::Error::new_from_js_message(
                    "secretStorage",
                    "prelude",
                    &format!("secrets prelude failed to load: {}", e),
                )
            },
        )?;

        debug!(
            "secretStorage JavaScript API initialized for script: {}",
            script_uri
        );

        Ok(())
    }
}

/// Extract the authenticated user_id from JavaScript `context.request.auth`.
/// Returns `None` if context is missing, request is missing, auth is missing,
/// or the user is not authenticated.
pub(super) fn get_auth_user_id(globals: &rquickjs::Object<'_>) -> Option<String> {
    let context_obj: rquickjs::Object = globals.get("context").ok()?;
    let request_obj: rquickjs::Object = context_obj.get("request").ok()?;
    let auth_obj: rquickjs::Object = request_obj.get("auth").ok()?;
    let is_authenticated: bool = auth_obj.get("isAuthenticated").unwrap_or_default();
    if !is_authenticated {
        return None;
    }
    auth_obj.get("userId").ok().flatten()
}
