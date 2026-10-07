//! `engine`: the management tools, for a principal that came from a credential.

use super::*;
use rquickjs::{Function, Result as JsResult};
use tracing::debug;

pub(super) const ENGINE_PRELUDE: &str = include_str!("../../../assets/engine_prelude.js");

impl SecureGlobalContext {
    /// `engine` — the engine's own management tools, reachable from a script.
    ///
    /// All scripts are equal, so this is every script's. What makes that safe is not a privileged-script list but the thing the
    /// capability model already does — **every call is authorized against the
    /// calling `UserContext`, by the same function `/mcp` calls
    /// ([`crate::engine_api::execute_native_mcp_tool`])**. A script holding
    /// this global holds nothing its caller does not, and there is one
    /// implementation of each tool rather than an in-process copy that could
    /// drift from the HTTP one.
    ///
    /// Installed only for a [`Principal`] that came from a credential — a
    /// caller or a delegating person — because the capability check is not
    /// sufficient on its own: a scheduled job and `init()` run as synthetic
    /// administrators with nobody behind them, and a `sandbox.run` subset
    /// cannot distinguish "my own `console`" from "every script's logs".
    /// [`Principal`] has the argument.
    ///
    /// What is *not* here is a second authorization model. This adds no
    /// capability, no bypass and no special case; it reaches the same
    /// operations as `/engine/*` and `/mcp`, with the same checks.
    pub(super) fn setup_engine_object(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
    ) -> JsResult<()> {
        if !self.config.principal.reaches_engine_api() {
            return Ok(());
        }

        let global = ctx.globals();
        let host = rquickjs::Object::new(ctx.clone())?;

        // Discovery, so a script never embeds a tool list that drifts from
        // what this engine actually serves. Names and descriptions only: the
        // schemas are large, and a caller that wants one can read the
        // published `/engine/openapi.json`.
        let tools = Function::new(ctx.clone(), move |area: String| -> String {
            let area = area.trim().to_ascii_lowercase();
            let listed: Vec<serde_json::Value> = crate::engine_api::native_mcp_tool_descriptors()
                .into_iter()
                .filter(|tool| {
                    area.is_empty()
                        || tool.name.contains(&area)
                        || tool.description.to_ascii_lowercase().contains(&area)
                })
                .map(|tool| {
                    serde_json::json!({
                        "name": tool.name,
                        "description": tool.description,
                    })
                })
                .collect();
            serde_json::json!({ "tools": listed, "count": listed.len() }).to_string()
        })?;

        let user_ctx_call = self.user_context.clone();
        let uri_for_audit = script_uri.to_string();
        let call = Function::new(
            ctx.clone(),
            move |name: String, args_json: String| -> JsResult<String> {
                let args: serde_json::Value = serde_json::from_str(&args_json).map_err(|e| {
                    rquickjs::Error::new_from_js_message(
                        "engine.call",
                        "type_error",
                        &format!("engine.call: arguments are not valid JSON: {}", e),
                    )
                })?;

                // Named rather than found, so a name this engine does not
                // serve is a thrown error at the call site instead of a
                // refusal that reads like a permission problem.
                let Some(answer) =
                    crate::engine_api::execute_native_mcp_tool(&name, &args, &user_ctx_call)
                else {
                    return Err(rquickjs::Error::new_from_js_message(
                        "engine.call",
                        "unknown_tool",
                        &format!(
                            "engine.call: this engine has no tool called '{}' —                              engine.tools() lists them",
                            name
                        ),
                    ));
                };

                debug!(
                    script = %uri_for_audit,
                    tool = %name,
                    user = ?user_ctx_call.user_id,
                    "engine.call"
                );

                Ok(answer.to_string())
            },
        )?;

        host.set("tools", tools)?;
        host.set("call", call)?;
        global.set("__hostEngine", host)?;

        crate::bytecode::eval_program(ctx, "engine://engine-prelude", ENGINE_PRELUDE).map_err(
            |e| {
                rquickjs::Error::new_from_js_message(
                    "engine",
                    "prelude",
                    &format!("engine prelude failed to load: {}", e),
                )
            },
        )?;

        debug!("engine API initialized for script: {}", script_uri);
        Ok(())
    }
}
