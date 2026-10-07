//! `scriptStorage` and `personalStorage`.

use super::*;
use crate::security::Capability;
use rquickjs::{Function, Result as JsResult};
use tracing::debug;

/// Builds the Web Storage interface — `length`, `key(i)`, named access, and
/// failures that throw rather than being returned — over the two host stores.
pub(super) const STORAGE_PRELUDE: &str = include_str!("../../../assets/storage_prelude.js");

impl SecureGlobalContext {
    /// The user the current invocation is running as, if any.
    ///
    /// Personal storage is keyed by it, and every one of its methods needs the
    /// same answer, so the walk down `context.request.auth` lives here rather
    /// than four times over. `None` means there is nobody to store anything
    /// for — which the prelude turns into a `SecurityError`, as a browser does
    /// when storage is not available to the caller.
    /// The person whose storage this execution may reach, if any.
    ///
    /// [`Self::current_user_id`] answers who the execution is running as;
    /// this answers whether it may act on that in a store belonging to them.
    /// The two differ only for a delegated task, which knows who it acts for
    /// and may still not have been authorised to touch their data.
    ///
    /// Refusing by answering `None` rather than by a distinct error is
    /// deliberate: it is the same answer a background task with nobody signed
    /// in already produces, so the failure a script sees is one it already
    /// had to handle.
    pub(super) fn delegated_user_id(ctx: &rquickjs::Ctx<'_>, allowed: bool) -> Option<String> {
        if !allowed {
            return None;
        }
        Self::current_user_id(ctx)
    }
}

impl SecureGlobalContext {
    pub(super) fn current_user_id(ctx: &rquickjs::Ctx<'_>) -> Option<String> {
        let context_obj: rquickjs::Object = ctx.globals().get("context").ok()?;
        let request_obj: rquickjs::Object = context_obj.get("request").ok()?;
        let auth_obj: rquickjs::Object = request_obj.get("auth").ok()?;

        let is_authenticated: bool = auth_obj.get("isAuthenticated").unwrap_or_default();
        if !is_authenticated {
            return None;
        }

        match auth_obj.get("userId") {
            Ok(Some(user_id)) => Some(user_id),
            _ => None,
        }
    }
}

impl SecureGlobalContext {
    /// The error envelope the storage prelude turns into a `DOMException`.
    ///
    /// `name` is the exception's, so the failure a script catches says which
    /// kind it was rather than being prose it would have to match on.
    pub(super) fn storage_failure(name: &str, message: &str) -> String {
        serde_json::json!({ "name": name, "message": message }).to_string()
    }
}

impl SecureGlobalContext {
    /// Classifies a write that the repository refused.
    ///
    /// Size is the one a script can do something about, and the one the Web
    /// Storage spec names, so it keeps its own exception; anything else is the
    /// store being unable to answer.
    pub(super) fn storage_write_failure(error: &crate::error::AppError) -> String {
        let message = error.to_string();
        if message.contains("too large") {
            Self::storage_failure("QuotaExceededError", &message)
        } else {
            Self::storage_failure("UnknownError", &message)
        }
    }
}

impl SecureGlobalContext {
    /// Setup secure script storage functions
    pub(super) fn setup_script_properties_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
    ) -> JsResult<()> {
        let global = ctx.globals();
        let script_uri_owned = script_uri.to_string();

        // What this execution may do with the store. An unnarrowed one holds
        // both, which is every context that existed before attenuation did.
        //
        // Reads are withheld by answering `available()` with false, which is
        // the prelude's existing "there is no store for you" path: a
        // deliberate `getItem` throws `SecurityError` and a property probe
        // stays quiet, which is the line the prelude already draws for a
        // personal store with nobody signed in. Writes are withheld by
        // answering the envelope the prelude throws, so a refused write fails
        // where it was written rather than silently doing nothing.
        let may_read_storage = self.user_context.has_capability(&Capability::ReadStorage);
        let may_write_storage = self.user_context.has_capability(&Capability::WriteStorage);
        let read_denial = capability_refusal(
            "scriptStorage",
            &Capability::ReadStorage,
            &self.user_context,
        );
        let write_denial = Self::storage_failure(
            "SecurityError",
            &capability_refusal(
                "scriptStorage",
                &Capability::WriteStorage,
                &self.user_context,
            ),
        );
        let write_denial_set = write_denial.clone();
        let write_denial_remove = write_denial.clone();
        let write_denial_clear = write_denial;

        // The Rust half of `scriptStorage`. Every method here answers with a
        // value rather than with prose about one: `null` where the browser's
        // `Storage` answers `null`, and — on the write paths — either nothing
        // or the envelope of the exception the prelude should throw. Building
        // the browser's interface on top of that is `storage_prelude.js`'s job.
        let host = rquickjs::Object::new(ctx.clone())?;

        let script_uri_get = script_uri_owned.clone();
        let get_item = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, key: String| -> JsResult<Option<String>> {
                debug!(
                    "scriptStorage.getItem called for script {} with key: {}",
                    script_uri_get, key
                );
                Ok(crate::repository::get_script_properties_item(
                    &script_uri_get,
                    &key,
                ))
            },
        )?;
        host.set("getItem", get_item)?;

        let script_uri_set = script_uri_owned.clone();
        let set_item = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  key: String,
                  value: String|
                  -> JsResult<Option<String>> {
                debug!(
                    "scriptStorage.setItem called for script {} with key: {}",
                    script_uri_set, key
                );

                if !may_write_storage {
                    return Ok(Some(write_denial_set.clone()));
                }

                if key.trim().is_empty() {
                    return Ok(Some(Self::storage_failure(
                        "SyntaxError",
                        "Key cannot be empty",
                    )));
                }

                match crate::repository::set_script_properties_item(&script_uri_set, &key, &value) {
                    Ok(()) => Ok(None),
                    Err(e) => Ok(Some(Self::storage_write_failure(&e))),
                }
            },
        )?;
        host.set("setItem", set_item)?;

        let script_uri_remove = script_uri_owned.clone();
        let remove_item = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, key: String| -> JsResult<Option<String>> {
                debug!(
                    "scriptStorage.removeItem called for script {} with key: {}",
                    script_uri_remove, key
                );
                if !may_write_storage {
                    return Ok(Some(write_denial_remove.clone()));
                }
                // Whether the key was there is not something `removeItem`
                // reports, in the browser or here.
                crate::repository::remove_script_properties_item(&script_uri_remove, &key);
                Ok(None)
            },
        )?;
        host.set("removeItem", remove_item)?;

        let script_uri_clear = script_uri_owned.clone();
        let clear_storage = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<Option<String>> {
                debug!("scriptStorage.clear called for script {}", script_uri_clear);
                if !may_write_storage {
                    return Ok(Some(write_denial_clear.clone()));
                }
                match crate::repository::clear_script_properties(&script_uri_clear) {
                    Ok(()) => Ok(None),
                    Err(e) => Ok(Some(Self::storage_write_failure(&e))),
                }
            },
        )?;
        host.set("clear", clear_storage)?;

        let script_uri_keys = script_uri_owned.clone();
        let keys = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<Vec<String>> {
                if !may_read_storage {
                    return Ok(Vec::new());
                }
                Ok(crate::repository::list_script_properties_keys(
                    &script_uri_keys,
                ))
            },
        )?;
        host.set("keys", keys)?;

        // Available whenever this execution may read it: script storage
        // belongs to the script rather than to a user, so there is nobody who
        // could be missing, and the only thing left to be missing is the
        // capability.
        let available = Function::new(ctx.clone(), move |_ctx: rquickjs::Ctx<'_>| -> bool {
            may_read_storage
        })?;
        host.set("available", available)?;

        // Why it is unavailable, when the reason is a narrowing rather than a
        // missing person. The prelude prefers this to its own wording, so a
        // script hears "does not hold 'read_storage'" instead of being told to
        // log in when logging in would not help.
        let denial = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> Option<String> {
                (!may_read_storage).then(|| read_denial.clone())
            },
        )?;
        host.set("denial", denial)?;

        global.set("__hostScriptStorage", host)?;

        debug!(
            "scriptStorage host functions initialized for script: {}",
            script_uri
        );

        Ok(())
    }
}

impl SecureGlobalContext {
    pub(super) fn setup_user_properties_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
    ) -> JsResult<()> {
        let global = ctx.globals();
        let script_uri_owned = script_uri.to_string();

        // Whether this execution may reach the person's storage at all.
        //
        // True for everything that is not delegated — an ordinary request *is*
        // the person, so there is no grant to hold it to. For a delegated task
        // it is exactly what they ticked, and a grant that did not name this
        // reads as "nobody is signed in": the same `SecurityError` a background
        // task with no person at all already gets, rather than a new failure
        // mode for a script to learn.
        // Two narrowings again, as in `setup_secrets_functions`: what the
        // absent person authorised, and what this execution holds. A
        // delegated *and* attenuated task is subject to each.
        let storage_allowed = self
            .config
            .allows_delegated(crate::delegation::Scope::PersonalStorage)
            && self.user_context.has_capability(&Capability::ReadStorage);
        let may_write_personal = self.user_context.has_capability(&Capability::WriteStorage);
        let read_denial = capability_refusal(
            "personalStorage",
            &Capability::ReadStorage,
            &self.user_context,
        );
        let write_denial = Self::storage_failure(
            "SecurityError",
            &capability_refusal(
                "personalStorage",
                &Capability::WriteStorage,
                &self.user_context,
            ),
        );
        let write_denial_set = write_denial.clone();
        let write_denial_remove = write_denial.clone();
        let write_denial_clear = write_denial;
        let may_read_personal = self.user_context.has_capability(&Capability::ReadStorage);

        // The Rust half of `personalStorage`. It differs from script storage in
        // one way that matters: without an authenticated user there is no store
        // to read or write, and saying so is not the same as saying the key was
        // missing. `available()` is what lets the prelude tell those apart and
        // raise `SecurityError` instead of quietly answering `null`.
        let host = rquickjs::Object::new(ctx.clone())?;

        let script_uri_get = script_uri_owned.clone();
        let get_item = Function::new(
            ctx.clone(),
            move |ctx: rquickjs::Ctx<'_>, key: String| -> JsResult<Option<String>> {
                debug!(
                    "personalStorage.getItem called for script {} with key: {}",
                    script_uri_get, key
                );
                let Some(user_id) = Self::delegated_user_id(&ctx, storage_allowed) else {
                    return Ok(None);
                };
                Ok(crate::repository::get_user_properties_item(
                    &script_uri_get,
                    &user_id,
                    &key,
                ))
            },
        )?;
        host.set("getItem", get_item)?;

        let script_uri_set = script_uri_owned.clone();
        let set_item = Function::new(
            ctx.clone(),
            move |ctx: rquickjs::Ctx<'_>, key: String, value: String| -> JsResult<Option<String>> {
                debug!(
                    "personalStorage.setItem called for script {} with key: {}",
                    script_uri_set, key
                );

                if !may_write_personal {
                    return Ok(Some(write_denial_set.clone()));
                }

                let Some(user_id) = Self::delegated_user_id(&ctx, storage_allowed) else {
                    return Ok(Some(Self::storage_failure(
                        "SecurityError",
                        "Personal storage requires an authenticated user",
                    )));
                };

                if key.trim().is_empty() {
                    return Ok(Some(Self::storage_failure(
                        "SyntaxError",
                        "Key cannot be empty",
                    )));
                }

                match crate::repository::set_user_properties_item(
                    &script_uri_set,
                    &user_id,
                    &key,
                    &value,
                ) {
                    Ok(()) => Ok(None),
                    Err(e) => Ok(Some(Self::storage_write_failure(&e))),
                }
            },
        )?;
        host.set("setItem", set_item)?;

        let script_uri_remove = script_uri_owned.clone();
        let remove_item = Function::new(
            ctx.clone(),
            move |ctx: rquickjs::Ctx<'_>, key: String| -> JsResult<Option<String>> {
                debug!(
                    "personalStorage.removeItem called for script {} with key: {}",
                    script_uri_remove, key
                );
                if !may_write_personal {
                    return Ok(Some(write_denial_remove.clone()));
                }
                let Some(user_id) = Self::delegated_user_id(&ctx, storage_allowed) else {
                    return Ok(Some(Self::storage_failure(
                        "SecurityError",
                        "Personal storage requires an authenticated user",
                    )));
                };
                crate::repository::remove_user_properties_item(&script_uri_remove, &user_id, &key);
                Ok(None)
            },
        )?;
        host.set("removeItem", remove_item)?;

        let script_uri_clear = script_uri_owned.clone();
        let clear_storage = Function::new(
            ctx.clone(),
            move |ctx: rquickjs::Ctx<'_>| -> JsResult<Option<String>> {
                debug!(
                    "personalStorage.clear called for script {}",
                    script_uri_clear
                );
                if !may_write_personal {
                    return Ok(Some(write_denial_clear.clone()));
                }
                let Some(user_id) = Self::delegated_user_id(&ctx, storage_allowed) else {
                    return Ok(Some(Self::storage_failure(
                        "SecurityError",
                        "Personal storage requires an authenticated user",
                    )));
                };
                match crate::repository::clear_user_properties(&script_uri_clear, &user_id) {
                    Ok(()) => Ok(None),
                    Err(e) => Ok(Some(Self::storage_write_failure(&e))),
                }
            },
        )?;
        host.set("clear", clear_storage)?;

        let script_uri_keys = script_uri_owned.clone();
        let keys = Function::new(
            ctx.clone(),
            move |ctx: rquickjs::Ctx<'_>| -> JsResult<Vec<String>> {
                let Some(user_id) = Self::delegated_user_id(&ctx, storage_allowed) else {
                    return Ok(Vec::new());
                };
                Ok(crate::repository::list_user_properties_keys(
                    &script_uri_keys,
                    &user_id,
                ))
            },
        )?;
        host.set("keys", keys)?;

        let available = Function::new(ctx.clone(), move |ctx: rquickjs::Ctx<'_>| -> bool {
            Self::delegated_user_id(&ctx, storage_allowed).is_some()
        })?;
        host.set("available", available)?;

        // Only a narrowing is reported here. A store that is unavailable
        // because nobody is signed in keeps the prelude's own wording, which
        // is the accurate one for that case and the one scripts already
        // match on.
        let denial = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> Option<String> {
                (!may_read_personal).then(|| read_denial.clone())
            },
        )?;
        host.set("denial", denial)?;

        global.set("__hostPersonalStorage", host)?;

        // Both stores are wrapped by one prelude, installed after the second of
        // them so it finds each host object in place. Compiled once per process
        // and cached, like the other preludes.
        crate::bytecode::eval_program(ctx, "engine://storage-prelude", STORAGE_PRELUDE).map_err(
            |e| {
                rquickjs::Error::new_from_js_message(
                    "storage",
                    "prelude",
                    &format!("storage prelude failed to load: {}", e),
                )
            },
        )?;

        debug!(
            "scriptStorage and personalStorage initialized for script: {}",
            script_uri
        );

        Ok(())
    }
}
