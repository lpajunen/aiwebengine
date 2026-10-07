//! `mcpRegistry`, `mcp` (asking and handing off) and `McpClient`.

use super::*;
use crate::repository;
use crate::security::{Capability, UserContext};
use rquickjs::{Function, Result as JsResult, function::Opt};
use tracing::debug;

pub(super) const MCP_PRELUDE: &str = include_str!("../../../assets/mcp_prelude.js");
/// Builds the `McpClient` class over `__hostMcpClient`.
pub(super) const MCP_CLIENT_PRELUDE: &str = include_str!("../../../assets/mcp_client_prelude.js");
/// Builds `mcpRegistry` over `__hostMcpRegistry`.
pub(super) const MCP_REGISTRY_PRELUDE: &str =
    include_str!("../../../assets/mcp_registry_prelude.js");

/// `{ name, description, mimeType }` off an optional metadata object, for
/// `mcpRegistry.registerResource`.
///
/// Every field is optional and a missing one comes back as `None` rather than
/// as a default, because the caller decides what to fall back to: the name
/// falls back to the asset's, and the MIME type falls back to the asset's own
/// at read time rather than at registration, so an asset re-uploaded as
/// something else is not described by a type recorded at `init()`.
pub(super) fn extract_resource_metadata(
    metadata: Option<&rquickjs::Object<'_>>,
) -> (Option<String>, Option<String>, Option<String>) {
    let mut name = None;
    let mut description = None;
    let mut mime_type = None;
    if let Some(meta) = metadata {
        if let Ok(value) = meta.get::<_, Option<String>>("name") {
            name = value;
        }
        if let Ok(value) = meta.get::<_, Option<String>>("description") {
            description = value;
        }
        if let Ok(value) = meta.get::<_, Option<String>>("mimeType") {
            mime_type = value;
        }
    }
    (name, description, mime_type)
}

/// What every `McpClient` arm requires before it does anything.
///
/// Both, always, and in this order: a call out to an MCP server is a network
/// request that carries a resolved secret, so an execution holding one of the
/// two and not the other may not make it. `UseNetwork` is checked first because
/// it is the coarser refusal — a context that may not call out at all should
/// hear that rather than be told about a credential it was never going to get
/// to use.
///
/// The `api` names the arm rather than the class, so the message points at the
/// call the script wrote (`McpClient.callTool`) instead of at the global.
pub(super) fn mcp_client_capabilities(api: &'static str, user: &UserContext) -> JsResult<()> {
    if !user.has_capability(&Capability::UseNetwork) {
        return Err(capability_error(api, &Capability::UseNetwork, user));
    }
    if !user.has_capability(&Capability::ReadSecrets) {
        return Err(capability_error(api, &Capability::ReadSecrets, user));
    }
    Ok(())
}

impl SecureGlobalContext {
    /// Setup MCP (Model Context Protocol) registry functions
    pub(super) fn setup_mcp_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
    ) -> JsResult<()> {
        let global = ctx.globals();
        let user_context = self.user_context.clone();
        let auditor = self.auditor.clone();
        let script_uri_owned = script_uri.to_string();
        let config = self.config.clone();

        // registerTool function - registers an MCP tool
        let user_ctx_register = user_context.clone();
        let auditor_register = auditor.clone();
        let script_uri_register = script_uri_owned.clone();
        let config_register = config.clone();
        let register_tool = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  name: String,
                  description: String,
                  input_schema_json: String,
                  handler_function: String|
                  -> JsResult<String> {
                if !config_register.registration_phase {
                    return Ok(refusal_answer(registration_inactive(
                        "mcpRegistry.registerTool",
                        &name,
                    )));
                }

                // Check capability - reuse ManageMcp for MCP tools
                if let Err(e) =
                    user_ctx_register.require_capability(&crate::security::Capability::ManageMcp)
                {
                    let auditor_clone = auditor_register.clone();
                    let user_id = user_ctx_register.user_id.clone();
                    tokio::task::spawn(async move {
                        let _ = auditor_clone
                            .log_authz_failure(
                                user_id,
                                "mcp".to_string(),
                                "register_tool".to_string(),
                                "ManageMcp".to_string(),
                            )
                            .await;
                    });
                    return Ok(host_failure("Error", &e.to_string()));
                }

                // Validate inputs
                if name.is_empty() || name.len() > 100 {
                    return Ok(host_failure(
                        "TypeError",
                        "Invalid tool name: must be between 1 and 100 characters",
                    ));
                }
                if description.is_empty() || description.len() > 1000 {
                    return Ok(host_failure(
                        "TypeError",
                        "Invalid description: must be between 1 and 1000 characters",
                    ));
                }

                // Parse and validate input schema JSON
                let input_schema: serde_json::Value = match serde_json::from_str(&input_schema_json)
                {
                    Ok(schema) => schema,
                    Err(e) => {
                        return Ok(host_failure(
                            "TypeError",
                            &format!("Invalid input schema: {}", e),
                        ));
                    }
                };

                // Check for dangerous patterns
                if input_schema_json.contains("__proto__")
                    || input_schema_json.contains("constructor")
                {
                    return Ok(host_failure(
                        "TypeError",
                        "Invalid schema: contains dangerous patterns",
                    ));
                }

                // Log the operation attempt
                let auditor_clone = auditor_register.clone();
                let user_id = user_ctx_register.user_id.clone();
                let name_clone = name.clone();
                let script_uri_clone = script_uri_register.clone();
                tokio::task::spawn(async move {
                    let _ = auditor_clone
                        .log_event(
                            crate::security::SecurityEvent::new(
                                crate::security::SecurityEventType::SystemSecurityEvent,
                                crate::security::SecuritySeverity::Medium,
                                user_id,
                            )
                            .with_resource("mcp".to_string())
                            .with_action("register_tool".to_string())
                            .with_detail("tool_name", &name_clone)
                            .with_detail("script_uri", &script_uri_clone),
                        )
                        .await;
                });

                debug!(
                    user_id = ?user_ctx_register.user_id,
                    name = %name,
                    "Secure registerTool called for MCP"
                );

                if config_register
                    .collect(
                        CollectedRegistration::new(RegistrationKind::McpTool, name.clone())
                            .with_handler(handler_function.clone()),
                    )
                    .is_some()
                {
                    return Ok(host_ok(serde_json::json!({ "ok": true })));
                }

                // Actually register the MCP tool
                crate::mcp::register_mcp_tool(
                    name.clone(),
                    description,
                    input_schema,
                    handler_function,
                    script_uri_register.clone(),
                );

                Ok(host_ok(serde_json::json!({ "ok": true })))
            },
        )?;

        // registerPrompt function - registers an MCP prompt
        let user_ctx_prompt = user_context.clone();
        let auditor_prompt = auditor.clone();
        let script_uri_prompt = script_uri_owned.clone();
        let config_prompt = config.clone();
        let register_prompt = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  name: String,
                  description: String,
                  arguments_json: String,
                  handler_function: String|
                  -> JsResult<String> {
                if !config_prompt.registration_phase {
                    return Ok(refusal_answer(registration_inactive(
                        "mcpRegistry.registerPrompt",
                        &name,
                    )));
                }

                // Check capability - reuse ManageMcp for MCP prompts
                if let Err(e) =
                    user_ctx_prompt.require_capability(&crate::security::Capability::ManageMcp)
                {
                    let auditor_clone = auditor_prompt.clone();
                    let user_id = user_ctx_prompt.user_id.clone();
                    tokio::task::spawn(async move {
                        let _ = auditor_clone
                            .log_authz_failure(
                                user_id,
                                "mcp".to_string(),
                                "register_prompt".to_string(),
                                "ManageMcp".to_string(),
                            )
                            .await;
                    });
                    return Ok(host_failure("Error", &e.to_string()));
                }

                // Validate inputs
                if name.is_empty() || name.len() > 100 {
                    return Ok(host_failure(
                        "TypeError",
                        "Invalid prompt name: must be between 1 and 100 characters",
                    ));
                }
                if description.is_empty() || description.len() > 1000 {
                    return Ok(host_failure(
                        "TypeError",
                        "Invalid description: must be between 1 and 1000 characters",
                    ));
                }
                if handler_function.is_empty() || handler_function.len() > 100 {
                    return Ok(host_failure(
                        "TypeError",
                        "Invalid handler function: must be between 1 and 100 characters",
                    ));
                }

                // Validate arguments JSON
                if arguments_json.contains("__proto__") || arguments_json.contains("constructor") {
                    return Ok(host_failure(
                        "TypeError",
                        "Invalid arguments: contains dangerous patterns",
                    ));
                }

                // Log the operation attempt
                let auditor_clone = auditor_prompt.clone();
                let user_id = user_ctx_prompt.user_id.clone();
                let name_clone = name.clone();
                let script_uri_clone = script_uri_prompt.clone();
                let handler_clone = handler_function.clone();
                tokio::task::spawn(async move {
                    let _ = auditor_clone
                        .log_event(
                            crate::security::SecurityEvent::new(
                                crate::security::SecurityEventType::SystemSecurityEvent,
                                crate::security::SecuritySeverity::Medium,
                                user_id,
                            )
                            .with_resource("mcp".to_string())
                            .with_action("register_prompt".to_string())
                            .with_detail("prompt_name", &name_clone)
                            .with_detail("handler", &handler_clone)
                            .with_detail("script_uri", &script_uri_clone),
                        )
                        .await;
                });

                debug!(
                    user_id = ?user_ctx_prompt.user_id,
                    name = %name,
                    handler = %handler_function,
                    "Secure registerPrompt called for MCP"
                );

                if config_prompt
                    .collect(
                        CollectedRegistration::new(RegistrationKind::McpPrompt, name.clone())
                            .with_handler(handler_function.clone()),
                    )
                    .is_some()
                {
                    return Ok(host_ok(serde_json::json!({ "ok": true })));
                }

                // Actually register the MCP prompt
                match crate::mcp::register_mcp_prompt(
                    name.clone(),
                    description,
                    arguments_json,
                    handler_function.clone(),
                    script_uri_prompt.clone(),
                ) {
                    Ok(_) => Ok(host_ok(serde_json::json!({ "ok": true }))),
                    Err(e) => Ok(host_failure("TypeError", &e.to_string())),
                }
            },
        )?;

        // registerResource function - publishes one of this script's assets as
        // an MCP resource. Deliberately shaped after
        // a file route (`registerRoute(path, { file })`) rather than after
        // `registerTool`:
        // what is being published is an asset the script already has, and the
        // only difference between the two is which protocol reaches it.
        let user_ctx_resource = user_context.clone();
        let auditor_resource = auditor.clone();
        let script_uri_resource = script_uri_owned.clone();
        let config_resource = config.clone();
        let register_resource = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  uri: String,
                  asset_name: String,
                  metadata: Opt<rquickjs::Object>|
                  -> JsResult<String> {
                if !config_resource.registration_phase {
                    return Ok(refusal_answer(registration_inactive(
                        "mcpRegistry.registerResource",
                        &uri,
                    )));
                }

                // The same capability as the rest of `mcpRegistry`, rather
                // than the `WriteAssets` its asset-route twin takes: what is
                // being decided here is whether a solution publishes an MCP
                // surface, not whether the asset may be written.
                if let Err(e) =
                    user_ctx_resource.require_capability(&crate::security::Capability::ManageMcp)
                {
                    let auditor_clone = auditor_resource.clone();
                    let user_id = user_ctx_resource.user_id.clone();
                    tokio::task::spawn(async move {
                        let _ = auditor_clone
                            .log_authz_failure(
                                user_id,
                                "mcp".to_string(),
                                "register_resource".to_string(),
                                "ManageMcp".to_string(),
                            )
                            .await;
                    });
                    return Ok(host_failure("Error", &e.to_string()));
                }

                // A resource URI is an opaque identifier to a client, but it
                // has to be one: something with a scheme, that a client can
                // round-trip through `resources/read` and display. Anything
                // shorter than `a:b` is a name that was meant to be a URI.
                if uri.len() < 3 || uri.len() > 500 {
                    return Ok(host_failure(
                        "TypeError",
                        "Invalid resource URI: must be 3-500 characters",
                    ));
                }
                let scheme_end = uri.find(':').unwrap_or(0);
                if scheme_end == 0 || scheme_end == uri.len() - 1 {
                    return Ok(host_failure(
                        "TypeError",
                        &(format!(
                            "Invalid resource URI '{}': must carry a scheme, as in \
                         'docs://handbook' or 'https://example.com/spec'",
                            uri
                        )),
                    ));
                }
                if uri.chars().any(|c| c.is_whitespace() || c.is_control()) {
                    return Ok(host_failure(
                        "TypeError",
                        &(format!(
                            "Invalid resource URI '{}': no whitespace or control characters",
                            uri
                        )),
                    ));
                }

                // The same two checks a file route makes, and for the
                // same reason: an asset name is a relative path, so `..` is
                // how one script would name another's file.
                if asset_name.is_empty() || asset_name.len() > 255 {
                    return Ok(host_failure(
                        "TypeError",
                        "Invalid asset name: must be 1-255 characters",
                    ));
                }
                if asset_name.contains("..") || asset_name.contains('\\') {
                    return Ok(host_failure(
                        "TypeError",
                        "Invalid asset name: path characters not allowed",
                    ));
                }

                let (name, description, mime_type) = extract_resource_metadata(metadata.0.as_ref());
                // A client's resource picker shows the name, so it always has
                // one — falling back to the asset's own name rather than to
                // the URI, which is the less readable of the two.
                let name = name.unwrap_or_else(|| asset_name.clone());

                // Verify the asset exists and belongs to this script, exactly
                // as a file route does. Registering a URI that answers
                // nothing would be a resource a client lists and cannot read.
                if repository::fetch_asset(&script_uri_resource, &asset_name).is_none() {
                    let reason = format!(
                        "Asset '{}' not found or not owned by script '{}'. Create the file \
                         under '{}' first, then register it",
                        asset_name,
                        script_uri_resource,
                        crate::exposure::RESOURCE_DIR
                    );
                    config_resource.note_dry_run_refusal(
                        CollectedRegistration::new(RegistrationKind::McpResource, uri.clone())
                            .with_refusal(reason.clone()),
                    );
                    return Ok(refusal_answer(reason));
                }

                if !crate::exposure::is_resource(&asset_name) {
                    crate::exposure::note_refusal(&script_uri_resource, true, &uri, &asset_name);
                    tracing::warn!(
                        script = %script_uri_resource,
                        uri = %uri,
                        asset = %asset_name,
                        "Refused to publish a file from outside '{}' as an MCP resource",
                        crate::exposure::RESOURCE_DIR,
                    );
                    let reason = format!(
                        "Refused: '{}' is not under '{}', so it is not a file an MCP client \
                         may read. Move it to '{}{}' and register that. See \
                         /engine/exposure_report.",
                        asset_name,
                        crate::exposure::RESOURCE_DIR,
                        crate::exposure::RESOURCE_DIR,
                        asset_name,
                    );
                    config_resource.note_dry_run_refusal(
                        CollectedRegistration::new(RegistrationKind::McpResource, uri.clone())
                            .with_refusal(reason.clone()),
                    );
                    return Ok(refusal_answer(reason));
                }

                debug!(
                    user_id = ?user_ctx_resource.user_id,
                    uri = %uri,
                    "Secure registerResource called for MCP"
                );

                if config_resource
                    .collect(CollectedRegistration::new(
                        RegistrationKind::McpResource,
                        uri.clone(),
                    ))
                    .is_some()
                {
                    return Ok(host_ok(serde_json::json!({ "ok": true })));
                }

                crate::mcp::register_mcp_resource(
                    uri.clone(),
                    name,
                    description.unwrap_or_default(),
                    mime_type,
                    asset_name.clone(),
                    script_uri_resource.clone(),
                );

                Ok(host_ok(serde_json::json!({ "ok": true })))
            },
        )?;

        // Create mcpRegistry object
        let mcp_registry = rquickjs::Object::new(ctx.clone())?;
        mcp_registry.set("registerTool", register_tool)?;
        mcp_registry.set("registerPrompt", register_prompt)?;
        mcp_registry.set("registerResource", register_resource)?;
        global.set("__hostMcpRegistry", mcp_registry)?;
        crate::bytecode::eval_program(ctx, "engine://mcp-registry-prelude", MCP_REGISTRY_PRELUDE)
            .map_err(|e| {
            rquickjs::Error::new_from_js_message(
                "mcpRegistry",
                "prelude",
                &format!("mcpRegistry prelude failed to load: {}", e),
            )
        })?;

        // The other half of MCP: asking the caller a question mid-tool.
        self.setup_mcp_elicitation(ctx, script_uri)?;

        // Setup McpClient class for connecting to external MCP servers
        self.setup_mcp_client_class(ctx, script_uri)?;

        Ok(())
    }
}

impl SecureGlobalContext {
    /// `mcp.ask` / `mcp.canAsk` / `mcp.once`, over the thread-local exchange.
    ///
    /// Every binding here reads [`crate::mcp_elicitation`]'s thread-local
    /// rather than anything captured at install time, which is the one thing
    /// that has to be true of them: globals are installed once per execution,
    /// and what a call must see is the exchange the *current* tool call
    /// established. Installed unconditionally, because an execution with no
    /// exchange is not an error — it is a scheduled job or a listener, where
    /// `canAsk` is false and `ask` throws.
    pub(super) fn setup_mcp_elicitation(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
    ) -> JsResult<()> {
        use crate::mcp_elicitation as elicitation;

        let host = rquickjs::Object::new(ctx.clone())?;

        let can_ask = Function::new(ctx.clone(), || -> bool { elicitation::can_ask() })?;
        host.set("canAsk", can_ask)?;

        let asked_so_far = Function::new(ctx.clone(), || -> usize { elicitation::asked_so_far() })?;
        host.set("askedSoFar", asked_so_far)?;

        // `null` rather than `undefined` for "not answered", so the prelude can
        // tell an absent answer from one whose value is legitimately falsy.
        let answer = Function::new(ctx.clone(), |key: String| -> Option<String> {
            elicitation::answer_for(&key).map(|value| value.to_string())
        })?;
        host.set("answer", answer)?;

        let ask = Function::new(ctx.clone(), |key: String, request: String| {
            let parsed = serde_json::from_str(&request).unwrap_or(serde_json::Value::Null);
            elicitation::record_ask(&key, parsed);
        })?;
        host.set("ask", ask)?;

        let memo_get = Function::new(ctx.clone(), |key: String| -> Option<String> {
            elicitation::memo_get(&key).map(|value| value.to_string())
        })?;
        host.set("memoGet", memo_get)?;

        let memo_set = Function::new(ctx.clone(), |key: String, value: String| {
            let parsed = serde_json::from_str(&value).unwrap_or(serde_json::Value::Null);
            elicitation::memo_set(&key, parsed);
        })?;
        host.set("memoSet", memo_set)?;

        // `mcp.task` — the other way a tool call ends without an answer.
        //
        // The script decides, not the engine: by the time the engine could
        // measure that a handler is slow, the handler has started and the
        // answer is a value rather than a handle. A handler that knows its own
        // work is long says so, and the work moves to the durable queue.
        let user_ctx_task = self.user_context.clone();
        let script_uri_task = script_uri.to_string();
        let hand_off = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, spec_json: String| -> JsResult<String> {
                // Queueing is queueing, whoever asked for it. A delegated run
                // that may not enqueue must not get one by routing through the
                // MCP surface.
                if !user_ctx_task.has_capability(&Capability::EnqueueTasks) {
                    return Err(capability_error(
                        "mcp.task",
                        &Capability::EnqueueTasks,
                        &user_ctx_task,
                    ));
                }

                let spec: serde_json::Value = serde_json::from_str(&spec_json).map_err(|e| {
                    rquickjs::Error::new_from_js_message(
                        "mcp.task",
                        "options",
                        &format!("Invalid task options: {}", e),
                    )
                })?;

                let handler = spec
                    .get("handler")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();
                if handler.is_empty() {
                    return Err(rquickjs::Error::new_from_js_message(
                        "mcp.task",
                        "handler",
                        "mcp.task: a handler name is required",
                    ));
                }

                let method = spec
                    .get("method")
                    .and_then(|value| value.as_str())
                    .unwrap_or("tools/call")
                    .to_string();
                let target = spec
                    .get("target")
                    .and_then(|value| value.as_str())
                    .unwrap_or_default()
                    .to_string();
                let status_message = spec
                    .get("statusMessage")
                    .and_then(|value| value.as_str())
                    .map(str::to_string);

                let queued = crate::tasks::blocking::enqueue(crate::tasks::NewTask {
                    script_uri: script_uri_task.clone(),
                    handler_name: handler,
                    payload: spec
                        .get("payload")
                        .cloned()
                        .unwrap_or(serde_json::json!({})),
                    run_at: None,
                    // One attempt. A retried tool call is a second run of work
                    // the client was told had started, with no way to tell it
                    // the first attempt failed — and `tasks.rs`'s backoff is
                    // built for work nobody is waiting on. A handler that wants
                    // retries can chain them itself and report through
                    // `statusMessage`.
                    max_attempts: Some(1),
                    enqueued_by: user_ctx_task.user_id.clone(),
                    // Script context. Running a queued tool call as the caller
                    // would need a delegation grant, and an MCP client holding
                    // a token is not the same as a person having consented to
                    // background work in their name.
                    run_as: None,
                    lane: spec
                        .get("lane")
                        .and_then(|value| value.as_str())
                        .map(str::to_string),
                })
                .map_err(|e| {
                    rquickjs::Error::new_from_js_message(
                        "mcp.task",
                        "enqueue",
                        &format!("mcp.task: could not queue the work: {}", e),
                    )
                })?;

                // The handle is recorded before the response goes out, which
                // the specification requires: a client holding a `taskId` that
                // names nothing is worse than a slow answer.
                let task_id = uuid::Uuid::new_v4();
                let created = crate::database::run_blocking(crate::mcp_tasks::create(
                    task_id,
                    queued.task_id,
                    &script_uri_task,
                    &method,
                    &target,
                    status_message.as_deref(),
                ))
                .map_err(|e| {
                    // The work is queued and the handle is not, so the client
                    // would have no way to reach it. Cancelling is the honest
                    // unwind: better nothing ran than something ran that
                    // nobody can collect.
                    let _ = crate::tasks::blocking::cancel(queued.task_id);
                    rquickjs::Error::new_from_js_message(
                        "mcp.task",
                        "record",
                        &format!("mcp.task: could not record the task handle: {}", e),
                    )
                })?;

                let create_result = created.create_result();
                elicitation::record_handoff(create_result.clone());
                Ok(create_result.to_string())
            },
        )?;
        host.set("handOff", hand_off)?;

        // Whether this call may hand back a handle at all: a tool call whose
        // client declared the extension. Two conditions rather than one,
        // because they fail for different reasons and a script wants to know
        // which — there is no request to answer at all in a scheduled job,
        // while a client that did not declare the extension is a request that
        // has to be answered synchronously.
        let can_task = Function::new(ctx.clone(), || -> bool {
            elicitation::in_exchange() && elicitation::can_hand_off()
        })?;
        host.set("canTask", can_task)?;

        ctx.globals().set("__hostMcp", host)?;

        crate::bytecode::eval_program(ctx, "engine://mcp-prelude", MCP_PRELUDE).map_err(|e| {
            rquickjs::Error::new_from_js_message(
                "mcp",
                "prelude",
                &format!("mcp prelude failed to load: {}", e),
            )
        })?;

        Ok(())
    }
}

impl SecureGlobalContext {
    /// Setup McpClient class for external MCP server connections
    ///
    /// Every arm here is gated on both [`Capability::UseNetwork`] and
    /// [`Capability::ReadSecrets`], because a call to an external MCP server is
    /// unconditionally both: it opens an outbound request to a caller-chosen
    /// URL, and it resolves `secret_identifier` host-side and sends it as a
    /// `Bearer` token. There is no unauthenticated arm to leave ungated —
    /// [`crate::mcp_client::McpClient`] answers `SecretNotFound` rather than
    /// sending the request without one.
    ///
    /// **The gate has to be on the methods and not only on the constructor.**
    /// `constructor` returns a plain JSON blob and `_listTools` / `_callTool`
    /// rebuild the client from whatever blob they are handed, so a check that
    /// sat only on the constructor would be one line of hand-written JSON away
    /// from being skipped. The constructor check is there to refuse early,
    /// where the message names the call the script actually wrote; the two on
    /// the methods are the enforcement.
    ///
    /// Ungated, this would be the way around
    /// [`super::capabilities::UserContext::attenuated`]: model-authored source
    /// in a narrowed `sandbox.run` could reach any public address and spend the
    /// person's API key getting there.
    pub(super) fn setup_mcp_client_class(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
    ) -> JsResult<()> {
        let global = ctx.globals();
        let script_uri_owned = script_uri.to_string();
        // Capture user_id for secret resolution (user_secrets first, then script_secrets)
        let user_id_for_mcp = self.user_context.user_id.clone();

        // One clone per arm below: each closure outlives this function and the
        // refusal names the caller's own tier, so the context travels with it.
        let user_ctx_new = self.user_context.clone();
        let user_ctx_list = self.user_context.clone();
        let user_ctx_call = self.user_context.clone();

        // McpClient constructor
        let mcp_client_constructor = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  server_url: String,
                  secret_identifier: String|
                  -> JsResult<String> {
                mcp_client_capabilities("constructor", &user_ctx_new)?;

                // Create MCP client instance (just validate parameters)
                let _client = crate::mcp_client::McpClient::scoped(
                    server_url.clone(),
                    secret_identifier.clone(),
                    user_ctx_new.network_scope.clone(),
                )
                .map_err(|e| {
                    rquickjs::Error::new_from_js_message(
                        "McpClient",
                        "constructor",
                        &format!("Failed to create MCP client: {}", e),
                    )
                })?;

                // Serialize client to JSON (we'll store server_url and secret_identifier)
                let client_data = serde_json::json!({
                    "serverUrl": server_url,
                    "secretIdentifier": secret_identifier,
                });

                Ok(serde_json::to_string(&client_data).unwrap())
            },
        )?;

        // Create McpClient class object with constructor and prototype
        let mcp_client_class = rquickjs::Object::new(ctx.clone())?;

        // listTools method
        let script_uri_list = script_uri_owned.clone();
        let user_id_list = user_id_for_mcp.clone();
        let list_tools = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, client_data_json: String| -> JsResult<String> {
                // Before the blob is even parsed: what it names is a URL and a
                // secret, and neither is this execution's to reach.
                mcp_client_capabilities("listTools", &user_ctx_list)?;

                // Parse client data
                let client_data: serde_json::Value = serde_json::from_str(&client_data_json)
                    .map_err(|e| {
                        rquickjs::Error::new_from_js_message(
                            "McpClient",
                            "listTools",
                            &format!("Invalid client data: {}", e),
                        )
                    })?;

                let server_url = client_data["serverUrl"].as_str().ok_or_else(|| {
                    rquickjs::Error::new_from_js_message(
                        "McpClient",
                        "listTools",
                        "Missing serverUrl in client data",
                    )
                })?;

                let secret_identifier =
                    client_data["secretIdentifier"].as_str().ok_or_else(|| {
                        rquickjs::Error::new_from_js_message(
                            "McpClient",
                            "listTools",
                            "Missing secretIdentifier in client data",
                        )
                    })?;

                // Create client
                let client = crate::mcp_client::McpClient::scoped(
                    server_url.to_string(),
                    secret_identifier.to_string(),
                    user_ctx_list.network_scope.clone(),
                )
                .map_err(|e| {
                    rquickjs::Error::new_from_js_message(
                        "McpClient",
                        "listTools",
                        &format!("Failed to create client: {}", e),
                    )
                })?;

                // List tools
                let tools = client
                    .list_tools(&script_uri_list, user_id_list.as_deref())
                    .map_err(|e| {
                        rquickjs::Error::new_from_js_message(
                            "McpClient",
                            "listTools",
                            &format!("Failed to list tools: {}", e),
                        )
                    })?;

                // Serialize tools to JSON
                serde_json::to_string(&tools).map_err(|e| {
                    rquickjs::Error::new_from_js_message(
                        "McpClient",
                        "listTools",
                        &format!("Failed to serialize tools: {}", e),
                    )
                })
            },
        )?;

        // callTool method
        let script_uri_call = script_uri_owned.clone();
        let user_id_call = user_id_for_mcp.clone();
        let call_tool = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  client_data_json: String,
                  tool_name: String,
                  arguments_json: String|
                  -> JsResult<String> {
                mcp_client_capabilities("callTool", &user_ctx_call)?;

                // Parse client data
                let client_data: serde_json::Value = serde_json::from_str(&client_data_json)
                    .map_err(|e| {
                        rquickjs::Error::new_from_js_message(
                            "McpClient",
                            "callTool",
                            &format!("Invalid client data: {}", e),
                        )
                    })?;

                let server_url = client_data["serverUrl"].as_str().ok_or_else(|| {
                    rquickjs::Error::new_from_js_message(
                        "McpClient",
                        "callTool",
                        "Missing serverUrl in client data",
                    )
                })?;

                let secret_identifier =
                    client_data["secretIdentifier"].as_str().ok_or_else(|| {
                        rquickjs::Error::new_from_js_message(
                            "McpClient",
                            "callTool",
                            "Missing secretIdentifier in client data",
                        )
                    })?;

                // Parse arguments
                let arguments: serde_json::Value =
                    serde_json::from_str(&arguments_json).map_err(|e| {
                        rquickjs::Error::new_from_js_message(
                            "McpClient",
                            "callTool",
                            &format!("Invalid arguments JSON: {}", e),
                        )
                    })?;

                // Create client
                let client = crate::mcp_client::McpClient::scoped(
                    server_url.to_string(),
                    secret_identifier.to_string(),
                    user_ctx_call.network_scope.clone(),
                )
                .map_err(|e| {
                    rquickjs::Error::new_from_js_message(
                        "McpClient",
                        "callTool",
                        &format!("Failed to create client: {}", e),
                    )
                })?;

                // Call tool
                let result = match client.call_tool(
                    tool_name.clone(),
                    arguments,
                    &script_uri_call,
                    user_id_call.as_deref(),
                ) {
                    Ok(res) => res,
                    Err(e) => {
                        // For JSON-RPC errors, return them as {error: {...}} objects
                        if let crate::mcp_client::McpClientError::JsonRpc(code, message) = e {
                            let error_obj = serde_json::json!({
                                "error": {
                                    "code": code,
                                    "message": message
                                }
                            });
                            return Ok(serde_json::to_string(&error_obj).unwrap());
                        }

                        // For other errors, throw JavaScript exceptions
                        return Err(rquickjs::Error::new_from_js_message(
                            "McpClient",
                            "callTool",
                            &format!("Failed to call tool '{}': {}", tool_name, e),
                        ));
                    }
                };

                // Serialize result to JSON
                serde_json::to_string(&result).map_err(|e| {
                    rquickjs::Error::new_from_js_message(
                        "McpClient",
                        "callTool",
                        &format!("Failed to serialize result: {}", e),
                    )
                })
            },
        )?;

        // Set methods on the class
        mcp_client_class.set("constructor", mcp_client_constructor)?;
        mcp_client_class.set("_listTools", list_tools)?;
        mcp_client_class.set("_callTool", call_tool)?;

        // Set the class on global scope
        global.set("__hostMcpClient", mcp_client_class)?;
        crate::bytecode::eval_program(ctx, "engine://mcp-client-prelude", MCP_CLIENT_PRELUDE)
            .map_err(|e| {
                rquickjs::Error::new_from_js_message(
                    "McpClient",
                    "prelude",
                    &format!("McpClient prelude failed to load: {}", e),
                )
            })?;

        debug!("McpClient class initialized for external MCP server connections");

        Ok(())
    }
}
