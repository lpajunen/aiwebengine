//! `tools`: calling the MCP tools other scripts publish, as the caller.

use super::*;
use crate::security::{Capability, UserContext};
use rquickjs::{Function, Result as JsResult};
use tracing::info;

/// Builds `tools` over `__hostTools`.
pub(super) const TOOLS_PRELUDE: &str = include_str!("../../../assets/tools_prelude.js");

/// What a `readOnly` call gives up: everything that changes something, and
/// acting on what the caller does not own.
///
/// Listed as what is removed rather than what is kept, so a capability added
/// later stays with a read-only call until somebody decides it is a write —
/// the cost of that mistake is a refusal, where the other way round it would
/// be a write nobody approved.
const WRITES: [Capability; 12] = [
    Capability::WriteScripts,
    Capability::DeleteScripts,
    Capability::WriteAssets,
    Capability::DeleteAssets,
    Capability::DeleteLogs,
    Capability::ManageMcp,
    Capability::WriteScriptData,
    Capability::ManageScriptDatabase,
    Capability::AdministerEngine,
    Capability::WriteSecrets,
    Capability::WriteStorage,
    Capability::EnqueueTasks,
];

/// What a read-only call runs with: `user` without [`WRITES`].
pub(crate) fn read_only(user: &UserContext) -> UserContext {
    user.attenuated(
        user.capabilities
            .iter()
            .filter(|capability| !WRITES.contains(capability))
            .cloned()
            .collect::<Vec<_>>(),
    )
}

/// `principal` acting with `user` instead, everything else unchanged.
///
/// The delegated scopes travel with the call: a tool reached from a delegated
/// run is still the person's absence, and reaches only what they granted.
fn acting_as(principal: &Principal, user: UserContext) -> Principal {
    match principal {
        Principal::Caller(_) => Principal::Caller(user),
        Principal::Delegated { scopes, .. } => Principal::Delegated {
            user,
            scopes: scopes.clone(),
        },
        Principal::Contained(_) => Principal::Contained(user),
        Principal::Engine(label) => Principal::Engine(label),
    }
}

impl SecureGlobalContext {
    /// The Rust half of `tools` — another script's MCP tools, called in
    /// process.
    ///
    /// Nothing here is a new authority. `/mcp` already lets a signed-in
    /// person call any tool published on its host; this is that call made
    /// from inside an execution, authorized against the same person, so an
    /// app can use another app the way the person could. The tool runs as the
    /// calling execution's principal, narrowed however it was narrowed — all
    /// scripts are equal, so which script asks decides nothing.
    ///
    /// Gated by [`Capability::CallTools`], which the engine's own executions
    /// never hold and a delegation holds only when the person ticked `tools`.
    pub(super) fn setup_tools_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
    ) -> JsResult<()> {
        let global = ctx.globals();
        let host = rquickjs::Object::new(ctx.clone())?;

        let script_uri_list = script_uri.to_string();
        let user_list = self.user_context.clone();
        let list = Function::new(ctx.clone(), move |filter: String| -> String {
            if !user_list.has_capability(&Capability::CallTools) {
                return host_failure(
                    "SecurityError",
                    &capability_refusal("tools.list", &Capability::CallTools, &user_list),
                );
            }
            let filter = filter.trim().to_ascii_lowercase();
            let listed: Vec<serde_json::Value> = crate::mcp::tools_callable_from(&script_uri_list)
                .into_iter()
                .filter(|tool| {
                    filter.is_empty()
                        || tool.name.to_ascii_lowercase().contains(&filter)
                        || tool.description.to_ascii_lowercase().contains(&filter)
                        || tool.script_uri.to_ascii_lowercase().contains(&filter)
                })
                .map(|tool| {
                    serde_json::json!({
                        "name": tool.name,
                        "description": tool.description,
                        "inputSchema": tool.input_schema,
                        "script": tool.script_uri,
                    })
                })
                .collect();
            host_ok(serde_json::Value::Array(listed))
        })?;
        host.set("list", list)?;

        let script_uri_call = script_uri.to_string();
        let user_call = self.user_context.clone();
        let config_call = self.config.clone();
        let call = Function::new(
            ctx.clone(),
            move |ctx: rquickjs::Ctx<'_>,
                  name: String,
                  args_json: String,
                  read_only_call: bool|
                  -> String {
                if config_call.is_dry_run() {
                    // A tool runs real code against live data. A check that
                    // deploys nothing must not set that in motion.
                    return host_failure(
                        "DryRunError",
                        "tools.call: nothing was called - this is a dry run",
                    );
                }
                if !user_call.has_capability(&Capability::CallTools) {
                    return host_failure(
                        "SecurityError",
                        &capability_refusal("tools.call", &Capability::CallTools, &user_call),
                    );
                }
                let arguments: serde_json::Value = match serde_json::from_str(&args_json) {
                    Ok(value @ serde_json::Value::Object(_)) => value,
                    Ok(_) => {
                        return host_failure(
                            "TypeError",
                            "tools.call: arguments must be an object",
                        );
                    }
                    Err(e) => {
                        return host_failure(
                            "TypeError",
                            &format!("tools.call: arguments are not valid JSON: {}", e),
                        );
                    }
                };

                // A tool can call a tool. The budget bounds how long a chain
                // runs; this bounds how deep it goes before the native stack
                // does.
                let _depth = match crate::sandbox::DepthGuard::enter() {
                    Ok(guard) => guard,
                    Err(depth) => {
                        return host_failure(
                            "RangeError",
                            &format!("tools.call: {}", crate::sandbox::Refusal::TooDeep(depth)),
                        );
                    }
                };

                let user = if read_only_call {
                    read_only(&user_call)
                } else {
                    user_call.clone()
                };
                let principal = acting_as(&config_call.principal, user);

                info!(
                    caller = %script_uri_call,
                    tool = %name,
                    user = ?user_call.user_id,
                    principal = config_call.principal.kind(),
                    read_only = read_only_call,
                    "tools.call"
                );

                match crate::mcp::call_tool_from_script(
                    &script_uri_call,
                    &name,
                    arguments,
                    Self::sandbox_auth_context(&ctx),
                    principal,
                ) {
                    Ok(result) => host_ok(result),
                    Err(refusal @ crate::mcp::ScriptToolRefusal::NotFound(_)) => {
                        host_failure("NotFoundError", &format!("tools.call: {}", refusal))
                    }
                    Err(refusal) => host_failure("ToolError", &refusal.to_string()),
                }
            },
        )?;
        host.set("call", call)?;

        global.set("__hostTools", host)?;

        crate::bytecode::eval_program(ctx, "engine://tools-prelude", TOOLS_PRELUDE).map_err(
            |e| {
                rquickjs::Error::new_from_js_message(
                    "tools",
                    "prelude",
                    &format!("tools prelude failed to load: {}", e),
                )
            },
        )?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A read-only call keeps what reading needs and nothing that changes
    /// anything, and still names the same person.
    #[test]
    fn a_read_only_call_reads_as_the_same_person() {
        let user = UserContext::editor("u1".to_string());
        let narrowed = read_only(&user);

        assert_eq!(narrowed.user_id.as_deref(), Some("u1"));
        for kept in [
            Capability::ReadScriptData,
            Capability::ReadStorage,
            Capability::ReadAssets,
            Capability::CallTools,
        ] {
            assert!(narrowed.has_capability(&kept), "{:?} should stay", kept);
        }
        for write in WRITES {
            assert!(!narrowed.has_capability(&write), "{:?} should go", write);
        }
    }

    /// The delegated scopes travel with the call, so a tool reached from a
    /// delegated run reaches only what the person granted.
    #[test]
    fn a_delegated_caller_stays_delegated() {
        let principal = Principal::Delegated {
            user: UserContext::authenticated("u1".to_string()),
            scopes: vec![crate::delegation::Scope::Tools],
        };
        match acting_as(&principal, UserContext::authenticated("u1".to_string())) {
            Principal::Delegated { scopes, .. } => {
                assert_eq!(scopes, vec![crate::delegation::Scope::Tools])
            }
            other => panic!("expected a delegated principal, got {}", other.kind()),
        }
    }
}
