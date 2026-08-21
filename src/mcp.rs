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

/// Registry for storing MCP tools and prompts registered from JavaScript
#[derive(Debug, Clone, Default)]
pub struct McpRegistry {
    /// Registered tools (key: tool name, value: tool definition)
    pub tools: HashMap<String, McpTool>,
    /// Registered prompts (key: prompt name, value: prompt definition)
    pub prompts: HashMap<String, McpPrompt>,
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

    tools
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
        .collect()
}

/// The prompts a client connecting on `host` should see. Filtered like
/// [`list_tools_for_host`].
pub async fn list_prompts_for_host(host: &str) -> Vec<McpPrompt> {
    let prompts = list_prompts();
    let Some(allowed) = crate::route_index::scripts_for_host(host).await else {
        return prompts;
    };
    prompts
        .into_iter()
        .filter(|prompt| allowed.contains(&prompt.script_uri))
        .collect()
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
) -> Result<serde_json::Value, String> {
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
    let result =
        crate::js_engine::execute_mcp_prompt_handler(&script_uri, &handler_function, context)?;

    debug!("MCP prompt '{}' executed successfully", prompt_name);
    Ok(result)
}

/// Execute an MCP completion by calling the prompt's JavaScript handler in completion mode
pub fn execute_mcp_completion(
    prompt_name: &str,
    argument_name: &str,
    argument_value: &str,
    context_arguments: Option<serde_json::Value>,
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
    let result =
        crate::js_engine::execute_mcp_prompt_handler(&script_uri, &handler_function, context)?;

    debug!(
        "MCP completion for prompt '{}' executed successfully",
        prompt_name
    );
    Ok(result)
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
pub fn execute_mcp_tool(
    tool_name: &str,
    arguments: serde_json::Value,
    auth_context: Option<crate::auth::JsAuthContext>,
    user_context: crate::security::UserContext,
) -> Result<serde_json::Value, String> {
    debug!(
        "Executing MCP tool: {} with args: {:?}",
        tool_name, arguments
    );

    // Native engine tools take precedence over script-registered tools
    if let Some(result) =
        crate::engine_api::execute_native_mcp_tool(tool_name, &arguments, &user_context)
    {
        return Ok(result);
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
    let result = crate::js_engine::execute_mcp_tool_handler(
        &script_uri,
        &handler_function,
        tool_name,
        arguments,
        auth_context,
        user_context,
    )
    .map_err(|e| format!("Tool execution failed: {}", e))?;

    // Parse the result as JSON
    serde_json::from_str(&result).map_err(|e| format!("Failed to parse tool result as JSON: {}", e))
}
