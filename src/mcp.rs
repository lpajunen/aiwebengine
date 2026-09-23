use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use tracing::{debug, error};

/// Represents an MCP tool registration from JavaScript
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpTool {
    /// The tool name
    pub name: String,
    /// The tool description
    pub description: String,
    /// The input schema (JSON Schema)
    pub input_schema: serde_json::Value,
    /// The handler function name in the JavaScript script
    pub handler_function: String,
    /// The script URI that contains this tool
    pub script_uri: String,
}

/// Represents an argument for an MCP prompt
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptArgument {
    /// The argument name
    pub name: String,
    /// The argument description
    pub description: String,
    /// Whether this argument is required
    pub required: bool,
}

/// Represents an MCP prompt registration from JavaScript
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpPrompt {
    /// The prompt name
    pub name: String,
    /// The prompt description
    pub description: String,
    /// The prompt arguments
    pub arguments: Vec<PromptArgument>,
    /// The JavaScript handler function name
    #[serde(skip)]
    pub handler_function: String,
    /// The script URI that contains this prompt
    #[serde(skip)]
    pub script_uri: String,
}

/// An asset a script has published as an MCP resource.
///
/// A resource is the read half of MCP: content a client may fetch by URI and
/// put in front of a model, as against a tool, which is something it may run.
/// The engine's answer to it is deliberately not a handler — it is
/// `routeRegistry.registerAssetRoute` pointed at `/mcp` instead of at a path.
/// Both publish an asset the script already has under a name callers can
/// reach; the only difference is which protocol does the reaching, which is
/// why this carries an `asset_name` rather than a `handler_function`.
///
/// That decision is what keeps the surface honest. A resource whose content
/// came from a handler would be a tool with a different spelling — it could
/// read a database, call out, and answer differently every time, none of which
/// a client caching by URI has any reason to expect. An asset cannot: it is
/// bytes in the repository, it changes only when somebody writes it, and
/// `revisions.rs` already records that they did.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpResource {
    /// The resource URI, as clients name it. Chosen by the script.
    pub uri: String,
    /// A short human-readable name, shown in a client's resource picker.
    pub name: String,
    /// What the resource is for.
    pub description: String,
    /// The MIME type, if the script overrode the asset's own.
    pub mime_type: Option<String>,
    /// The asset backing it, in the registering script's own asset store.
    #[serde(skip)]
    pub asset_name: String,
    /// The script that registered it.
    #[serde(skip)]
    pub script_uri: String,
}

/// Registry for storing MCP tools, prompts and resources registered from
/// JavaScript
#[derive(Debug, Clone, Default)]
pub struct McpRegistry {
    /// Registered tools (key: tool name, value: tool definition)
    pub tools: HashMap<String, McpTool>,
    /// Registered prompts (key: prompt name, value: prompt definition)
    pub prompts: HashMap<String, McpPrompt>,
    /// Registered resources (key: resource URI, value: resource definition)
    pub resources: HashMap<String, McpResource>,
}

impl McpRegistry {
    /// Create a new empty registry
    pub fn new() -> Self {
        Self::default()
    }

    /// Clear all registrations from a specific script URI
    pub fn clear_script_registrations(&mut self, script_uri: &str) {
        debug!("Clearing MCP registrations for script: {}", script_uri);

        // Remove tools from this script
        let tools_to_remove: Vec<String> = self
            .tools
            .iter()
            .filter(|(_, tool)| tool.script_uri == script_uri)
            .map(|(name, _)| name.clone())
            .collect();

        for tool_name in tools_to_remove {
            self.tools.remove(&tool_name);
            debug!(
                "Removed MCP tool '{}' from script '{}'",
                tool_name, script_uri
            );
        }

        // Remove prompts from this script
        let prompts_to_remove: Vec<String> = self
            .prompts
            .iter()
            .filter(|(_, prompt)| prompt.script_uri == script_uri)
            .map(|(name, _)| name.clone())
            .collect();

        for prompt_name in prompts_to_remove {
            self.prompts.remove(&prompt_name);
            debug!(
                "Removed MCP prompt '{}' from script '{}'",
                prompt_name, script_uri
            );
        }

        // Resources from this script. Same reasoning as the two above: a
        // registration belongs to the version of the code that declared it, so
        // re-running `init()` must not leave a resource pointing at an asset
        // the new version no longer publishes.
        let resources_to_remove: Vec<String> = self
            .resources
            .iter()
            .filter(|(_, resource)| resource.script_uri == script_uri)
            .map(|(uri, _)| uri.clone())
            .collect();

        for resource_uri in resources_to_remove {
            self.resources.remove(&resource_uri);
            debug!(
                "Removed MCP resource '{}' from script '{}'",
                resource_uri, script_uri
            );
        }
    }

    /// Register an MCP tool
    pub fn register_tool(&mut self, name: String, tool: McpTool) {
        debug!("Registering MCP tool: {}", name);
        self.tools.insert(name, tool);
    }

    /// Get all registered tools
    pub fn get_tools(&self) -> &HashMap<String, McpTool> {
        &self.tools
    }

    /// Get a specific tool by name
    pub fn get_tool(&self, name: &str) -> Option<&McpTool> {
        self.tools.get(name)
    }

    /// Register an MCP prompt
    pub fn register_prompt(&mut self, name: String, prompt: McpPrompt) {
        debug!("Registering MCP prompt: {}", name);
        self.prompts.insert(name, prompt);
    }

    /// Get all registered prompts
    pub fn get_prompts(&self) -> &HashMap<String, McpPrompt> {
        &self.prompts
    }

    /// Get a specific prompt by name
    pub fn get_prompt(&self, name: &str) -> Option<&McpPrompt> {
        self.prompts.get(name)
    }

    /// Register an MCP resource
    pub fn register_resource(&mut self, uri: String, resource: McpResource) {
        debug!("Registering MCP resource: {}", uri);
        self.resources.insert(uri, resource);
    }

    /// Get all registered resources
    pub fn get_resources(&self) -> &HashMap<String, McpResource> {
        &self.resources
    }

    /// Get a specific resource by URI
    pub fn get_resource(&self, uri: &str) -> Option<&McpResource> {
        self.resources.get(uri)
    }
}

lazy_static::lazy_static! {
    pub static ref MCP_REGISTRY: Arc<RwLock<McpRegistry>> = Arc::new(RwLock::new(McpRegistry::new()));
}

/// Get a reference to the global MCP registry
pub fn get_registry() -> Arc<RwLock<McpRegistry>> {
    Arc::clone(&MCP_REGISTRY)
}

/// Register an MCP tool from JavaScript
pub fn register_mcp_tool(
    name: String,
    description: String,
    input_schema: serde_json::Value,
    handler_function: String,
    script_uri: String,
) {
    debug!(
        "Registering MCP tool: {} with handler: {} from script: {}",
        name, handler_function, script_uri
    );

    let tool = McpTool {
        name: name.clone(),
        description,
        input_schema,
        handler_function,
        script_uri: script_uri.clone(),
    };

    if let Ok(mut registry) = get_registry().write() {
        registry.register_tool(name.clone(), tool);
        debug!(
            "Successfully registered MCP tool: {} - total tools: {}",
            name,
            registry.get_tools().len()
        );
    } else {
        error!("Failed to acquire write lock on MCP registry");
    }
}

/// Register an MCP prompt from JavaScript
pub fn register_mcp_prompt(
    name: String,
    description: String,
    arguments_json: String,
    handler_function: String,
    script_uri: String,
) -> Result<(), String> {
    debug!(
        "Registering MCP prompt: {} from script: {} with handler: {}",
        name, script_uri, handler_function
    );

    // Parse arguments JSON
    let arguments: Vec<PromptArgument> = serde_json::from_str(&arguments_json)
        .map_err(|e| format!("Failed to parse prompt arguments: {}", e))?;

    let prompt = McpPrompt {
        name: name.clone(),
        description,
        arguments,
        handler_function: handler_function.clone(),
        script_uri: script_uri.clone(),
    };

    if let Ok(mut registry) = get_registry().write() {
        registry.register_prompt(name.clone(), prompt);
        debug!(
            "Successfully registered MCP prompt: {} - total prompts: {}",
            name,
            registry.get_prompts().len()
        );
        Ok(())
    } else {
        error!("Failed to acquire write lock on MCP registry");
        Err("Failed to acquire write lock on MCP registry".to_string())
    }
}

/// Register an MCP resource from JavaScript.
///
/// The asset is *not* read here. Registration records which asset backs the
/// URI and `resources/read` fetches it when somebody asks, so that a resource
/// answers with what the asset says now rather than with what it said at
/// `init()` — an asset written after startup, by the script itself or by an
/// editor, is served without a redeploy. The same reason
/// `registerAssetRoute` does not copy an asset into the route index.
pub fn register_mcp_resource(
    uri: String,
    name: String,
    description: String,
    mime_type: Option<String>,
    asset_name: String,
    script_uri: String,
) {
    debug!(
        "Registering MCP resource: {} backed by asset '{}' from script: {}",
        uri, asset_name, script_uri
    );

    let resource = McpResource {
        uri: uri.clone(),
        name,
        description,
        mime_type,
        asset_name,
        script_uri,
    };

    if let Ok(mut registry) = get_registry().write() {
        registry.register_resource(uri.clone(), resource);
        debug!(
            "Successfully registered MCP resource: {} - total resources: {}",
            uri,
            registry.get_resources().len()
        );
    } else {
        error!("Failed to acquire write lock on MCP registry");
    }
}

/// List all registered MCP resources
pub fn list_resources() -> Vec<McpResource> {
    match get_registry().read() {
        Ok(registry) => registry.get_resources().values().cloned().collect(),
        Err(_) => {
            error!("Failed to acquire read lock on MCP registry");
            Vec::new()
        }
    }
}

/// The resources a client connecting on `host` should see. Filtered like
/// [`list_tools_for_host`], and sorted for the same reason.
pub async fn list_resources_for_host(host: &str) -> Vec<McpResource> {
    let resources = list_resources();
    let host_scripts = crate::route_index::scripts_for_host(host).await;
    let mut resources: Vec<McpResource> = match &host_scripts {
        Some(allowed) => resources
            .into_iter()
            .filter(|resource| allowed.contains(&resource.script_uri))
            .collect(),
        // Host binding not in force; every script publishes everywhere.
        None => resources,
    };
    resources.sort_by(|a, b| a.uri.cmp(&b.uri));
    resources
}

/// The resource `uri` names, if it is published on `host`.
///
/// Reading has to repeat the listing's filter for the reason
/// [`tool_is_available_on_host`] gives: a URI learned somewhere else must not
/// reach content this host does not publish just by being named. It answers
/// `None` either way, so "not published here" and "no such resource" are one
/// reply — a client that could tell them apart could enumerate the resources
/// of scripts bound to other hosts.
pub async fn resource_for_host(uri: &str, host: &str) -> Option<McpResource> {
    let resource = match get_registry().read() {
        Ok(registry) => registry.get_resource(uri).cloned(),
        Err(e) => {
            error!("Failed to read MCP registry for host check: {}", e);
            return None;
        }
    }?;
    if crate::route_index::script_serves_host(&resource.script_uri, host).await {
        Some(resource)
    } else {
        None
    }
}

/// Clear all MCP registrations (tools and prompts) from a specific script URI
pub fn clear_script_mcp_registrations(script_uri: &str) {
    debug!("Clearing MCP registrations for script: {}", script_uri);
    if let Ok(mut registry) = get_registry().write() {
        registry.clear_script_registrations(script_uri);
        debug!(
            "Successfully cleared MCP registrations for script: {}",
            script_uri
        );
    } else {
        error!("Failed to acquire write lock on MCP registry for clearing");
    }
}

/// Script URI stamped on the engine's own tools, which have no script behind
/// them. Distinguishes them from script-registered tools when filtering.
pub const NATIVE_TOOL_URI: &str = "engine://native";

/// List all registered MCP tools: native engine tools first, then
/// script-registered tools. A script tool whose name collides with a native
/// tool is omitted — native tools always win at dispatch time.
pub fn list_tools() -> Vec<McpTool> {
    let mut tools: Vec<McpTool> = crate::engine_api::native_mcp_tool_descriptors()
        .into_iter()
        .map(|descriptor| McpTool {
            name: descriptor.name.to_string(),
            description: descriptor.description.to_string(),
            input_schema: descriptor.input_schema,
            handler_function: String::new(),
            script_uri: NATIVE_TOOL_URI.to_string(),
        })
        .collect();

    if let Ok(registry) = get_registry().read() {
        let native_names: Vec<String> = tools.iter().map(|t| t.name.clone()).collect();
        tools.extend(
            registry
                .get_tools()
                .values()
                .filter(|tool| !native_names.iter().any(|name| name == &tool.name))
                .cloned(),
        );
    } else {
        error!("Failed to acquire read lock on MCP registry");
    }
    tools
}

/// The tools a client connecting on `host` should see.
///
/// Script tools are dropped unless their script publishes on that host, so a
/// tool registered by an admin-only script is absent from the listing on a
/// content host.
///
/// `native_allowed` decides the engine's own tools — script CRUD, secrets,
/// user roles — which are not script-backed and so have no binding of their
/// own. It comes from `server.management_hosts`, the same list that governs
/// `/engine/*`, so the management surface is reachable on the same hostnames
/// whichever protocol is used to get at it.
pub async fn list_tools_for_host(host: &str, native_allowed: bool) -> Vec<McpTool> {
    let tools = list_tools();
    let host_scripts = crate::route_index::scripts_for_host(host).await;

    let mut tools: Vec<McpTool> = tools
        .into_iter()
        .filter(|tool| {
            if tool.script_uri == NATIVE_TOOL_URI {
                return native_allowed;
            }
            match &host_scripts {
                Some(allowed) => allowed.contains(&tool.script_uri),
                // Host binding not in force; every script publishes everywhere
                None => true,
            }
        })
        .collect();
    // A stable order, which the specification asks for and [`LIST_CACHE_TTL_MS`]
    // now depends on: a client caching this list compares it against the next
    // one, and a registry whose iteration order moves would look like a change
    // on every poll. It also keeps an LLM's prompt cache warm across turns.
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    tools
}

/// The prompts a client connecting on `host` should see. Filtered like
/// [`list_tools_for_host`].
pub async fn list_prompts_for_host(host: &str) -> Vec<McpPrompt> {
    let prompts = list_prompts();
    let Some(allowed) = crate::route_index::scripts_for_host(host).await else {
        let mut prompts = prompts;
        prompts.sort_by(|a, b| a.name.cmp(&b.name));
        return prompts;
    };
    let mut prompts: Vec<McpPrompt> = prompts
        .into_iter()
        .filter(|prompt| allowed.contains(&prompt.script_uri))
        .collect();
    prompts.sort_by(|a, b| a.name.cmp(&b.name));
    prompts
}

/// Whether a tool may be called from `host`.
///
/// Dispatch has to repeat the listing's filter: a client that learned a tool
/// name elsewhere must not be able to reach a tool that is not published here
/// just by naming it.
pub async fn tool_is_available_on_host(tool_name: &str, host: &str, native_allowed: bool) -> bool {
    // Asked before the script registry because native tools win at dispatch
    // ([`execute_mcp_tool`]): a script registering a colliding name would
    // otherwise decide, with its own binding, whether the native tool runs.
    if crate::engine_api::is_native_mcp_tool(tool_name) {
        return native_allowed;
    }

    let script_uri = match get_registry().read() {
        Ok(registry) => match registry.get_tool(tool_name) {
            Some(tool) => tool.script_uri.clone(),
            // Unknown tool; about to fail with "tool not found" anyway.
            None => return true,
        },
        Err(e) => {
            error!("Failed to read MCP registry for host check: {}", e);
            return false;
        }
    };
    crate::route_index::script_serves_host(&script_uri, host).await
}

/// List all registered MCP prompts
pub fn list_prompts() -> Vec<McpPrompt> {
    if let Ok(registry) = get_registry().read() {
        registry.get_prompts().values().cloned().collect()
    } else {
        error!("Failed to acquire read lock on MCP registry");
        Vec::new()
    }
}

/// Get a specific prompt by name
pub fn get_prompt(name: &str) -> Option<McpPrompt> {
    if let Ok(registry) = get_registry().read() {
        registry.get_prompt(name).cloned()
    } else {
        error!("Failed to acquire read lock on MCP registry");
        None
    }
}

/// Execute an MCP prompt by calling its JavaScript handler
pub fn execute_mcp_prompt(
    prompt_name: &str,
    arguments: serde_json::Value,
    auth_context: Option<crate::auth::JsAuthContext>,
    user_context: crate::security::UserContext,
    exchange: crate::mcp_elicitation::Exchange,
) -> Result<PromptOutcome, String> {
    debug!(
        "Executing MCP prompt: {} with args: {:?}",
        prompt_name, arguments
    );

    // Get the prompt from registry
    let registry_arc = get_registry();
    let (script_uri, handler_function) = {
        let registry = registry_arc
            .read()
            .map_err(|e| format!("Failed to read MCP registry: {}", e))?;

        let prompt = registry
            .get_prompt(prompt_name)
            .ok_or_else(|| format!("Prompt '{}' not found", prompt_name))?;

        (prompt.script_uri.clone(), prompt.handler_function.clone())
    };

    // Build context for handler - all arguments provided, full prompt mode
    let context = serde_json::json!({
        "mode": "prompt",
        "arguments": arguments
    });

    // Execute the JavaScript handler
    let outcome = crate::js_engine::execute_mcp_prompt_handler(
        &script_uri,
        &handler_function,
        context,
        auth_context,
        user_context,
        exchange,
    )?;

    debug!("MCP prompt '{}' executed", prompt_name);
    Ok(outcome)
}

/// Execute an MCP completion by calling the prompt's JavaScript handler in completion mode
pub fn execute_mcp_completion(
    prompt_name: &str,
    argument_name: &str,
    argument_value: &str,
    context_arguments: Option<serde_json::Value>,
    auth_context: Option<crate::auth::JsAuthContext>,
    user_context: crate::security::UserContext,
) -> Result<serde_json::Value, String> {
    debug!(
        "Executing MCP completion for prompt: {}, argument: {}, value: '{}'",
        prompt_name, argument_name, argument_value
    );

    // Get the prompt from registry
    let registry_arc = get_registry();
    let (script_uri, handler_function) = {
        let registry = registry_arc
            .read()
            .map_err(|e| format!("Failed to read MCP registry: {}", e))?;

        let prompt = registry
            .get_prompt(prompt_name)
            .ok_or_else(|| format!("Prompt '{}' not found", prompt_name))?;

        (prompt.script_uri.clone(), prompt.handler_function.clone())
    };

    // Build context for handler - completion mode with partial arguments
    let context = serde_json::json!({
        "mode": "completion",
        "completingArgument": argument_name,
        "partialValue": argument_value,
        "arguments": context_arguments.unwrap_or(serde_json::json!({}))
    });

    // Execute the JavaScript handler in completion mode
    // Completion runs the same handler in a different mode, and deliberately
    // unattended: `completion/complete` is an autocomplete on a half-typed
    // argument, and the specification does not permit `input_required` on it.
    let outcome = crate::js_engine::execute_mcp_prompt_handler(
        &script_uri,
        &handler_function,
        context,
        auth_context,
        user_context,
        crate::mcp_elicitation::Exchange::unattended(),
    )?;

    match outcome {
        Outcome::Complete(result) => {
            debug!(
                "MCP completion for prompt '{}' executed successfully",
                prompt_name
            );
            Ok(result)
        }
        // Unreachable by construction — the exchange above says nobody can be
        // asked, so `mcp.ask` throws rather than recording anything — but the
        // handler is shared with `prompts/get` and saying so beats a panic if
        // that ever stops being true.
        Outcome::InputRequired(_) => Err("a completion handler cannot ask for input".to_string()),
    }
}

/// Wall-clock slack the dispatcher's backstop allows on top of whatever ceiling
/// a tool enforces for itself.
///
/// The tool's own limit is the better stop — it returns a result, where the
/// backstop can only abandon the call — so it gets first refusal and this
/// covers only what it cannot: JavaScript blocked in a *host* call, where the
/// interrupt handler never runs because no bytecode is executing.
const TOOL_CALL_GRACE_MS: u64 = 5_000;

/// How long the dispatcher waits for one tool call before giving up on it.
///
/// Per tool rather than one ceiling for all of them: a script-registered tool
/// is bounded by the JavaScript execution budget, and holding a request open
/// for the sixty seconds a full test run may need would be the wrong answer for
/// a handler whose own budget is two.
pub fn tool_call_backstop_ms(tool_name: &str) -> u64 {
    crate::engine_api::native_tool_ceiling_ms(tool_name)
        .unwrap_or_else(|| crate::js_engine::current_execution_limits().timeout_ms)
        .saturating_add(TOOL_CALL_GRACE_MS)
}

/// Execute an MCP tool by calling its JavaScript handler
/// What a call produced: an answer, or a question for the caller.
///
/// A handler that asks produces no result at all — the specification's Multi
/// Round-Trip Requests pattern ends the call, and the client starts a fresh one
/// carrying the answer. So this is an enum rather than a result with an
/// optional field: the two outcomes have nothing in common to merge.
///
/// Generic because the specification permits `input_required` on `tools/call`,
/// `prompts/get` and `resources/read`, and the engine serves the first two.
/// They differ only in what a *finished* call hands back.
pub enum Outcome<T> {
    /// The handler returned.
    Complete(T),
    /// The handler asked for something before it could finish.
    InputRequired(crate::mcp_elicitation::Asked),
}

/// A tool's result is its handler's JSON, still a string.
pub type ToolOutcome = Outcome<String>;

/// A prompt's result is parsed, because the engine reads its `messages`.
pub type PromptOutcome = Outcome<serde_json::Value>;

pub fn execute_mcp_tool(
    tool_name: &str,
    arguments: serde_json::Value,
    auth_context: Option<crate::auth::JsAuthContext>,
    user_context: crate::security::UserContext,
    exchange: crate::mcp_elicitation::Exchange,
) -> Result<ToolOutcome, String> {
    debug!(
        "Executing MCP tool: {} with args: {:?}",
        tool_name, arguments
    );

    // Native engine tools take precedence over script-registered tools. None of
    // them elicits: they are the engine's own management surface, called by an
    // agent against an account that already holds the rights, so there is never
    // a question to put to a person mid-call.
    if let Some(result) =
        crate::engine_api::execute_native_mcp_tool(tool_name, &arguments, &user_context)
    {
        return Ok(ToolOutcome::Complete(result.to_string()));
    }

    // Get the tool from registry
    let registry_arc = get_registry();
    let (script_uri, handler_function) = {
        let registry = registry_arc
            .read()
            .map_err(|e| format!("Failed to read MCP registry: {}", e))?;

        let tool = registry
            .get_tool(tool_name)
            .ok_or_else(|| format!("Tool '{}' not found", tool_name))?;

        (tool.script_uri.clone(), tool.handler_function.clone())
    };

    // Execute the JavaScript handler
    crate::js_engine::execute_mcp_tool_handler(
        &script_uri,
        &handler_function,
        tool_name,
        arguments,
        auth_context,
        user_context,
        exchange,
    )
    .map_err(|e| format!("Tool execution failed: {}", e))
}

/// Versions reachable through the `initialize` handshake, newest first.
///
/// The specification calls these **legacy**: a client opens with `initialize`,
/// the server answers with one version, and everything after that is scoped to
/// the session that established. `2025-11-25` is the last of them.
pub const LEGACY_PROTOCOL_VERSIONS: &[&str] =
    &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// Versions reachable statelessly, each request naming its own, newest first.
///
/// The specification calls these **modern**. `2026-07-28` removed the
/// `initialize`/`notifications/initialized` handshake and the `Mcp-Session-Id`
/// header outright: every request carries its protocol version and the client's
/// capabilities in [`META_PROTOCOL_VERSION`] and [`META_CLIENT_CAPABILITIES`],
/// and a server answers each one without reference to any that came before.
///
/// That is the shape `/mcp` already had. The engine has no session header, its
/// route index is keyed `(host, path, method)`, and instances stay in step over
/// LISTEN/NOTIFY — a request could always land on any of them. What the
/// revision took away is something this engine never had.
pub const MODERN_PROTOCOL_VERSIONS: &[&str] = &["2026-07-28"];

/// Every version this engine's MCP server implements, newest first.
///
/// Modern first, then legacy, which is also newest-first overall — a test holds
/// that, because the fallbacks below read the ends of the sub-lists and a
/// version appearing in the wrong one would be offered over a transport that
/// cannot carry it.
///
/// The server used to answer `2024-11-05` unconditionally and drop whatever the
/// client had asked for, which is not a negotiation — a client speaking a later
/// revision was told the server only knew the first one, and the engine's own
/// MCP *client* meanwhile speaks [`crate::mcp_client`]'s version, so the two
/// halves of the same codebase disagreed about what year it was.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &[
    "2026-07-28",
    "2025-11-25",
    "2025-06-18",
    "2025-03-26",
    "2024-11-05",
];

/// `_meta` key carrying the protocol version of a single request. Required on
/// every modern request.
pub const META_PROTOCOL_VERSION: &str = "io.modelcontextprotocol/protocolVersion";

/// `_meta` key carrying the client's capabilities. Required on every modern
/// request: a server **MUST NOT** rely on a capability the client did not name.
pub const META_CLIENT_CAPABILITIES: &str = "io.modelcontextprotocol/clientCapabilities";

/// `_meta` key carrying the client's name and version. Optional, and
/// self-reported — for display and logs, never for a decision.
pub const META_CLIENT_INFO: &str = "io.modelcontextprotocol/clientInfo";

/// `_meta` key the server identifies itself under on every result, which is how
/// a client learns who answered without a handshake to have learned it from.
pub const META_SERVER_INFO: &str = "io.modelcontextprotocol/serverInfo";

/// The `Mcp-Method` header names the method in the body, so a gateway can route
/// and meter without parsing JSON.
pub const HEADER_MCP_METHOD: &str = "mcp-method";

/// `HeaderMismatch`: a routing header disagrees with the body it travelled
/// with. In the reserved range, so it means only this.
pub const ERROR_HEADER_MISMATCH: i32 = -32020;

/// `UnsupportedProtocolVersion`: the request named a version this server does
/// not implement. Its `data` carries what we do, so one round trip is enough
/// for the client to choose again.
pub const ERROR_UNSUPPORTED_PROTOCOL_VERSION: i32 = -32022;

/// How long a client may reuse a list result before asking again.
///
/// This is the honest form of what `listChanged` was claiming. A script
/// registers its tools when it runs, so the list genuinely changes underneath a
/// client — but the notification saying so travels server-to-client, and a POST
/// response has nothing to carry it. A promise the engine cannot keep made a
/// conforming client cache forever; a freshness hint makes it come back.
///
/// Sixty seconds is short enough that a newly deployed tool shows up while
/// somebody is still looking for it, and long enough to spare the engine a
/// `tools/list` per turn.
pub const LIST_CACHE_TTL_MS: u64 = 60_000;

/// Which caches may hold a list result.
///
/// `private` rather than `public`, and not as a default: [`list_tools_for_host`]
/// filters by host *and* by whether the engine's own tools are allowed on it
/// (`server.management_hosts`), so two callers genuinely do not see the same
/// list. A shared intermediary treating one answer as everyone's would hand a
/// script host the management tools.
pub const LIST_CACHE_SCOPE: &str = "private";

/// Pick the protocol version to answer an `initialize` with.
///
/// The rule the specification states: answer with the client's own version if
/// it is one we speak, and otherwise with the newest we do, leaving the client
/// to decide whether it can continue. A client that names nothing predates the
/// field, so it gets the oldest revision on the list rather than the newest —
/// guessing high at something that did not say is how a client ends up sent
/// capabilities it has no parser for.
///
/// This reads [`LEGACY_PROTOCOL_VERSIONS`] and not [`SUPPORTED_PROTOCOL_VERSIONS`],
/// which is the whole subtlety: a client that sent `initialize` cannot be
/// speaking a modern revision, because modern revisions have no `initialize`.
/// Answering one here would name a version whose rules neither side is
/// following.
pub fn negotiate_protocol_version(requested: Option<&str>) -> &'static str {
    let newest = || {
        LEGACY_PROTOCOL_VERSIONS
            .first()
            .copied()
            .unwrap_or("2024-11-05")
    };

    let Some(requested) = requested else {
        return LEGACY_PROTOCOL_VERSIONS
            .last()
            .copied()
            .unwrap_or("2024-11-05");
    };

    LEGACY_PROTOCOL_VERSIONS
        .iter()
        .find(|supported| **supported == requested)
        .copied()
        .unwrap_or_else(newest)
}

/// Which era a request is speaking, decided by the request alone.
///
/// A dual-era server picks its behaviour from how the client opens, and the
/// specification says what to look at: a request carrying modern per-request
/// `_meta` is served statelessly, an `initialize` selects legacy semantics.
/// Nothing here consults connection state, because there is none to consult.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Era {
    /// The request named a protocol version in its `_meta`.
    Modern { version: String },
    /// No per-request version: either `initialize` itself, or a call made
    /// inside a session an `initialize` established.
    Legacy,
}

/// Read a request's `_meta` field, wherever `params` happens to be absent.
fn request_meta(params: Option<&serde_json::Value>) -> Option<&serde_json::Value> {
    params?.get("_meta")
}

/// Classify a request by the presence of a per-request protocol version.
///
/// Deliberately not by method name. A modern client calls `tools/list` exactly
/// as a legacy one does, and the only thing separating them is the `_meta` the
/// modern one carries — which is the point of the revision: the request says
/// what it is, rather than the server remembering.
pub fn classify_era(params: Option<&serde_json::Value>) -> Era {
    match request_meta(params)
        .and_then(|meta| meta.get(META_PROTOCOL_VERSION))
        .and_then(|version| version.as_str())
    {
        Some(version) => Era::Modern {
            version: version.to_string(),
        },
        None => Era::Legacy,
    }
}

/// Whether a modern request named a version this server implements.
pub fn supports_modern_version(version: &str) -> bool {
    MODERN_PROTOCOL_VERSIONS.contains(&version)
}

/// Whether a modern request declared the client capabilities it must.
///
/// The value is not inspected — an empty object is a complete declaration,
/// meaning "I have none of the optional client features". What matters is that
/// the client said so, because a server **MUST NOT** rely on a capability that
/// was never declared, and absence and "declared empty" are different claims.
pub fn declares_client_capabilities(params: Option<&serde_json::Value>) -> bool {
    request_meta(params)
        .and_then(|meta| meta.get(META_CLIENT_CAPABILITIES))
        .is_some()
}

/// The client's self-reported name, for logs. Never a decision input.
pub fn client_name(params: Option<&serde_json::Value>) -> Option<String> {
    request_meta(params)?
        .get(META_CLIENT_INFO)?
        .get("name")?
        .as_str()
        .map(str::to_string)
}

/// How the engine names itself, on `server/discover` and in every result's
/// `_meta`.
pub fn server_info() -> serde_json::Value {
    serde_json::json!({
        "name": "aiwebengine",
        "version": env!("CARGO_PKG_VERSION"),
    })
}

/// Stamp a result object as a finished answer.
///
/// Every result in the modern revision carries a `resultType`, and `"complete"`
/// is the ordinary one — `"input_required"` is the other, which is what an MRTR
/// elicitation will return once the engine has one. Applied to legacy answers
/// too, deliberately: a `Result` has always been an open map (`[key: string]:
/// unknown`), so the extra field is allowed in every revision the engine
/// speaks, and one code path is worth more here than a saved field. Clients on
/// older revisions are required to read its absence as `"complete"` anyway, so
/// nothing can misread its presence.
///
/// `_meta.serverInfo` rides along for the same reason it exists: with no
/// handshake there is nowhere else for a client to learn who answered.
pub fn complete(result: serde_json::Value) -> serde_json::Value {
    let mut result = result;
    if let Some(object) = result.as_object_mut() {
        object
            .entry("resultType")
            .or_insert_with(|| serde_json::json!("complete"));
        let meta = object
            .entry("_meta")
            .or_insert_with(|| serde_json::json!({}));
        if let Some(meta) = meta.as_object_mut() {
            meta.entry(META_SERVER_INFO).or_insert_with(server_info);
        }
    }
    result
}

/// The `server/discover` result: what we speak, what we can do, who we are.
///
/// `supportedVersions` names the modern revisions only. The engine answers
/// `initialize` too and will go on doing so, but a version a client would put
/// in `_meta` has to be one whose rules `_meta` is part of; offering
/// `2025-11-25` here would invite a client to name a revision that has no
/// per-request metadata and then send it some.
pub fn discover_result(native_tools_allowed: bool) -> serde_json::Value {
    complete(serde_json::json!({
        "supportedVersions": MODERN_PROTOCOL_VERSIONS,
        "capabilities": {
            "tools": {},
            "prompts": {},
            // Advertised unconditionally rather than only when a script has
            // registered one: the registry is filled at runtime by whatever is
            // deployed, so "no resources right now" is a listing that comes
            // back empty, not a server that cannot serve them.
            "resources": {},
            "completions": {},
        },
        "instructions": if native_tools_allowed {
            "Scripts hosted by this engine register tools, prompts and resources at \
             runtime, and this host also exposes the engine's own management tools. \
             Call tools/list rather than caching a list across deployments; results \
             carry a ttlMs saying how long they stay good."
        } else {
            "Scripts hosted by this engine register tools, prompts and resources at \
             runtime. Call tools/list rather than caching a list across deployments; \
             results carry a ttlMs saying how long they stay good."
        },
        "ttlMs": LIST_CACHE_TTL_MS,
        "cacheScope": LIST_CACHE_SCOPE,
    }))
}

/// The refusal a modern request naming an unimplemented version gets.
///
/// Carries what we do implement, so the client chooses again from fact rather
/// than probing. This is also the error that identifies the engine as a modern
/// server to a dual-era client deciding whether to fall back to `initialize`.
pub fn unsupported_version_error(
    id: Option<serde_json::Value>,
    requested: &str,
) -> serde_json::Value {
    serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": ERROR_UNSUPPORTED_PROTOCOL_VERSION,
            "message": "Unsupported protocol version",
            "data": {
                "supported": MODERN_PROTOCOL_VERSIONS,
                "requested": requested,
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_legacy_version_we_speak_is_answered_with_itself() {
        for version in LEGACY_PROTOCOL_VERSIONS {
            assert_eq!(
                negotiate_protocol_version(Some(version)),
                *version,
                "a client that names a version we support must hear it back"
            );
        }
    }

    #[test]
    fn a_version_we_do_not_speak_falls_back_to_the_newest_legacy_one() {
        assert_eq!(
            negotiate_protocol_version(Some("2099-01-01")),
            LEGACY_PROTOCOL_VERSIONS[0],
            "a client from the future is told the most recent thing we know"
        );
        assert_eq!(negotiate_protocol_version(Some("")), "2025-11-25");
    }

    #[test]
    fn initialize_never_answers_with_a_modern_version() {
        // The subtlety worth a test of its own: a client that sent
        // `initialize` cannot be speaking a revision that deleted it, so
        // naming one back would agree on rules neither side is following.
        for version in MODERN_PROTOCOL_VERSIONS {
            let answered = negotiate_protocol_version(Some(version));
            assert!(
                LEGACY_PROTOCOL_VERSIONS.contains(&answered),
                "initialize answered {answered}, which has no initialize"
            );
        }
        assert!(LEGACY_PROTOCOL_VERSIONS.contains(&negotiate_protocol_version(None)));
    }

    #[test]
    fn a_client_that_names_nothing_gets_the_oldest() {
        assert_eq!(
            negotiate_protocol_version(None),
            "2024-11-05",
            "omitting the field predates it, so answer the revision that predates it too"
        );
    }

    #[test]
    fn the_lists_are_ordered_newest_first_and_do_not_overlap() {
        for list in [
            SUPPORTED_PROTOCOL_VERSIONS,
            MODERN_PROTOCOL_VERSIONS,
            LEGACY_PROTOCOL_VERSIONS,
        ] {
            let mut sorted = list.to_vec();
            sorted.sort_unstable();
            sorted.reverse();
            assert_eq!(
                sorted, list,
                "the fallbacks read the ends of these lists, so their order is load-bearing"
            );
            let mut unique = list.to_vec();
            unique.dedup();
            assert_eq!(unique.len(), list.len(), "a version listed twice");
        }

        // The whole is exactly the two halves, so a version added to one of
        // them cannot go missing from what the engine claims to speak.
        let halves: Vec<&str> = MODERN_PROTOCOL_VERSIONS
            .iter()
            .chain(LEGACY_PROTOCOL_VERSIONS.iter())
            .copied()
            .collect();
        assert_eq!(
            halves, SUPPORTED_PROTOCOL_VERSIONS,
            "every supported version belongs to exactly one era"
        );
    }

    /// A request is modern because it says so, not because of its method.
    #[test]
    fn an_era_is_read_from_the_request_rather_than_the_method() {
        let modern = serde_json::json!({
            "_meta": {
                META_PROTOCOL_VERSION: "2026-07-28",
                META_CLIENT_CAPABILITIES: {}
            }
        });
        assert_eq!(
            classify_era(Some(&modern)),
            Era::Modern {
                version: "2026-07-28".to_string()
            }
        );
        assert!(declares_client_capabilities(Some(&modern)));

        // The same method with no per-request version is a call inside a
        // session some `initialize` established.
        let legacy = serde_json::json!({ "name": "some_tool" });
        assert_eq!(classify_era(Some(&legacy)), Era::Legacy);
        assert_eq!(classify_era(None), Era::Legacy);
    }

    #[test]
    fn capabilities_declared_empty_are_declared() {
        // "I have none" and "I did not say" are different claims, and only the
        // second is malformed — so the check is for the field, not its content.
        let said_none = serde_json::json!({
            "_meta": { META_PROTOCOL_VERSION: "2026-07-28", META_CLIENT_CAPABILITIES: {} }
        });
        let said_nothing = serde_json::json!({
            "_meta": { META_PROTOCOL_VERSION: "2026-07-28" }
        });
        assert!(declares_client_capabilities(Some(&said_none)));
        assert!(!declares_client_capabilities(Some(&said_nothing)));
    }

    #[test]
    fn only_modern_versions_are_offered_to_a_modern_client() {
        assert!(supports_modern_version("2026-07-28"));
        for legacy in LEGACY_PROTOCOL_VERSIONS {
            assert!(
                !supports_modern_version(legacy),
                "{legacy} has no per-request metadata, so it cannot be named in some"
            );
        }

        let refusal = unsupported_version_error(Some(serde_json::json!(1)), "1900-01-01");
        assert_eq!(refusal["error"]["code"], ERROR_UNSUPPORTED_PROTOCOL_VERSION);
        assert_eq!(refusal["error"]["data"]["requested"], "1900-01-01");
        assert_eq!(
            refusal["error"]["data"]["supported"],
            serde_json::json!(MODERN_PROTOCOL_VERSIONS),
            "the refusal has to carry enough for the client to choose again"
        );
    }

    #[test]
    fn a_result_is_stamped_complete_and_says_who_answered() {
        let stamped = complete(serde_json::json!({ "tools": [] }));
        assert_eq!(stamped["resultType"], "complete");
        assert_eq!(stamped["_meta"][META_SERVER_INFO]["name"], "aiwebengine");
        assert_eq!(stamped["tools"], serde_json::json!([]));
    }

    #[test]
    fn stamping_never_overwrites_what_a_handler_already_said() {
        // The MRTR arm to come returns `input_required`, and a handler that
        // set its own `_meta` means it. Neither may be clobbered on the way out.
        let interim = complete(serde_json::json!({
            "resultType": "input_required",
            "_meta": { "com.example/trace": "abc" }
        }));
        assert_eq!(interim["resultType"], "input_required");
        assert_eq!(interim["_meta"]["com.example/trace"], "abc");
        assert_eq!(
            interim["_meta"][META_SERVER_INFO]["name"], "aiwebengine",
            "and the server still identifies itself alongside"
        );
    }

    #[test]
    fn discover_answers_what_a_client_needs_before_anything_else() {
        let discovered = discover_result(true);
        assert_eq!(discovered["resultType"], "complete");
        assert_eq!(
            discovered["supportedVersions"],
            serde_json::json!(MODERN_PROTOCOL_VERSIONS)
        );
        assert!(discovered["capabilities"]["tools"].is_object());
        assert_eq!(discovered["_meta"][META_SERVER_INFO]["name"], "aiwebengine");
        assert_eq!(discovered["cacheScope"], LIST_CACHE_SCOPE);

        // A host that does not carry the engine's own tools must not be
        // described as though it did.
        let scripts_only = discover_result(false);
        let described = scripts_only["instructions"].as_str().unwrap_or_default();
        assert!(
            !described.contains("management tools"),
            "instructions described tools this host does not serve: {described}"
        );
    }
}
