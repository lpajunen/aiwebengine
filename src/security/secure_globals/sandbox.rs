//! `sandbox.run`: running model-authored code with fewer capabilities.

use super::*;
use crate::security::Capability;
use rquickjs::{Function, Result as JsResult};
use tracing::debug;

/// Builds `sandbox` over the host call that runs a narrowed sub-execution.
pub(super) const SANDBOX_PRELUDE: &str = include_str!("../../../assets/sandbox_prelude.js");

impl SecureGlobalContext {
    /// The Rust half of `sandbox` — [`crate::sandbox`].
    ///
    /// One host call that runs a whole second execution of this script, in a
    /// context holding a chosen subset of what this one holds. What makes
    /// that affordable is that nothing here is new: `evaluate_snippet` already
    /// evaluates caller-authored source against a script's program with a
    /// caller-chosen `UserContext`, and a stream customization function
    /// already builds a nested runtime from inside a running host call. This is those
    /// two facts put together and pointed at the calling script itself.
    pub(super) fn setup_sandbox_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
    ) -> JsResult<()> {
        let global = ctx.globals();
        let host = rquickjs::Object::new(ctx.clone())?;

        let script_uri_run = script_uri.to_string();
        let user_run = self.user_context.clone();
        let config_run = self.config.clone();
        let run = Function::new(
            ctx.clone(),
            move |ctx: rquickjs::Ctx<'_>, options_json: String| -> JsResult<String> {
                if config_run.is_dry_run() {
                    // A sub-execution runs real code against live data, and
                    // only its database writes are undone. A check that
                    // deploys nothing must not set that in motion.
                    return Ok(Self::sandbox_failure(
                        "DryRunError",
                        "sandbox.run: nothing was run - this is a dry run",
                    ));
                }

                let options: serde_json::Value = match serde_json::from_str(&options_json) {
                    Ok(options) => options,
                    Err(e) => {
                        return Ok(Self::sandbox_failure(
                            "TypeError",
                            &format!("sandbox.run: options are not valid JSON: {}", e),
                        ));
                    }
                };

                let source = options
                    .get("source")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();
                if source.trim().is_empty() {
                    return Ok(Self::sandbox_failure(
                        "TypeError",
                        "sandbox.run: there is no source to run",
                    ));
                }

                let requested: Vec<String> = match options.get("capabilities") {
                    Some(serde_json::Value::Array(names)) => {
                        let mut parsed = Vec::with_capacity(names.len());
                        for name in names {
                            match name.as_str() {
                                Some(name) => parsed.push(name.to_string()),
                                None => {
                                    return Ok(Self::sandbox_failure(
                                        "TypeError",
                                        "sandbox.run: every capability must be named by a string",
                                    ));
                                }
                            }
                        }
                        parsed
                    }
                    // An omitted list is the empty one, not "everything I
                    // hold". Defaulting the other way would make a typo in
                    // the option name — `capability`, `caps` — silently hand
                    // model-authored code the whole of the caller's
                    // authority, which is the one mistake this API exists to
                    // make impossible.
                    None | Some(serde_json::Value::Null) => Vec::new(),
                    Some(_) => {
                        return Ok(Self::sandbox_failure(
                            "TypeError",
                            "sandbox.run: capabilities must be an array of names",
                        ));
                    }
                };

                // `hosts` is the destination half, and it defaults the other
                // way round from `capabilities`: omitted means "whatever the
                // caller could already reach", not "nothing". They differ
                // because they answer different questions — `capabilities`
                // says what this sub-execution may do, and an omitted list
                // safely means none of it, while `hosts` only ever *removes*
                // destinations from a set that is already the caller's.
                // Defaulting it to the empty list would silently take the
                // network away from every existing `sandbox.run` call that
                // asked for `use_network`.
                let hosts: Option<Vec<String>> = match options.get("hosts") {
                    Some(serde_json::Value::Array(names)) => {
                        let mut parsed = Vec::with_capacity(names.len());
                        for name in names {
                            match name.as_str() {
                                Some(name) => parsed.push(name.to_string()),
                                None => {
                                    return Ok(Self::sandbox_failure(
                                        "TypeError",
                                        "sandbox.run: every host must be named by a string",
                                    ));
                                }
                            }
                        }
                        Some(parsed)
                    }
                    None | Some(serde_json::Value::Null) => None,
                    Some(_) => {
                        return Ok(Self::sandbox_failure(
                            "TypeError",
                            "sandbox.run: hosts must be an array of host patterns",
                        ));
                    }
                };

                let narrowed = match crate::sandbox::narrow_to(&user_run, &requested, hosts) {
                    Ok(narrowed) => narrowed,
                    Err(refusal) => {
                        let name = match refusal {
                            crate::sandbox::Refusal::UnknownCapability(_) => "TypeError",
                            _ => "SecurityError",
                        };
                        return Ok(Self::sandbox_failure(
                            name,
                            &format!("sandbox.run: {}", refusal),
                        ));
                    }
                };

                // Held for the whole sub-execution: the budget stops a
                // runaway one, and this stops a chain of them from exhausting
                // the native stack before the budget notices.
                let _depth = match crate::sandbox::DepthGuard::enter() {
                    Ok(guard) => guard,
                    Err(depth) => {
                        return Ok(Self::sandbox_failure(
                            "RangeError",
                            &format!("sandbox.run: {}", crate::sandbox::Refusal::TooDeep(depth)),
                        ));
                    }
                };

                let report = crate::script_eval::eval_blocking(crate::script_eval::EvalRequest {
                    timeout_ms: options.get("timeoutMs").and_then(|value| value.as_u64()),
                    // Off by default here, on by default at `/engine/eval_script`.
                    // There a caller is inspecting a deployment and should
                    // leave no trace; here a turn that may write is being run
                    // because its writes are wanted, and one that may not is
                    // already stopped by holding no write capability.
                    rollback: options
                        .get("rollback")
                        .and_then(|value| value.as_bool())
                        .unwrap_or(false),
                    input: options.get("input").cloned(),
                    // The person carries through. Attenuation says what may be
                    // done, not who is doing it, so `personalStorage` and
                    // `{{secret:...}}` go on resolving against the same
                    // account — a narrower part of their data rather than
                    // nobody's.
                    auth_context: Self::sandbox_auth_context(&ctx),
                    // The files the caller is running, not head: a pinned
                    // script's sub-execution must be built from the revision
                    // it serves.
                    view: crate::deployments::serving_view(&script_uri_run),
                    ..crate::script_eval::EvalRequest::new(script_uri_run.clone(), source, narrowed)
                });

                Ok(Self::sandbox_ok(serde_json::json!({
                    "value": report.outcome.value,
                    "valueType": report.outcome.value_type,
                    "console": report.outcome.console,
                    "consoleDropped": report.outcome.console_dropped,
                    "durationMs": report.outcome.duration_ms,
                    "rolledBack": report.outcome.rolled_back,
                    "error": report.outcome.error,
                    "ok": report.ok,
                })))
            },
        )?;
        host.set("run", run)?;

        // Every name there is, and what this execution holds of them. A
        // script builds its narrowed set by subtracting from the second
        // rather than by writing out a list that drifts as the vocabulary
        // grows.
        let all = Function::new(ctx.clone(), move |_ctx: rquickjs::Ctx<'_>| -> String {
            serde_json::json!(
                Capability::all()
                    .iter()
                    .map(|capability| capability.as_str())
                    .collect::<Vec<_>>()
            )
            .to_string()
        })?;
        host.set("capabilities", all)?;

        // What this execution may reach, so a script can narrow by subtracting
        // rather than by writing a list it hopes is a subset. `null` rather
        // than an empty array for "anywhere": an empty array is a real answer
        // here — a scope that permits nothing — and the two must not collide.
        let user_hosts = self.user_context.clone();
        let hosts = Function::new(ctx.clone(), move |_ctx: rquickjs::Ctx<'_>| -> String {
            match &user_hosts.network_scope {
                Some(scope) => serde_json::json!(scope.hosts().collect::<Vec<_>>()).to_string(),
                None => "null".to_string(),
            }
        })?;
        host.set("hosts", hosts)?;

        let user_held = self.user_context.clone();
        let held = Function::new(ctx.clone(), move |_ctx: rquickjs::Ctx<'_>| -> String {
            let mut names: Vec<&str> = user_held
                .capabilities
                .iter()
                .map(|capability| capability.as_str())
                .collect();
            // A `HashSet` has no order and this is read by scripts, so sort
            // it: a list that shuffles between calls is one nobody can
            // usefully compare or log.
            names.sort_unstable();
            serde_json::json!(names).to_string()
        })?;
        host.set("held", held)?;

        global.set("__hostSandbox", host)?;

        crate::bytecode::eval_program(ctx, "engine://sandbox-prelude", SANDBOX_PRELUDE).map_err(
            |e| {
                rquickjs::Error::new_from_js_message(
                    "sandbox",
                    "prelude",
                    &format!("sandbox prelude failed to load: {}", e),
                )
            },
        )?;

        debug!("sandbox initialized for script: {}", script_uri);
        Ok(())
    }
}

impl SecureGlobalContext {
    /// The person the sub-execution runs as, read from the calling context
    /// rather than from the [`UserContext`].
    ///
    /// Those two carry different things and both are needed:
    /// `UserContext.user_id` is who the engine authorizes, while
    /// `context.request.auth` is what JavaScript reads — and
    /// `personalStorage`, `secretStorage` and `personalTasks` all resolve
    /// against the second. A sub-execution that dropped it would lose the
    /// person as well as the capabilities.
    ///
    /// The role flags carry through unchanged, because attenuation does not
    /// change who somebody is. A narrowed turn belonging to an administrator
    /// still reads `isAdmin`, and is still refused at every gate it no longer
    /// holds — the refusal is the enforcement, not the flag.
    pub(super) fn sandbox_auth_context(
        ctx: &rquickjs::Ctx<'_>,
    ) -> Option<crate::auth::JsAuthContext> {
        let context: rquickjs::Object = ctx.globals().get("context").ok()?;
        let request: rquickjs::Object = context.get("request").ok()?;
        let auth: rquickjs::Object = request.get("auth").ok()?;

        Some(crate::auth::JsAuthContext {
            user_id: auth.get("userId").ok().flatten(),
            email: auth.get("email").ok().flatten(),
            name: auth.get("name").ok().flatten(),
            provider: auth.get("provider").ok().flatten(),
            is_authenticated: auth.get("isAuthenticated").unwrap_or_default(),
            is_admin: auth.get("isAdmin").unwrap_or_default(),
            is_editor: auth.get("isEditor").unwrap_or_default(),
            // Never an elevation. JavaScript is not told about one, so there
            // is nothing here to read back — and a sub-execution is the last
            // place that should reconstruct authority out of what the running
            // code could see. `sandbox.run` narrows; it has never widened, and
            // this is where widening would have had to come from.
            elevation: None,
        })
    }
}

impl SecureGlobalContext {
    /// The envelope shape `sandbox_prelude.js` unwraps. Flatter than the
    /// task one because a sandbox result is already an object with its own
    /// `error` field, and nesting a second `error` beside it would be two
    /// different failures spelled the same way.
    pub(super) fn sandbox_ok(result: serde_json::Value) -> String {
        serde_json::json!({ "ok": true, "result": result }).to_string()
    }
}

impl SecureGlobalContext {
    pub(super) fn sandbox_failure(name: &str, message: &str) -> String {
        serde_json::json!({ "ok": false, "name": name, "message": message }).to_string()
    }
}
