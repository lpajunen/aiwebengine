use base64::Engine;
use chrono::Duration as ChronoDuration;
use rquickjs::{Function, Result as JsResult, function::Opt};
use std::collections::HashMap;
use tracing::{debug, error, warn};

/// The JavaScript half of `fetch()`: wraps the Rust call's JSON envelope in a
/// response that can be awaited, read as an object, or parsed as a string.
const FETCH_PRELUDE: &str = include_str!("../../assets/fetch_prelude.js");

/// Gives the host namespaces that answer with a JSON string the same shape a
/// `fetch` response has.
const RESULT_PRELUDE: &str = include_str!("../../assets/result_prelude.js");

/// The JavaScript half of `console`: joins a variadic call, fills in format
/// specifiers and renders values, so the host binding — which takes one string
/// and throws on anything else — is handed something it accepts.
const CONSOLE_PRELUDE: &str = include_str!("../../assets/console_prelude.js");

/// Builds the Web Storage interface — `length`, `key(i)`, named access, and
/// failures that throw rather than being returned — over the two host stores.
const STORAGE_PRELUDE: &str = include_str!("../../assets/storage_prelude.js");

/// `Headers`, `URLSearchParams`, and the methods `context.request` gains so a
/// body a script receives reads the way a body it fetched does.
const REQUEST_PRELUDE: &str = include_str!("../../assets/request_prelude.js");

use crate::repository;
use crate::scheduler;
use crate::security::{
    SecureOperations, SecurityAuditor, SecurityEventType, SecuritySeverity, UserContext,
};

// Type alias for route registration callback function
type RouteRegisterFn =
    Box<dyn Fn(&str, &repository::RouteMetadata, Option<&str>) -> Result<(), rquickjs::Error>>;

/// Which registry a [`CollectedRegistration`] would have been written to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RegistrationKind {
    Route,
    Stream,
    AssetRoute,
    GraphqlQuery,
    GraphqlMutation,
    GraphqlSubscription,
    McpTool,
    McpPrompt,
    ScheduledJob,
    MessageListener,
}

impl RegistrationKind {
    /// The registering API's name, for messages that have to say what was
    /// skipped.
    pub fn api(self) -> &'static str {
        match self {
            RegistrationKind::Route => "routeRegistry.registerRoute",
            RegistrationKind::Stream => "routeRegistry.registerStreamRoute",
            RegistrationKind::AssetRoute => "routeRegistry.registerAssetRoute",
            RegistrationKind::GraphqlQuery => "graphQLRegistry.registerQuery",
            RegistrationKind::GraphqlMutation => "graphQLRegistry.registerMutation",
            RegistrationKind::GraphqlSubscription => "graphQLRegistry.registerSubscription",
            RegistrationKind::McpTool => "mcpRegistry.registerTool",
            RegistrationKind::McpPrompt => "mcpRegistry.registerPrompt",
            RegistrationKind::ScheduledJob => "schedulerService",
            RegistrationKind::MessageListener => "dispatcher.registerListener",
        }
    }
}

/// One registration a script made, recorded instead of applied.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectedRegistration {
    pub kind: RegistrationKind,
    /// What the registration is keyed by: a path, an operation name, a tool
    /// name, a message type, or a scheduled job's key.
    pub name: String,
    /// HTTP method, for the registrations that have one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// The script function the engine would call. `None` where a registration
    /// names no delegate — an asset route serves bytes, not code, and a stream
    /// without a customization function has nothing to call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handler: Option<String>,
}

impl CollectedRegistration {
    pub fn new(kind: RegistrationKind, name: impl Into<String>) -> Self {
        Self {
            kind,
            name: name.into(),
            method: None,
            handler: None,
        }
    }

    pub fn with_method(mut self, method: impl Into<String>) -> Self {
        self.method = Some(method.into());
        self
    }

    pub fn with_handler(mut self, handler: impl Into<String>) -> Self {
        self.handler = Some(handler.into());
        self
    }
}

/// Where a dry run's registrations accumulate.
///
/// `Arc<Mutex<_>>` rather than `Rc<RefCell<_>>` because it is held in
/// [`GlobalSecurityConfig`], which callers build outside the JavaScript context
/// and move in; the contention is nil either way, since only the one QuickJS
/// thread ever touches it.
pub type RegistrationSink = std::sync::Arc<std::sync::Mutex<Vec<CollectedRegistration>>>;

/// One line a script wrote through `console`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConsoleLine {
    /// `LOG`, `INFO`, `WARN`, `ERROR` or `DEBUG` — the level the `console`
    /// method maps to, unchanged.
    pub level: String,
    pub message: String,
    /// Milliseconds since the epoch, so an interleaved read stays ordered even
    /// when the caller merges several runs.
    pub timestamp_ms: u64,
}

/// Captured `console` output and what did not fit.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ConsoleCapture {
    pub lines: Vec<ConsoleLine>,
    /// Lines dropped once [`MAX_CAPTURED_CONSOLE_LINES`] was reached. Counted
    /// rather than inferred, so a caller can tell a capture that happens to sit
    /// exactly on the cap from one that was truncated.
    pub dropped: usize,
}

/// Where captured `console` output accumulates. See
/// [`GlobalSecurityConfig::console_sink`].
pub type ConsoleSink = std::sync::Arc<std::sync::Mutex<ConsoleCapture>>;

/// Cap on captured lines, so a snippet that logs in a loop cannot grow the
/// response without bound. Lines past the cap are dropped and the caller is
/// told how many.
pub const MAX_CAPTURED_CONSOLE_LINES: usize = 1_000;

fn parse_filter_match_mode(
    match_mode: Option<String>,
) -> JsResult<crate::stream_registry::FilterMatchMode> {
    match match_mode {
        Some(raw_mode) => raw_mode.parse().map_err(|err: String| {
            rquickjs::Error::new_from_js_message("matchMode", "FilterMatchMode", &err)
        }),
        None => Ok(crate::stream_registry::FilterMatchMode::Subset),
    }
}

/// Extract optional OpenAPI documentation metadata (`tags`, `summary`,
/// `description`) from the `metadata` object accepted by
/// `registerAssetRoute`/`registerStreamRoute`. Returns `(tags, summary,
/// description)`; missing fields yield an empty vector / `None`, so callers
/// fall back to their default Swagger group and auto-generated text.
fn extract_route_metadata(
    metadata: Option<&rquickjs::Object<'_>>,
) -> (Vec<String>, Option<String>, Option<String>) {
    let mut tags = Vec::new();
    let mut summary = None;
    let mut description = None;
    if let Some(meta) = metadata {
        if let Ok(tags_arr) = meta.get::<_, rquickjs::Array>("tags") {
            for i in 0..tags_arr.len() {
                if let Ok(tag) = tags_arr.get::<String>(i) {
                    tags.push(tag);
                }
            }
        }
        if let Ok(value) = meta.get::<_, Option<String>>("summary") {
            summary = value;
        }
        if let Ok(value) = meta.get::<_, Option<String>>("description") {
            description = value;
        }
    }
    (tags, summary, description)
}

/// Secure wrapper for JavaScript global functions that enforces Rust-level validation
pub struct SecureGlobalContext {
    user_context: UserContext,
    secure_ops: SecureOperations,
    auditor: SecurityAuditor,
    config: GlobalSecurityConfig,
}

/// Controls the parts of the JavaScript API whose behaviour depends on *when* a
/// script runs rather than on who is calling it.
///
/// Every global and every method is installed in every context, so a script
/// never has to feature-detect. These flags only decide whether a registration
/// call takes effect.
#[derive(Debug, Clone)]
pub struct GlobalSecurityConfig {
    /// True only while a script's registrations are being collected: engine
    /// startup and the `init()` call that follows it.
    ///
    /// A script's top-level program is re-evaluated on *every* invocation, so
    /// registration calls written at top level run again on each request. In
    /// the registration phase they take effect; everywhere else they are
    /// no-ops that report what happened. They must not throw — that would
    /// break every script that registers at top level rather than in `init()`.
    pub registration_phase: bool,
    /// Disabled where there is no Tokio runtime to spawn the audit writer onto.
    pub enable_audit_logging: bool,
    /// When set, registration calls are validated as usual and then *recorded
    /// here* instead of reaching the engine's live registries.
    ///
    /// This is what makes `/engine/check` safe to run against a deployed
    /// script. Only `registerRoute` collects by design — every other registry
    /// (GraphQL, streams, asset routes, MCP, scheduler, dispatcher) is a
    /// process-wide singleton written to directly, so a candidate's `init()`
    /// would otherwise replace the deployed script's resolvers, listeners and
    /// jobs with its own, and a broken candidate would take the live script
    /// down with it. Nothing undoes those writes afterwards, which is why the
    /// test runner opts out of the registration phase entirely
    /// (`registration_phase: false`) rather than isolating it.
    ///
    /// Set only together with `registration_phase: true`: the phase check runs
    /// first, so a sink on an inactive context would never be reached.
    pub dry_run_sink: Option<RegistrationSink>,
    /// When set, `console` output is captured here as well as written to the
    /// script's log.
    ///
    /// Capture is what makes `/engine/eval` usable, not a convenience on top of
    /// it: `console` writes go through the repository, so they join whatever
    /// transaction is open — and an evaluation that rolls back would otherwise
    /// roll back its own output, losing exactly what the caller asked for.
    pub console_sink: Option<ConsoleSink>,
    /// Which invocation the script's `console` output is attributed to.
    ///
    /// Empty for contexts with no invocation to name; a line written under an
    /// empty context is stored exactly as it was before this existed.
    pub log_context: repository::LogContext,
}

impl Default for GlobalSecurityConfig {
    fn default() -> Self {
        Self {
            // Fail closed: a caller that does not opt in cannot mutate
            // registries that outlive its own invocation.
            registration_phase: false,
            enable_audit_logging: true,
            dry_run_sink: None,
            console_sink: None,
            log_context: repository::LogContext::default(),
        }
    }
}

impl GlobalSecurityConfig {
    /// Record `registration` and return the reply to give JavaScript, or `None`
    /// when this context registers for real and the caller should carry on to
    /// the live registry.
    ///
    /// Call this *after* the validation and capability checks of the API it
    /// guards, so a dry run reports the same refusals a real registration
    /// would, and immediately before the registry write, so nothing that
    /// outlives the run has happened yet.
    fn collect(&self, registration: CollectedRegistration) -> Option<String> {
        let sink = self.dry_run_sink.as_ref()?;
        let reply = format!(
            "{}: '{}' checked but not registered - this is a dry run",
            registration.kind.api(),
            registration.name
        );
        if let Ok(mut collected) = sink.lock() {
            collected.push(registration);
        }
        Some(reply)
    }

    /// True when registration calls are being recorded rather than applied.
    fn is_dry_run(&self) -> bool {
        self.dry_run_sink.is_some()
    }

    /// Record one `console` line if this context is capturing.
    fn capture_console(&self, level: &str, message: &str) {
        let Some(sink) = self.console_sink.as_ref() else {
            return;
        };
        let Ok(mut capture) = sink.lock() else {
            return;
        };
        if capture.lines.len() >= MAX_CAPTURED_CONSOLE_LINES {
            capture.dropped += 1;
            return;
        }
        capture.lines.push(ConsoleLine {
            level: level.to_string(),
            message: message.to_string(),
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|since| since.as_millis() as u64)
                .unwrap_or_default(),
        });
    }
}

/// Reply for a registration call made outside the registration phase.
///
/// Registration APIs stay callable everywhere so that top-level script code
/// keeps working, but only the registration phase writes to the registry.
fn registration_inactive(api: &str, name: &str) -> String {
    format!(
        "{}: '{}' not registered - registration only takes effect during script \
         startup and init()",
        api, name
    )
}

impl SecureGlobalContext {
    pub fn new(user_context: UserContext) -> Self {
        let pool = crate::database::get_global_database().map(|db| db.pool().clone());

        Self {
            user_context,
            secure_ops: SecureOperations::new(),
            auditor: SecurityAuditor::new(pool),
            config: GlobalSecurityConfig::default(),
        }
    }

    pub fn new_with_config(user_context: UserContext, config: GlobalSecurityConfig) -> Self {
        let pool = crate::database::get_global_database().map(|db| db.pool().clone());

        Self {
            user_context,
            secure_ops: SecureOperations::new(),
            auditor: SecurityAuditor::new(pool),
            config,
        }
    }

    /// Setup all secure global functions in the JavaScript context
    pub fn setup_secure_globals<'js>(
        &self,
        ctx: &'js rquickjs::Ctx<'js>,
        script_uri: &str,
    ) -> JsResult<()> {
        self.setup_secure_functions(ctx, script_uri, None)
    }

    /// Setup secure global functions with optional route registration function
    pub fn setup_secure_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
        register_fn: Option<RouteRegisterFn>,
    ) -> JsResult<()> {
        // Every global below is installed in every execution context. A script's
        // API surface must not depend on how it was entered: the same helper
        // may be reached from an HTTP handler, a scheduled job and a message
        // listener, and `typeof x === "undefined"` guards are not something
        // solution developers should have to write. Where an operation is
        // meaningless outside the registration phase, the method is still
        // present and still callable - see `registration_inactive`.
        self.setup_route_registry(ctx, script_uri, register_fn)?;
        self.setup_logging_functions(ctx, script_uri)?;
        self.setup_asset_management_functions(ctx, script_uri)?;
        self.setup_secrets_functions(ctx, script_uri)?;
        self.setup_fetch_function(ctx, script_uri)?;
        self.setup_database_functions(ctx, script_uri)?;
        self.setup_conversion_functions(ctx, script_uri)?;
        self.setup_script_properties_functions(ctx, script_uri)?;
        self.setup_user_properties_functions(ctx, script_uri)?;
        self.setup_graphql_functions(ctx, script_uri)?;
        self.setup_mcp_functions(ctx, script_uri)?;
        self.setup_scheduler_functions(ctx, script_uri)?;
        self.setup_dispatcher_functions(ctx, script_uri)?;

        // Setup JSX factory functions for server-side HTML generation
        self.setup_jsx_functions(ctx)?;

        Ok(())
    }

    /// Setup secure logging functions
    fn setup_logging_functions(&self, ctx: &rquickjs::Ctx<'_>, script_uri: &str) -> JsResult<()> {
        let global = ctx.globals();
        let user_context = self.user_context.clone();
        let auditor = self.auditor.clone();
        let script_uri_owned = script_uri.to_string();
        let config = self.config.clone();

        // Secure writeLog function
        let user_ctx_write = user_context.clone();
        let auditor_write = auditor.clone();
        let script_uri_write = script_uri_owned.clone();
        let config_write = config.clone();
        let write_log = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, message: String, level: String| -> JsResult<String> {
                // Check capability
                if let Err(e) =
                    user_ctx_write.require_capability(&crate::security::Capability::ViewLogs)
                {
                    if config_write.enable_audit_logging {
                        let rt = tokio::runtime::Handle::try_current();
                        if let Ok(_rt) = rt {
                            // Only attempt async logging if we're in a runtime
                            let auditor_clone = auditor_write.clone();
                            let user_id = user_ctx_write.user_id.clone();
                            tokio::spawn(async move {
                                let _ = auditor_clone
                                    .log_authz_failure(
                                        user_id,
                                        "log".to_string(),
                                        "write".to_string(),
                                        "ViewLogs".to_string(),
                                    )
                                    .await;
                            });
                        }
                    }
                    return Ok(format!("Error: {}", e));
                }

                // Log the write operation
                if config_write.enable_audit_logging {
                    let rt = tokio::runtime::Handle::try_current();
                    if let Ok(_rt) = rt {
                        let auditor_clone = auditor_write.clone();
                        let user_id = user_ctx_write.user_id.clone();
                        let script_uri_clone = script_uri_write.clone();
                        let message_len = message.len();
                        tokio::spawn(async move {
                            let _ = auditor_clone
                                .log_event(
                                    crate::security::SecurityEvent::new(
                                        SecurityEventType::SystemSecurityEvent,
                                        SecuritySeverity::Low,
                                        user_id,
                                    )
                                    .with_resource("log".to_string())
                                    .with_action("write".to_string())
                                    .with_detail("script_uri", &script_uri_clone)
                                    .with_detail("message_length", message_len.to_string()),
                                )
                                .await;
                        });
                    }
                }

                debug!(
                    script_uri = %script_uri_write,
                    user_id = ?user_ctx_write.user_id,
                    message_len = message.len(),
                    "Secure writeLog called"
                );

                // Capture before the repository write, and independently of it:
                // the write joins the caller's transaction and disappears with
                // it on a rollback, which is the case capture exists for.
                config_write.capture_console(&level, &message);

                // Call actual repository function
                repository::insert_log_message_in_context(
                    &script_uri_write,
                    &message,
                    &level,
                    &config_write.log_context,
                );
                Ok("Log written successfully".to_string())
            },
        )?;

        // The Rust half is installed under a private name, as `fetch` and
        // `database` install theirs. `console` itself is built by the prelude
        // below, which does the argument formatting this call cannot: it only
        // accepts a string, and a script logging an object or a number would
        // otherwise get a TypeError where it asked for a log line.
        global.set("__writeLog", write_log)?;

        // Compiled once per process and cached under a stable key, like the
        // other preludes. Installing it here covers every context that gets
        // host functions rather than each entry point remembering to do it.
        crate::bytecode::eval_program(ctx, "engine://console-prelude", CONSOLE_PRELUDE).map_err(
            |e| {
                rquickjs::Error::new_from_js_message(
                    "console",
                    "prelude",
                    &format!("console prelude failed to load: {}", e),
                )
            },
        )?;

        // `Headers` and `URLSearchParams` are ordinary globals, so they are
        // installed for every context rather than only where a request exists;
        // the request enhancement they back is applied when one is built.
        crate::bytecode::eval_program(ctx, "engine://request-prelude", REQUEST_PRELUDE).map_err(
            |e| {
                rquickjs::Error::new_from_js_message(
                    "request",
                    "prelude",
                    &format!("request prelude failed to load: {}", e),
                )
            },
        )?;

        Ok(())
    }

    /// Setup secure asset management functions
    fn setup_asset_management_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
    ) -> JsResult<()> {
        let global = ctx.globals();
        let user_context = self.user_context.clone();
        let secure_ops = self.secure_ops.clone();
        let auditor = self.auditor.clone();
        let script_uri_owned = script_uri.to_string();
        let script_uri_remaining = script_uri_owned.clone(); // Clone for remaining functions

        // Create assetStorage object
        let asset_storage = rquickjs::Object::new(ctx.clone())?;

        // Secure listAssets function
        let user_ctx_list = user_context.clone();
        let script_uri_list = script_uri_owned.clone();
        let list_assets = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
                // Check capability
                if let Err(_e) =
                    user_ctx_list.require_capability(&crate::security::Capability::ReadAssets)
                {
                    // Return empty array JSON if no permission
                    return Ok("[]".to_string());
                }

                debug!(
                    user_id = ?user_ctx_list.user_id,
                    "Secure listAssets called"
                );

                let assets = repository::fetch_assets(&script_uri_list);

                // Build JSON array of asset metadata (matching listScripts pattern)
                let assets_json: Vec<serde_json::Value> = assets
                    .values()
                    .map(|asset| {
                        serde_json::json!({
                            "uri": asset.uri,
                            "name": asset.name,
                            "size": asset.content.len(),
                            "mimetype": asset.mimetype,
                            "createdAt": asset.created_at
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as f64,
                            "updatedAt": asset.updated_at
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as f64,
                        })
                    })
                    .collect();

                match serde_json::to_string(&assets_json) {
                    Ok(json) => Ok(json),
                    Err(e) => {
                        error!("Failed to serialize assets to JSON: {}", e);
                        Ok("[]".to_string())
                    }
                }
            },
        )?;
        asset_storage.set("listAssets", list_assets)?;

        // Secure fetchAsset function
        let user_ctx_fetch = user_context.clone();
        let script_uri_fetch = script_uri_remaining.clone();
        let fetch_asset = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, uri: String| -> JsResult<String> {
                // Check capability
                if let Err(e) =
                    user_ctx_fetch.require_capability(&crate::security::Capability::ReadAssets)
                {
                    return Ok(format!("Error: {}", e));
                }

                debug!(
                    user_id = ?user_ctx_fetch.user_id,
                    uri = %uri,
                    "Secure fetchAsset called"
                );

                match repository::fetch_asset(&script_uri_fetch, &uri) {
                    Some(asset) => {
                        // Convert bytes to base64 for safe JavaScript transfer
                        Ok(base64::engine::general_purpose::STANDARD.encode(asset.content))
                    }
                    None => Ok(format!("Asset '{}' not found", uri)),
                }
            },
        )?;
        asset_storage.set("fetchAsset", fetch_asset)?;

        // Secure upsertAsset function
        let user_ctx_upsert_asset = user_context.clone();
        let _secure_ops_asset = secure_ops.clone();
        let auditor_asset = auditor.clone();
        let script_uri_asset = script_uri_owned.clone();
        let upsert_asset = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  uri: String,
                  mimetype: String,
                  content_b64: String,
                  name: Opt<String>|
                  -> JsResult<String> {
                // Decode base64 content
                let content = match base64::engine::general_purpose::STANDARD.decode(&content_b64) {
                    Ok(c) => c,
                    Err(e) => return Ok(format!("Error decoding base64 content: {}", e)),
                };

                // Check capability
                if let Err(e) = user_ctx_upsert_asset
                    .require_capability(&crate::security::Capability::WriteAssets)
                {
                    return Ok(format!("Access denied: {}", e));
                }

                // Validate asset URI (inline validation since we can't call async)
                if uri.is_empty() || uri.len() > 255 {
                    return Ok("Invalid asset URI: must be 1-255 characters".to_string());
                }
                if uri.contains("..") || uri.contains('\\') {
                    return Ok("Invalid asset URI: path traversal not allowed".to_string());
                }

                // Validate content size (10MB limit)
                if content.len() > 10 * 1024 * 1024 {
                    return Ok("Asset too large (max 10MB)".to_string());
                }

                // Log the operation attempt using spawn to avoid runtime conflicts
                let auditor_clone = auditor_asset.clone();
                let user_id = user_ctx_upsert_asset.user_id.clone();
                let uri_clone = uri.clone();
                let script_uri_clone = script_uri_asset.clone();
                let content_len = content.len();
                let mimetype_clone = mimetype.clone();
                tokio::task::spawn(async move {
                    let _ = auditor_clone
                        .log_event(
                            crate::security::SecurityEvent::new(
                                SecurityEventType::SystemSecurityEvent,
                                SecuritySeverity::Medium,
                                user_id,
                            )
                            .with_resource("asset".to_string())
                            .with_action("upsert".to_string())
                            .with_detail("uri", &uri_clone)
                            .with_detail("script_uri", &script_uri_clone)
                            .with_detail("content_size", content_len.to_string())
                            .with_detail("mimetype", &mimetype_clone),
                        )
                        .await;
                });

                // Call repository directly (sync operation)
                let now = std::time::SystemTime::now();
                let asset = repository::Asset {
                    uri: uri.clone(),
                    name: name.0.or_else(|| Some(uri.clone())),
                    mimetype,
                    content,
                    created_at: now,
                    updated_at: now,
                    script_uri: script_uri_owned.clone(),
                };
                match repository::upsert_asset(asset) {
                    Ok(_) => Ok(format!("Asset '{}' upserted successfully", uri)),
                    Err(e) => Ok(format!("Error upserting asset: {}", e)),
                }
            },
        )?;
        asset_storage.set("upsertAsset", upsert_asset)?;

        // Secure deleteAsset function
        let user_ctx_delete_asset = user_context.clone();
        let auditor_delete_asset = auditor.clone();
        let script_uri_delete_asset = script_uri_remaining.clone();
        let delete_asset = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, uri: String| -> JsResult<String> {
                // Check capability
                if let Err(e) = user_ctx_delete_asset
                    .require_capability(&crate::security::Capability::DeleteAssets)
                {
                    // Use spawn for fire-and-forget audit logging to avoid runtime conflicts
                    let auditor_clone = auditor_delete_asset.clone();
                    let user_id = user_ctx_delete_asset.user_id.clone();
                    tokio::task::spawn(async move {
                        let _ = auditor_clone
                            .log_authz_failure(
                                user_id,
                                "asset".to_string(),
                                "delete".to_string(),
                                "DeleteAssets".to_string(),
                            )
                            .await;
                    });
                    return Ok(format!("Error: {}", e));
                }

                // Log the operation attempt using spawn to avoid runtime conflicts
                let auditor_clone = auditor_delete_asset.clone();
                let user_id = user_ctx_delete_asset.user_id.clone();
                let uri_clone = uri.clone();
                tokio::task::spawn(async move {
                    let _ = auditor_clone
                        .log_event(
                            crate::security::SecurityEvent::new(
                                SecurityEventType::SystemSecurityEvent,
                                SecuritySeverity::High,
                                user_id,
                            )
                            .with_resource("asset".to_string())
                            .with_action("delete".to_string())
                            .with_detail("uri", &uri_clone),
                        )
                        .await;
                });

                debug!(
                    user_id = ?user_ctx_delete_asset.user_id,
                    uri = %uri,
                    "Secure deleteAsset called"
                );

                match repository::delete_asset(&script_uri_delete_asset, &uri) {
                    true => Ok(format!("Asset '{}' deleted successfully", uri)),
                    false => Ok(format!("Asset '{}' not found", uri)),
                }
            },
        )?;
        asset_storage.set("deleteAsset", delete_asset)?;

        // Set the assetStorage object on the global scope
        global.set("assetStorage", asset_storage)?;
        Ok(())
    }

    /// Setup secret storage functions
    ///
    /// Exposes a JavaScript API for per-user secret management scoped to the current script.
    /// All methods require an authenticated user; unauthenticated calls return errors or false.
    /// Secrets are stored in the user_secrets table keyed by (script_uri, user_id, key).
    ///
    /// - secretStorage.exists(key): boolean
    /// - secretStorage.setSecret(key, value): string
    /// - secretStorage.removeSecret(key): boolean
    /// - secretStorage.clear(): string
    fn setup_secrets_functions(&self, ctx: &rquickjs::Ctx<'_>, script_uri: &str) -> JsResult<()> {
        let global = ctx.globals();
        let script_uri_owned = script_uri.to_string();

        let secret_storage_obj = rquickjs::Object::new(ctx.clone())?;

        // secretStorage.exists(key) - Check if secret exists in user_secrets or script_secrets
        let script_uri_exists = script_uri_owned.clone();
        let exists_fn = Function::new(
            ctx.clone(),
            move |ctx: rquickjs::Ctx<'_>, key: String| -> JsResult<bool> {
                let globals = ctx.globals();
                // Check user_secrets first (if authenticated)
                if let Some(user_id) = get_auth_user_id(&globals)
                    && crate::repository::get_user_secret_item(&script_uri_exists, &user_id, &key)
                        .is_some()
                {
                    return Ok(true);
                }
                // Fall back to script_secrets
                Ok(crate::repository::get_script_secret_item(&script_uri_exists, &key).is_some())
            },
        )?;
        secret_storage_obj.set("exists", exists_fn)?;

        // secretStorage.setSecret(key, value) - Store a secret for current user
        let script_uri_set = script_uri_owned.clone();
        let set_secret_fn = Function::new(
            ctx.clone(),
            move |ctx: rquickjs::Ctx<'_>, key: String, value: String| -> JsResult<String> {
                let globals = ctx.globals();
                let user_id = match get_auth_user_id(&globals) {
                    Some(id) => id,
                    None => {
                        return Ok(
                            "Error: Secret storage requires authentication. Please log in."
                                .to_string(),
                        );
                    }
                };
                if key.trim().is_empty() {
                    return Ok("Error: Key cannot be empty".to_string());
                }
                if value.len() > 1_000_000 {
                    return Ok("Error: Value too large (>1MB)".to_string());
                }
                match crate::repository::set_user_secret_item(
                    &script_uri_set,
                    &user_id,
                    &key,
                    &value,
                ) {
                    Ok(()) => Ok("Secret set successfully".to_string()),
                    Err(e) => Ok(format!("Error setting secret: {}", e)),
                }
            },
        )?;
        secret_storage_obj.set("setSecret", set_secret_fn)?;

        // secretStorage.removeSecret(key) - Remove a single secret for current user
        let script_uri_remove = script_uri_owned.clone();
        let remove_secret_fn = Function::new(
            ctx.clone(),
            move |ctx: rquickjs::Ctx<'_>, key: String| -> JsResult<bool> {
                let globals = ctx.globals();
                let user_id = match get_auth_user_id(&globals) {
                    Some(id) => id,
                    None => return Ok(false),
                };
                Ok(crate::repository::remove_user_secret_item(
                    &script_uri_remove,
                    &user_id,
                    &key,
                ))
            },
        )?;
        secret_storage_obj.set("removeSecret", remove_secret_fn)?;

        // secretStorage.clear() - Clear all secrets for current user in this script
        let script_uri_clear = script_uri_owned.clone();
        let clear_fn = Function::new(
            ctx.clone(),
            move |ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
                let globals = ctx.globals();
                let user_id = match get_auth_user_id(&globals) {
                    Some(id) => id,
                    None => {
                        return Ok(
                            "Error: Secret storage requires authentication. Please log in."
                                .to_string(),
                        );
                    }
                };
                match crate::repository::clear_user_secrets(&script_uri_clear, &user_id) {
                    Ok(()) => Ok("Secrets cleared successfully".to_string()),
                    Err(e) => Ok(format!("Error clearing secrets: {}", e)),
                }
            },
        )?;
        secret_storage_obj.set("clear", clear_fn)?;

        global.set("secretStorage", secret_storage_obj)?;

        debug!(
            "secretStorage JavaScript API initialized for script: {}",
            script_uri
        );

        Ok(())
    }

    /// Setup secure GraphQL functions  
    fn setup_graphql_functions(&self, ctx: &rquickjs::Ctx<'_>, script_uri: &str) -> JsResult<()> {
        let global = ctx.globals();
        let user_context = self.user_context.clone();
        let secure_ops = self.secure_ops.clone();
        let auditor = self.auditor.clone();
        let script_uri_owned = script_uri.to_string();

        // Secure registerGraphQLQuery function
        let user_ctx_query = user_context.clone();
        let _secure_ops_query = secure_ops.clone();
        let auditor_query = auditor.clone();
        let script_uri_query = script_uri_owned.clone();
        let config_query = self.config.clone();
        let register_graphql_query = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  name: String,
                  sdl: String,
                  resolver_function: String,
                  visibility: String|
                  -> JsResult<String> {
                debug!(
                    "registerGraphQLQuery called: name={}, visibility={}",
                    name, visibility
                );
                if !config_query.registration_phase {
                    return Ok(registration_inactive(
                        "graphQLRegistry.registerQuery",
                        &name,
                    ));
                }

                // Check capability
                if let Err(e) =
                    user_ctx_query.require_capability(&crate::security::Capability::ManageGraphQL)
                {
                    // Use spawn for fire-and-forget audit logging to avoid runtime conflicts
                    let auditor_clone = auditor_query.clone();
                    let user_id = user_ctx_query.user_id.clone();
                    tokio::task::spawn(async move {
                        let _ = auditor_clone
                            .log_authz_failure(
                                user_id,
                                "graphql".to_string(),
                                "register_query".to_string(),
                                "ManageGraphQL".to_string(),
                            )
                            .await;
                    });
                    return Ok(format!("Error: {}", e));
                }

                // Validate GraphQL schema inline (sync validation)
                // Basic SDL validation
                if sdl.is_empty() || sdl.len() > 100_000 {
                    return Ok("Invalid SDL: must be between 1 and 100,000 characters".to_string());
                }
                if name.is_empty() || name.len() > 100 {
                    return Ok(
                        "Invalid query name: must be between 1 and 100 characters".to_string()
                    );
                }
                // Check for dangerous patterns
                if sdl.contains("__proto__") || sdl.contains("constructor") {
                    return Ok("Invalid SDL: contains dangerous patterns".to_string());
                }

                // Log the operation attempt using spawn to avoid runtime conflicts
                let auditor_clone = auditor_query.clone();
                let user_id = user_ctx_query.user_id.clone();
                let name_clone = name.clone();
                let script_uri_clone = script_uri_query.clone();
                let sdl_len = sdl.len();
                let visibility_clone = visibility.clone();
                tokio::task::spawn(async move {
                    let _ = auditor_clone
                        .log_event(
                            crate::security::SecurityEvent::new(
                                SecurityEventType::SystemSecurityEvent,
                                SecuritySeverity::Medium,
                                user_id,
                            )
                            .with_resource("graphql".to_string())
                            .with_action("register_query".to_string())
                            .with_detail("query_name", &name_clone)
                            .with_detail("script_uri", &script_uri_clone)
                            .with_detail("sdl_length", sdl_len.to_string())
                            .with_detail("visibility", &visibility_clone),
                        )
                        .await;
                });

                debug!(
                    user_id = ?user_ctx_query.user_id,
                    name = %name,
                    sdl_len = sdl.len(),
                    visibility = %visibility,
                    "Secure registerGraphQLQuery called"
                );

                if let Some(reply) = config_query.collect(
                    CollectedRegistration::new(RegistrationKind::GraphqlQuery, name.clone())
                        .with_handler(resolver_function.clone()),
                ) {
                    return Ok(reply);
                }

                // Actually register the GraphQL query
                match crate::graphql::register_graphql_query(
                    name.clone(),
                    sdl.clone(),
                    resolver_function.clone(),
                    script_uri_query.clone(),
                    visibility,
                ) {
                    Ok(()) => Ok(format!("GraphQL query '{}' registered successfully", name)),
                    Err(e) => Ok(format!("Error registering GraphQL query '{}': {}", name, e)),
                }
            },
        )?;

        // Secure registerGraphQLMutation function
        let user_ctx_mutation = user_context.clone();
        let _secure_ops_mutation = secure_ops.clone();
        let auditor_mutation = auditor.clone();
        let script_uri_mutation = script_uri_owned.clone();
        let config_mutation = self.config.clone();
        let register_graphql_mutation = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  name: String,
                  sdl: String,
                  resolver_function: String,
                  visibility: String|
                  -> JsResult<String> {
                debug!(
                    "registerGraphQLMutation called: name={}, visibility={}",
                    name, visibility
                );
                if !config_mutation.registration_phase {
                    return Ok(registration_inactive(
                        "graphQLRegistry.registerMutation",
                        &name,
                    ));
                }

                // Check capability
                if let Err(e) = user_ctx_mutation
                    .require_capability(&crate::security::Capability::ManageGraphQL)
                {
                    // Use spawn for fire-and-forget audit logging to avoid runtime conflicts
                    let auditor_clone = auditor_mutation.clone();
                    let user_id = user_ctx_mutation.user_id.clone();
                    tokio::task::spawn(async move {
                        let _ = auditor_clone
                            .log_authz_failure(
                                user_id,
                                "graphql".to_string(),
                                "register_mutation".to_string(),
                                "ManageGraphQL".to_string(),
                            )
                            .await;
                    });
                    return Ok(format!("Error: {}", e));
                }

                // Validate GraphQL schema inline (sync validation)
                if sdl.is_empty() || sdl.len() > 100_000 {
                    return Ok("Invalid SDL: must be between 1 and 100,000 characters".to_string());
                }
                if name.is_empty() || name.len() > 100 {
                    return Ok(
                        "Invalid mutation name: must be between 1 and 100 characters".to_string(),
                    );
                }
                if sdl.contains("__proto__") || sdl.contains("constructor") {
                    return Ok("Invalid SDL: contains dangerous patterns".to_string());
                }

                // Log the operation attempt using spawn to avoid runtime conflicts
                let auditor_clone = auditor_mutation.clone();
                let user_id = user_ctx_mutation.user_id.clone();
                let name_clone = name.clone();
                let sdl_len = sdl.len();
                let visibility_clone = visibility.clone();
                tokio::task::spawn(async move {
                    let _ = auditor_clone
                        .log_event(
                            crate::security::SecurityEvent::new(
                                SecurityEventType::SystemSecurityEvent,
                                SecuritySeverity::Medium,
                                user_id,
                            )
                            .with_resource("graphql".to_string())
                            .with_action("register_mutation".to_string())
                            .with_detail("mutation_name", &name_clone)
                            .with_detail("sdl_length", sdl_len.to_string())
                            .with_detail("visibility", &visibility_clone),
                        )
                        .await;
                });

                debug!(
                    user_id = ?user_ctx_mutation.user_id,
                    name = %name,
                    sdl_len = sdl.len(),
                    visibility = %visibility,
                    "Secure registerGraphQLMutation called"
                );

                if let Some(reply) = config_mutation.collect(
                    CollectedRegistration::new(RegistrationKind::GraphqlMutation, name.clone())
                        .with_handler(resolver_function.clone()),
                ) {
                    return Ok(reply);
                }

                // Actually register the GraphQL mutation
                match crate::graphql::register_graphql_mutation(
                    name.clone(),
                    sdl.clone(),
                    resolver_function.clone(),
                    script_uri_mutation.clone(),
                    visibility,
                ) {
                    Ok(()) => Ok(format!(
                        "GraphQL mutation '{}' registered successfully",
                        name
                    )),
                    Err(e) => Ok(format!(
                        "Error registering GraphQL mutation '{}': {}",
                        name, e
                    )),
                }
            },
        )?;

        // Secure registerGraphQLSubscription function
        let user_ctx_subscription = user_context.clone();
        let _secure_ops_subscription = secure_ops.clone();
        let auditor_subscription = auditor.clone();
        let script_uri_subscription = script_uri_owned.clone();
        let config_subscription = self.config.clone();
        let register_graphql_subscription = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  name: String,
                  sdl: String,
                  resolver_function: String,
                  visibility: String|
                  -> JsResult<String> {
                debug!(
                    "registerGraphQLSubscription called: name={}, visibility={}",
                    name, visibility
                );
                if !config_subscription.registration_phase {
                    return Ok(registration_inactive(
                        "graphQLRegistry.registerSubscription",
                        &name,
                    ));
                }

                // Check capability
                if let Err(e) = user_ctx_subscription
                    .require_capability(&crate::security::Capability::ManageGraphQL)
                {
                    // Use spawn for fire-and-forget audit logging to avoid runtime conflicts
                    let auditor_clone = auditor_subscription.clone();
                    let user_id = user_ctx_subscription.user_id.clone();
                    tokio::task::spawn(async move {
                        let _ = auditor_clone
                            .log_authz_failure(
                                user_id,
                                "graphql".to_string(),
                                "register_subscription".to_string(),
                                "ManageGraphQL".to_string(),
                            )
                            .await;
                    });
                    return Ok(format!("Error: {}", e));
                }

                // Validate GraphQL schema inline (sync validation)
                if sdl.is_empty() || sdl.len() > 100_000 {
                    return Ok("Invalid SDL: must be between 1 and 100,000 characters".to_string());
                }
                if name.is_empty() || name.len() > 100 {
                    return Ok(
                        "Invalid subscription name: must be between 1 and 100 characters"
                            .to_string(),
                    );
                }
                if sdl.contains("__proto__") || sdl.contains("constructor") {
                    return Ok("Invalid SDL: contains dangerous patterns".to_string());
                }

                // Log the operation attempt using spawn to avoid runtime conflicts
                let auditor_clone = auditor_subscription.clone();
                let user_id = user_ctx_subscription.user_id.clone();
                let name_clone = name.clone();
                let sdl_len = sdl.len();
                let visibility_clone = visibility.clone();
                tokio::task::spawn(async move {
                    let _ = auditor_clone
                        .log_event(
                            crate::security::SecurityEvent::new(
                                SecurityEventType::SystemSecurityEvent,
                                SecuritySeverity::Medium,
                                user_id,
                            )
                            .with_resource("graphql".to_string())
                            .with_action("register_subscription".to_string())
                            .with_detail("subscription_name", &name_clone)
                            .with_detail("sdl_length", sdl_len.to_string())
                            .with_detail("visibility", &visibility_clone),
                        )
                        .await;
                });

                debug!(
                    user_id = ?user_ctx_subscription.user_id,
                    name = %name,
                    sdl_len = sdl.len(),
                    visibility = %visibility,
                    "Secure registerGraphQLSubscription called"
                );

                if let Some(reply) = config_subscription.collect(
                    CollectedRegistration::new(RegistrationKind::GraphqlSubscription, name.clone())
                        .with_handler(resolver_function.clone()),
                ) {
                    return Ok(reply);
                }

                // Actually register the GraphQL subscription
                match crate::graphql::register_graphql_subscription(
                    name.clone(),
                    sdl.clone(),
                    resolver_function.clone(),
                    script_uri_subscription.clone(),
                    visibility,
                ) {
                    Ok(()) => Ok(format!(
                        "GraphQL subscription '{}' registered successfully",
                        name
                    )),
                    Err(e) => Ok(format!(
                        "Error registering GraphQL subscription '{}': {}",
                        name, e
                    )),
                }
            },
        )?;

        // Secure executeGraphQL function
        let user_ctx_execute = user_context.clone();
        let auditor_execute = auditor.clone();
        let script_uri_execute = script_uri_owned.clone();
        let execute_graphql = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  query: String,
                  variables_json: Option<String>|
                  -> JsResult<String> {
                // Executing a query is a read of the live schema, not a
                // registration, so it works in every context. It was previously
                // gated on the registration flag, which left it usable only
                // during startup - the one phase where a script has least
                // reason to run one.
                debug!("executeGraphQL called: query_length={}", query.len());

                // Check capability
                if let Err(e) =
                    user_ctx_execute.require_capability(&crate::security::Capability::ManageGraphQL)
                {
                    // Use spawn for fire-and-forget audit logging to avoid runtime conflicts
                    let auditor_clone = auditor_execute.clone();
                    let user_id = user_ctx_execute.user_id.clone();
                    tokio::task::spawn(async move {
                        let _ = auditor_clone
                            .log_authz_failure(
                                user_id,
                                "graphql".to_string(),
                                "execute".to_string(),
                                "ManageGraphQL".to_string(),
                            )
                            .await;
                    });
                    return Ok(format!("{{\"errors\": [{{\"message\": \"{}\"}}]}}", e));
                }

                // Validate query
                if query.is_empty() || query.len() > 100_000 {
                    return Ok("{\"errors\": [{\"message\": \"Invalid query: must be between 1 and 100,000 characters\"}]}".to_string());
                }

                // Parse variables if provided
                let variables = if let Some(vars_json) = variables_json {
                    if vars_json.len() > 50_000 {
                        return Ok("{\"errors\": [{\"message\": \"Variables too large: max 50,000 characters\"}]}".to_string());
                    }
                    match serde_json::from_str::<serde_json::Value>(&vars_json) {
                        Ok(v) => Some(v),
                        Err(e) => {
                            return Ok(format!(
                                "{{\"errors\": [{{\"message\": \"Invalid variables JSON: {}\"}}]}}",
                                e
                            ));
                        }
                    }
                } else {
                    None
                };

                // Log the operation attempt using spawn to avoid runtime conflicts
                let auditor_clone = auditor_execute.clone();
                let user_id = user_ctx_execute.user_id.clone();
                let query_clone = query.clone();
                let script_uri_clone = script_uri_execute.clone();
                tokio::task::spawn(async move {
                    let _ = auditor_clone
                        .log_event(
                            crate::security::SecurityEvent::new(
                                SecurityEventType::SystemSecurityEvent,
                                SecuritySeverity::Medium,
                                user_id,
                            )
                            .with_resource("graphql".to_string())
                            .with_action("execute".to_string())
                            .with_detail("script_uri", &script_uri_clone)
                            .with_detail("query_length", query_clone.len().to_string()),
                        )
                        .await;
                });

                debug!(
                    user_id = ?user_ctx_execute.user_id,
                    query_len = query.len(),
                    has_variables = variables.is_some(),
                    "Secure executeGraphQL called"
                );

                // Execute the GraphQL query
                match crate::graphql::execute_graphql_query_sync(&query, variables) {
                    Ok(result_json) => {
                        debug!("GraphQL execution successful");
                        Ok(result_json)
                    }
                    Err(e) => {
                        tracing::error!("GraphQL execution failed: {}", e);
                        Ok(format!(
                            "{{\"errors\": [{{\"message\": \"GraphQL execution failed: {}\"}}]}}",
                            e
                        ))
                    }
                }
            },
        )?;

        // Secure sendSubscriptionMessage function
        let user_ctx_send_sub = user_context.clone();
        let auditor_send_sub = auditor.clone();
        let send_subscription_message = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  subscription_name: String,
                  message: String|
                  -> JsResult<String> {
                // Check capability
                if let Err(e) = user_ctx_send_sub
                    .require_capability(&crate::security::Capability::ManageGraphQL)
                {
                    // Use spawn for fire-and-forget audit logging to avoid runtime conflicts
                    let auditor_clone = auditor_send_sub.clone();
                    let user_id = user_ctx_send_sub.user_id.clone();
                    tokio::task::spawn(async move {
                        let _ = auditor_clone
                            .log_authz_failure(
                                user_id,
                                "graphql".to_string(),
                                "send_subscription_message".to_string(),
                                "ManageGraphQL".to_string(),
                            )
                            .await;
                    });
                    return Ok(format!("Error: {}", e));
                }

                // Log the operation attempt using spawn to avoid runtime conflicts
                let auditor_clone = auditor_send_sub.clone();
                let user_id = user_ctx_send_sub.user_id.clone();
                let subscription_name_clone = subscription_name.clone();
                let message_clone = message.clone();
                tokio::task::spawn(async move {
                    let _ = auditor_clone
                        .log_event(
                            crate::security::SecurityEvent::new(
                                SecurityEventType::SystemSecurityEvent,
                                SecuritySeverity::Low,
                                user_id,
                            )
                            .with_resource("graphql".to_string())
                            .with_action("send_subscription_message".to_string())
                            .with_detail("subscription_name", &subscription_name_clone)
                            .with_detail("message_length", message_clone.len().to_string()),
                        )
                        .await;
                });

                debug!(
                    user_id = ?user_ctx_send_sub.user_id,
                    subscription_name = %subscription_name,
                    message_len = message.len(),
                    "Secure sendSubscriptionMessage called"
                );

                // Send to the auto-registered stream path for this subscription
                let stream_path = format!("/engine/graphql/subscription/{}", subscription_name);

                // Call actual stream message sending (sync operation)
                match crate::stream_registry::GLOBAL_STREAM_REGISTRY
                    .broadcast_to_stream(&stream_path, &message)
                {
                    Ok(result) => {
                        if result.is_fully_successful() {
                            Ok(format!(
                                "GraphQL subscription message sent to '{}' ({} connections) successfully",
                                subscription_name, result.successful_sends
                            ))
                        } else {
                            Ok(format!(
                                "GraphQL subscription message to '{}' partially sent: {} successful, {} failed out of {} total",
                                subscription_name,
                                result.successful_sends,
                                result.failed_connections.len(),
                                result.total_connections
                            ))
                        }
                    }
                    Err(e) => Ok(format!(
                        "Failed to send GraphQL subscription message to '{}': {}",
                        subscription_name, e
                    )),
                }
            },
        )?;

        // Secure sendSubscriptionMessageFiltered function (selective broadcasting for GraphQL)
        let user_ctx_send_sub_filtered = user_context.clone();
        let auditor_send_sub_filtered = auditor.clone();
        let send_subscription_message_filtered = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  subscription_name: String,
                  message: String,
                  filter_json: Option<String>,
                  match_mode: Option<String>|
                  -> JsResult<String> {
                // Parse filter criteria from JSON string
                let metadata_filter: HashMap<String, String> = if let Some(json_str) = filter_json {
                    serde_json::from_str(&json_str).map_err(|e| {
                        rquickjs::Error::new_from_js_message(
                            "filter",
                            "MetadataFilter",
                            &format!("Invalid filter JSON: {}", e),
                        )
                    })?
                } else {
                    HashMap::new() // Empty filter matches all connections
                };
                let match_mode = parse_filter_match_mode(match_mode)?;

                // Same capability as the unfiltered `sendSubscriptionMessage`:
                // both publish to /engine/graphql/subscription/{name}, and an
                // empty filter matches every connection, so exempting this one
                // would just be a bypass of the check on its sibling.
                if let Err(e) = user_ctx_send_sub_filtered
                    .require_capability(&crate::security::Capability::ManageGraphQL)
                {
                    // Use spawn for fire-and-forget audit logging to avoid runtime conflicts
                    let auditor_clone = auditor_send_sub_filtered.clone();
                    let user_id = user_ctx_send_sub_filtered.user_id.clone();
                    tokio::task::spawn(async move {
                        let _ = auditor_clone
                            .log_authz_failure(
                                user_id,
                                "graphql".to_string(),
                                "send_subscription_message_to_connections".to_string(),
                                "ManageGraphQL".to_string(),
                            )
                            .await;
                    });
                    return Ok(format!("Error: {}", e));
                }

                // Log the operation attempt using spawn to avoid runtime conflicts
                let auditor_clone = auditor_send_sub_filtered.clone();
                let user_id = user_ctx_send_sub_filtered.user_id.clone();
                let subscription_name_clone = subscription_name.clone();
                let message_clone = message.clone();
                let filter_clone = metadata_filter.clone();
                tokio::task::spawn(async move {
                    let _ = auditor_clone
                        .log_event(
                            crate::security::SecurityEvent::new(
                                SecurityEventType::SystemSecurityEvent,
                                SecuritySeverity::Low,
                                user_id,
                            )
                            .with_resource("graphql".to_string())
                            .with_action("send_subscription_message_to_connections".to_string())
                            .with_detail("subscription_name", &subscription_name_clone)
                            .with_detail("message_length", message_clone.len().to_string())
                            .with_detail("filter_criteria", format!("{:?}", filter_clone)),
                        )
                        .await;
                });

                debug!(
                    user_id = ?user_ctx_send_sub_filtered.user_id,
                    subscription_name = %subscription_name,
                    message_len = message.len(),
                    filter = ?metadata_filter,
                    match_mode = ?match_mode,
                    "Secure sendSubscriptionMessageFiltered called"
                );

                // Send to the auto-registered stream path for this subscription with filtering
                let stream_path = format!("/engine/graphql/subscription/{}", subscription_name);

                // Call selective broadcasting (sync operation)
                let result = crate::stream_registry::GLOBAL_STREAM_REGISTRY
                    .broadcast_to_stream_with_filter_mode(
                        &stream_path,
                        &message,
                        &metadata_filter,
                        match_mode,
                    );

                match result {
                    Ok(broadcast_result) => {
                        if broadcast_result.is_fully_successful() {
                            Ok(format!(
                                "GraphQL subscription message sent to '{}' with filter {:?} ({} connections) successfully",
                                subscription_name,
                                metadata_filter,
                                broadcast_result.successful_sends
                            ))
                        } else {
                            Ok(format!(
                                "GraphQL subscription message to '{}' with filter {:?} partially sent: {} successful, {} failed connections",
                                subscription_name,
                                metadata_filter,
                                broadcast_result.successful_sends,
                                broadcast_result.failed_connections.len()
                            ))
                        }
                    }
                    Err(e) => Ok(format!(
                        "Failed to send GraphQL subscription message to '{}' with filter: {}",
                        subscription_name, e
                    )),
                }
            },
        )?;

        // Create graphQLRegistry object with all 6 functions
        let graphql_registry = rquickjs::Object::new(ctx.clone())?;
        graphql_registry.set("registerQuery", register_graphql_query)?;
        graphql_registry.set("registerMutation", register_graphql_mutation)?;
        graphql_registry.set("registerSubscription", register_graphql_subscription)?;
        graphql_registry.set("executeGraphQL", execute_graphql)?;
        graphql_registry.set("sendSubscriptionMessage", send_subscription_message)?;
        graphql_registry.set(
            "sendSubscriptionMessageFiltered",
            send_subscription_message_filtered,
        )?;
        global.set("graphQLRegistry", graphql_registry)?;

        Ok(())
    }

    /// Setup MCP (Model Context Protocol) registry functions
    fn setup_mcp_functions(&self, ctx: &rquickjs::Ctx<'_>, script_uri: &str) -> JsResult<()> {
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
                    return Ok(registration_inactive("mcpRegistry.registerTool", &name));
                }

                // Check capability - reuse ManageGraphQL for MCP tools
                if let Err(e) = user_ctx_register
                    .require_capability(&crate::security::Capability::ManageGraphQL)
                {
                    let auditor_clone = auditor_register.clone();
                    let user_id = user_ctx_register.user_id.clone();
                    tokio::task::spawn(async move {
                        let _ = auditor_clone
                            .log_authz_failure(
                                user_id,
                                "mcp".to_string(),
                                "register_tool".to_string(),
                                "ManageGraphQL".to_string(),
                            )
                            .await;
                    });
                    return Ok(format!("Error: {}", e));
                }

                // Validate inputs
                if name.is_empty() || name.len() > 100 {
                    return Ok(
                        "Invalid tool name: must be between 1 and 100 characters".to_string()
                    );
                }
                if description.is_empty() || description.len() > 1000 {
                    return Ok(
                        "Invalid description: must be between 1 and 1000 characters".to_string()
                    );
                }

                // Parse and validate input schema JSON
                let input_schema: serde_json::Value = serde_json::from_str(&input_schema_json)
                    .map_err(|e| {
                        rquickjs::Error::new_from_js_message(
                            "schema",
                            "InputSchema",
                            &format!("Invalid input schema JSON: {}", e),
                        )
                    })?;

                // Check for dangerous patterns
                if input_schema_json.contains("__proto__")
                    || input_schema_json.contains("constructor")
                {
                    return Ok("Invalid schema: contains dangerous patterns".to_string());
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

                if let Some(reply) = config_register.collect(
                    CollectedRegistration::new(RegistrationKind::McpTool, name.clone())
                        .with_handler(handler_function.clone()),
                ) {
                    return Ok(reply);
                }

                // Actually register the MCP tool
                crate::mcp::register_mcp_tool(
                    name.clone(),
                    description,
                    input_schema,
                    handler_function,
                    script_uri_register.clone(),
                );

                Ok(format!("MCP tool '{}' registered successfully", name))
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
                    return Ok(registration_inactive("mcpRegistry.registerPrompt", &name));
                }

                // Check capability - reuse ManageGraphQL for MCP prompts
                if let Err(e) =
                    user_ctx_prompt.require_capability(&crate::security::Capability::ManageGraphQL)
                {
                    let auditor_clone = auditor_prompt.clone();
                    let user_id = user_ctx_prompt.user_id.clone();
                    tokio::task::spawn(async move {
                        let _ = auditor_clone
                            .log_authz_failure(
                                user_id,
                                "mcp".to_string(),
                                "register_prompt".to_string(),
                                "ManageGraphQL".to_string(),
                            )
                            .await;
                    });
                    return Ok(format!("Error: {}", e));
                }

                // Validate inputs
                if name.is_empty() || name.len() > 100 {
                    return Ok(
                        "Invalid prompt name: must be between 1 and 100 characters".to_string()
                    );
                }
                if description.is_empty() || description.len() > 1000 {
                    return Ok(
                        "Invalid description: must be between 1 and 1000 characters".to_string()
                    );
                }
                if handler_function.is_empty() || handler_function.len() > 100 {
                    return Ok(
                        "Invalid handler function: must be between 1 and 100 characters"
                            .to_string(),
                    );
                }

                // Validate arguments JSON
                if arguments_json.contains("__proto__") || arguments_json.contains("constructor") {
                    return Ok("Invalid arguments: contains dangerous patterns".to_string());
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

                if let Some(reply) = config_prompt.collect(
                    CollectedRegistration::new(RegistrationKind::McpPrompt, name.clone())
                        .with_handler(handler_function.clone()),
                ) {
                    return Ok(reply);
                }

                // Actually register the MCP prompt
                match crate::mcp::register_mcp_prompt(
                    name.clone(),
                    description,
                    arguments_json,
                    handler_function.clone(),
                    script_uri_prompt.clone(),
                ) {
                    Ok(_) => Ok(format!(
                        "MCP prompt '{}' registered successfully with handler '{}'",
                        name, handler_function
                    )),
                    Err(e) => Ok(format!("Error registering prompt: {}", e)),
                }
            },
        )?;

        // Create mcpRegistry object
        let mcp_registry = rquickjs::Object::new(ctx.clone())?;
        mcp_registry.set("registerTool", register_tool)?;
        mcp_registry.set("registerPrompt", register_prompt)?;
        global.set("mcpRegistry", mcp_registry)?;

        // Setup McpClient class for connecting to external MCP servers
        self.setup_mcp_client_class(ctx, script_uri)?;

        Ok(())
    }

    /// Setup McpClient class for external MCP server connections
    fn setup_mcp_client_class(&self, ctx: &rquickjs::Ctx<'_>, script_uri: &str) -> JsResult<()> {
        let global = ctx.globals();
        let script_uri_owned = script_uri.to_string();
        // Capture user_id for secret resolution (user_secrets first, then script_secrets)
        let user_id_for_mcp = self.user_context.user_id.clone();

        // McpClient constructor
        let mcp_client_constructor = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  server_url: String,
                  secret_identifier: String|
                  -> JsResult<String> {
                // Create MCP client instance (just validate parameters)
                let _client = crate::mcp_client::McpClient::new(
                    server_url.clone(),
                    secret_identifier.clone(),
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
                let client = crate::mcp_client::McpClient::new(
                    server_url.to_string(),
                    secret_identifier.to_string(),
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
                let client = crate::mcp_client::McpClient::new(
                    server_url.to_string(),
                    secret_identifier.to_string(),
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
        global.set("McpClient", mcp_client_class)?;

        debug!("McpClient class initialized for external MCP server connections");

        Ok(())
    }

    /// Setup routeRegistry object with all route-related functions
    fn setup_route_registry(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
        register_fn: Option<RouteRegisterFn>,
    ) -> JsResult<()> {
        let global = ctx.globals();
        let user_context = self.user_context.clone();
        let auditor = self.auditor.clone();
        let script_uri_owned = script_uri.to_string();
        let config = self.config.clone();

        // Create the routeRegistry object
        let route_registry = rquickjs::Object::new(ctx.clone())?;

        // 1. registerRoute function
        if let Some(register_impl) = register_fn {
            let register_route = Function::new(
                ctx.clone(),
                move |_ctx: rquickjs::Ctx<'_>,
                      path: String,
                      handler: String,
                      method: Option<String>,
                      metadata: Opt<rquickjs::Object>|
                      -> JsResult<String> {
                    // Engine-owned prefixes are off-limits; any script may
                    // register any other path.
                    if let Some(prefix) = crate::engine_api::reserved_route_prefix(&path) {
                        return Err(rquickjs::Error::new_from_js_message(
                            "routeRegistry.registerRoute",
                            "reserved_path",
                            &format!(
                                "Path '{}' is reserved for the engine (prefix '{}')",
                                path, prefix
                            ),
                        ));
                    }

                    // Build RouteMetadata from parameters
                    let mut route_meta = repository::RouteMetadata::simple(handler.clone());

                    if let Some(meta_obj) = metadata.0 {
                        // Extract summary
                        if let Ok(summary) = meta_obj.get::<_, Option<String>>("summary") {
                            route_meta.summary = summary;
                        }
                        // Extract description
                        if let Ok(description) = meta_obj.get::<_, Option<String>>("description") {
                            route_meta.description = description;
                        }
                        // Extract tags
                        if let Ok(tags_arr) = meta_obj.get::<_, rquickjs::Array>("tags") {
                            let mut tags = Vec::new();
                            for i in 0..tags_arr.len() {
                                if let Ok(tag) = tags_arr.get::<String>(i) {
                                    tags.push(tag);
                                }
                            }
                            route_meta.tags = tags;
                        }
                        // Extract parameters
                        if let Ok(Some(params_json)) =
                            meta_obj.get::<_, Option<String>>("parameters")
                            && let Ok(params_value) =
                                serde_json::from_str::<serde_json::Value>(&params_json)
                        {
                            route_meta.parameters = Some(params_value);
                        }
                        // Extract requestBody
                        if let Ok(Some(body_json)) =
                            meta_obj.get::<_, Option<String>>("requestBody")
                            && let Ok(body_value) =
                                serde_json::from_str::<serde_json::Value>(&body_json)
                        {
                            route_meta.request_body = Some(body_value);
                        }
                    }

                    let method_ref = method.as_deref();
                    register_impl(&path, &route_meta, method_ref)?;
                    Ok(format!(
                        "Route '{} {}' registered to handler '{}'",
                        method_ref.unwrap_or("GET"),
                        path,
                        route_meta.handler_name
                    ))
                },
            )?;
            route_registry.set("registerRoute", register_route)?;
        } else {
            // Outside the registration phase there is nothing to register into,
            // but the reserved-path check still applies so a bad path is
            // reported the same way in every context.
            let reg_noop = Function::new(
                ctx.clone(),
                move |_c: rquickjs::Ctx<'_>,
                      path: String,
                      _h: String,
                      _m: Option<String>,
                      _meta: Opt<rquickjs::Object>|
                      -> JsResult<String> {
                    if let Some(prefix) = crate::engine_api::reserved_route_prefix(&path) {
                        return Err(rquickjs::Error::new_from_js_message(
                            "routeRegistry.registerRoute",
                            "reserved_path",
                            &format!(
                                "Path '{}' is reserved for the engine (prefix '{}')",
                                path, prefix
                            ),
                        ));
                    }

                    Ok(registration_inactive("routeRegistry.registerRoute", &path))
                },
            )?;
            route_registry.set("registerRoute", reg_noop)?;
        }

        // 2. registerStreamRoute function
        let user_ctx_stream = user_context.clone();
        let auditor_stream = auditor.clone();
        let config_stream = config.clone();
        let script_uri_stream = script_uri_owned.clone();
        let register_stream_route = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  path: String,
                  customization_function: Opt<String>,
                  metadata: Opt<rquickjs::Object>|
                  -> JsResult<String> {
                // Convert Opt to Option
                let customization_function = customization_function.0;
                // Extract optional OpenAPI metadata (tags/summary/description)
                let (tags, summary, description) = extract_route_metadata(metadata.0.as_ref());
                // Argument validation runs in every context, so a malformed
                // path is reported the same way wherever the call is made.
                //
                // Engine-owned prefixes are off-limits; any script may
                // register any other stream path.
                if let Some(prefix) = crate::engine_api::reserved_route_prefix(&path) {
                    return Err(rquickjs::Error::new_from_js_message(
                        "routeRegistry.registerStreamRoute",
                        "reserved_path",
                        &format!(
                            "Path '{}' is reserved for the engine (prefix '{}')",
                            path, prefix
                        ),
                    ));
                }

                // Validate path format
                if path.is_empty() || !path.starts_with('/') {
                    return Ok(format!(
                        "Invalid stream path '{}': path must start with '/' and not be empty",
                        path
                    ));
                }

                if !config_stream.registration_phase {
                    return Ok(registration_inactive(
                        "routeRegistry.registerStreamRoute",
                        &path,
                    ));
                }

                if path.len() > 200 {
                    return Ok(format!(
                        "Invalid stream path '{}': path too long (max 200 characters)",
                        path
                    ));
                }

                // Validate customization function name if provided
                if let Some(ref func_name) = customization_function {
                    if func_name.is_empty() {
                        return Ok(
                            "Invalid customization function: name cannot be empty".to_string()
                        );
                    }
                    if func_name.len() > 100 {
                        return Ok(
                            "Invalid customization function: name too long (max 100 characters)"
                                .to_string(),
                        );
                    }
                    // Basic validation: should be a valid identifier
                    if !func_name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                        return Ok("Invalid customization function: name must contain only alphanumeric characters and underscores".to_string());
                    }
                }

                // Check capability
                if let Err(e) =
                    user_ctx_stream.require_capability(&crate::security::Capability::ManageStreams)
                {
                    if config_stream.enable_audit_logging
                        && let Ok(rt) = tokio::runtime::Handle::try_current()
                    {
                        let auditor_clone = auditor_stream.clone();
                        let user_id = user_ctx_stream.user_id.clone();
                        rt.spawn(async move {
                            let _ = auditor_clone
                                .log_event(
                                    crate::security::SecurityEvent::new(
                                        crate::security::SecurityEventType::AuthorizationFailure,
                                        crate::security::SecuritySeverity::Medium,
                                        user_id,
                                    )
                                    .with_resource("stream".to_string())
                                    .with_action("register".to_string()),
                                )
                                .await;
                        });
                    }
                    return Ok(format!("Error: {}", e));
                }

                // Validate stream path
                if path.contains("..") || path.contains('\\') {
                    return Ok("Invalid stream path: path traversal not allowed".to_string());
                }

                // Log the operation attempt
                if config_stream.enable_audit_logging
                    && let Ok(rt) = tokio::runtime::Handle::try_current()
                {
                    let auditor_clone = auditor_stream.clone();
                    let user_id = user_ctx_stream.user_id.clone();
                    let path_clone = path.clone();
                    let script_uri_clone = script_uri_stream.clone();
                    rt.spawn(async move {
                        let _ = auditor_clone
                            .log_event(
                                crate::security::SecurityEvent::new(
                                    crate::security::SecurityEventType::SystemSecurityEvent,
                                    crate::security::SecuritySeverity::Medium,
                                    user_id,
                                )
                                .with_resource("stream".to_string())
                                .with_action("register".to_string())
                                .with_detail("path", &path_clone)
                                .with_detail("script_uri", &script_uri_clone),
                            )
                            .await;
                    });
                }

                if let Some(reply) = config_stream.collect({
                    let registration =
                        CollectedRegistration::new(RegistrationKind::Stream, path.clone());
                    match customization_function.as_ref() {
                        Some(function) => registration.with_handler(function.clone()),
                        None => registration,
                    }
                }) {
                    return Ok(reply);
                }

                // Register the stream
                match crate::stream_registry::GLOBAL_STREAM_REGISTRY.register_stream_with_metadata(
                    &path,
                    &script_uri_stream,
                    customization_function,
                    crate::stream_registry::StreamRouteMetadata {
                        tags,
                        summary,
                        description,
                    },
                ) {
                    Ok(()) => Ok(format!("Web stream '{}' registered successfully", path)),
                    Err(e) => Ok(format!("Failed to register stream '{}': {}", path, e)),
                }
            },
        )?;
        route_registry.set("registerStreamRoute", register_stream_route)?;

        // 3. registerAssetRoute function
        let user_ctx_asset = user_context.clone();
        let script_uri_asset = script_uri_owned.clone();
        let config_asset_route = self.config.clone();
        let register_asset_route = Function::new(
            ctx.clone(),
            move |_c: rquickjs::Ctx<'_>,
                  path: String,
                  asset_name: String,
                  metadata: Opt<rquickjs::Object>|
                  -> Result<String, rquickjs::Error> {
                // Engine-owned prefixes are off-limits; any script may
                // register any other asset path.
                if let Some(prefix) = crate::engine_api::reserved_route_prefix(&path) {
                    return Err(rquickjs::Error::new_from_js_message(
                        "routeRegistry.registerAssetRoute",
                        "reserved_path",
                        &format!(
                            "Path '{}' is reserved for the engine (prefix '{}')",
                            path, prefix
                        ),
                    ));
                }

                // Check capability
                if let Err(e) =
                    user_ctx_asset.require_capability(&crate::security::Capability::WriteAssets)
                {
                    return Ok(format!("Access denied: {}", e));
                }

                // Validate path
                if !path.starts_with('/') {
                    return Ok("Path must start with '/'".to_string());
                }
                if path.len() > 500 {
                    return Ok("Path too long (max 500 characters)".to_string());
                }

                // Validate asset name
                if asset_name.is_empty() || asset_name.len() > 255 {
                    return Ok("Invalid asset name: must be 1-255 characters".to_string());
                }
                if asset_name.contains("..") || asset_name.contains('\\') {
                    return Ok("Invalid asset name: path characters not allowed".to_string());
                }

                if !config_asset_route.registration_phase {
                    return Ok(registration_inactive(
                        "routeRegistry.registerAssetRoute",
                        &path,
                    ));
                }

                // Verify the asset exists and belongs to this script
                match repository::fetch_asset(&script_uri_asset, &asset_name) {
                    Some(_) => {
                        // Asset exists and belongs to this script, proceed
                    }
                    None => {
                        return Ok(format!(
                            "Asset '{}' not found or not owned by script '{}'",
                            asset_name, script_uri_asset
                        ));
                    }
                }

                // Extract optional OpenAPI metadata (tags/summary/description)
                let (tags, summary, description) = extract_route_metadata(metadata.0.as_ref());

                if let Some(reply) = config_asset_route.collect(CollectedRegistration::new(
                    RegistrationKind::AssetRoute,
                    path.clone(),
                )) {
                    return Ok(reply);
                }

                // Register the path in the global asset registry
                match crate::asset_registry::get_global_registry().register_path_with_metadata(
                    &path,
                    &asset_name,
                    &script_uri_asset,
                    crate::asset_registry::AssetRouteMetadata {
                        tags,
                        summary,
                        description,
                    },
                ) {
                    Ok(()) => Ok(format!(
                        "Asset path '{}' registered to asset '{}'",
                        path, asset_name
                    )),
                    Err(e) => Ok(format!("Failed to register asset path: {}", e)),
                }
            },
        )?;
        route_registry.set("registerAssetRoute", register_asset_route)?;

        // 4. sendStreamMessage function
        let user_ctx_send = user_context.clone();
        let auditor_send = auditor.clone();
        let send_stream_message = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, path: String, message: String| -> JsResult<String> {
                // Allow system-level broadcasting without capability checks on
                // the shared /system/ namespace. The engine's script-update
                // stream is deliberately not exempt: it is broadcast to from
                // Rust (`engine_api::broadcast_script_update`), which never
                // passes through here, so exempting it only let any script
                // forge engine notifications to every subscriber.
                let is_system_broadcast = path.starts_with("/system/");

                if !is_system_broadcast {
                    // Check capability for non-system operations
                    if let Err(e) = user_ctx_send
                        .require_capability(&crate::security::Capability::ManageStreams)
                    {
                        let auditor_clone = auditor_send.clone();
                        let user_id = user_ctx_send.user_id.clone();
                        tokio::task::spawn(async move {
                            let _ = auditor_clone
                                .log_event(
                                    crate::security::SecurityEvent::new(
                                        crate::security::SecurityEventType::AuthorizationFailure,
                                        crate::security::SecuritySeverity::Medium,
                                        user_id,
                                    )
                                    .with_resource("stream".to_string())
                                    .with_action("send_message".to_string()),
                                )
                                .await;
                        });
                        return Ok(format!("Error: {}", e));
                    }
                }

                // Log the operation attempt
                let auditor_clone = auditor_send.clone();
                let user_id = user_ctx_send.user_id.clone();
                let path_clone = path.clone();
                let message_clone = message.clone();
                tokio::task::spawn(async move {
                    let _ = auditor_clone
                        .log_event(
                            crate::security::SecurityEvent::new(
                                crate::security::SecurityEventType::SystemSecurityEvent,
                                crate::security::SecuritySeverity::Low,
                                user_id,
                            )
                            .with_resource("stream".to_string())
                            .with_action("send_message".to_string())
                            .with_detail("path", &path_clone)
                            .with_detail("message_length", message_clone.len().to_string()),
                        )
                        .await;
                });

                // Send the message
                match crate::stream_registry::GLOBAL_STREAM_REGISTRY
                    .broadcast_to_stream(&path, &message)
                {
                    Ok(result) => {
                        if result.is_fully_successful() {
                            Ok(format!(
                                "Successfully sent message to {} connections on path '{}'",
                                result.successful_sends, path
                            ))
                        } else {
                            Ok(format!(
                                "Sent message to {}/{} connections on path '{}' ({} failed)",
                                result.successful_sends,
                                result.total_connections,
                                path,
                                result.failed_connections.len()
                            ))
                        }
                    }
                    Err(e) => Ok(format!("Failed to send message to path '{}': {}", path, e)),
                }
            },
        )?;
        route_registry.set("sendStreamMessage", send_stream_message)?;

        // 5. sendStreamMessageFiltered function
        let user_ctx_filtered = user_context.clone();
        let auditor_filtered = auditor.clone();
        let send_stream_message_filtered = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  path: String,
                  message: String,
                  filter_json: Option<String>,
                  match_mode: Option<String>|
                  -> JsResult<String> {
                // Parse filter criteria
                let metadata_filter: HashMap<String, String> = if let Some(json_str) = filter_json {
                    serde_json::from_str(&json_str).map_err(|e| {
                        rquickjs::Error::new_from_js_message(
                            "filter",
                            "MetadataFilter",
                            &format!("Invalid filter JSON: {}", e),
                        )
                    })?
                } else {
                    HashMap::new()
                };
                let match_mode = parse_filter_match_mode(match_mode)?;

                // Allow system-level broadcasting on the shared /system/
                // namespace only; see the note in `sendStreamMessage`.
                let is_system_broadcast = path.starts_with("/system/");

                if !is_system_broadcast
                    && let Err(e) = user_ctx_filtered
                        .require_capability(&crate::security::Capability::ManageStreams)
                {
                    let auditor_clone = auditor_filtered.clone();
                    let user_id = user_ctx_filtered.user_id.clone();
                    tokio::task::spawn(async move {
                        let _ = auditor_clone
                            .log_event(
                                crate::security::SecurityEvent::new(
                                    crate::security::SecurityEventType::AuthorizationFailure,
                                    crate::security::SecuritySeverity::Medium,
                                    user_id,
                                )
                                .with_resource("stream".to_string())
                                .with_action("send_filtered_message".to_string()),
                            )
                            .await;
                    });
                    return Ok(format!("Error: {}", e));
                }

                // Log the operation
                let auditor_clone = auditor_filtered.clone();
                let user_id = user_ctx_filtered.user_id.clone();
                let path_clone = path.clone();
                let message_clone = message.clone();
                let filter_clone = metadata_filter.clone();
                tokio::task::spawn(async move {
                    let _ = auditor_clone
                        .log_event(
                            crate::security::SecurityEvent::new(
                                crate::security::SecurityEventType::SystemSecurityEvent,
                                crate::security::SecuritySeverity::Low,
                                user_id,
                            )
                            .with_resource("stream".to_string())
                            .with_action("send_filtered_message".to_string())
                            .with_detail("path", &path_clone)
                            .with_detail("message_length", message_clone.len().to_string())
                            .with_detail("filter_criteria_count", filter_clone.len().to_string()),
                        )
                        .await;
                });

                // Send filtered message
                let result = crate::stream_registry::GLOBAL_STREAM_REGISTRY
                    .broadcast_to_stream_with_filter_mode(
                        &path,
                        &message,
                        &metadata_filter,
                        match_mode,
                    );

                match result {
                    Ok(broadcast_result) => {
                        if broadcast_result.is_fully_successful() {
                            Ok(format!(
                                "Successfully sent filtered message to {} connections on path '{}'",
                                broadcast_result.successful_sends, path
                            ))
                        } else {
                            Ok(format!(
                                "Sent filtered message to {}/{} connections on path '{}' ({} failed)",
                                broadcast_result.successful_sends,
                                broadcast_result.total_connections,
                                path,
                                broadcast_result.failed_connections.len()
                            ))
                        }
                    }
                    Err(e) => Ok(format!(
                        "Failed to send filtered message to path '{}': {}",
                        path, e
                    )),
                }
            },
        )?;
        route_registry.set("sendStreamMessageFiltered", send_stream_message_filtered)?;

        // Set the routeRegistry object on global scope
        global.set("routeRegistry", route_registry)?;

        Ok(())
    }

    /// Setup fetch() function for HTTP requests with secret injection
    fn setup_fetch_function(&self, ctx: &rquickjs::Ctx<'_>, script_uri: &str) -> JsResult<()> {
        let global = ctx.globals();
        let script_uri_owned = script_uri.to_string();
        // Capture the user_id at script setup time for secret lookup in user_secrets
        let user_id_for_fetch = self.user_context.user_id.clone();

        // Create the fetch function (synchronous version)
        let fetch_fn = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  url: String,
                  options_json: Option<String>|
                  -> JsResult<String> {
                // Parse options from JSON string
                let options: crate::http_client::FetchOptions = if let Some(json_str) = options_json
                {
                    serde_json::from_str(&json_str).map_err(|e| {
                        rquickjs::Error::new_from_js_message(
                            "options",
                            "FetchOptions",
                            &format!("Invalid fetch options: {}", e),
                        )
                    })?
                } else {
                    Default::default()
                };

                tracing::debug!("Fetching URL: {} from script: {}", url, script_uri_owned);

                // Create HTTP client
                let client = crate::http_client::HttpClient::new().map_err(|e| {
                    rquickjs::Error::new_from_js_message(
                        "fetch",
                        "client_init",
                        &format!("Failed to create HTTP client: {}", e),
                    )
                })?;

                // Perform the fetch (synchronous) with script_uri and user_id for secret resolution
                let response = client
                    .fetch(
                        url.clone(),
                        options,
                        Some(&script_uri_owned),
                        user_id_for_fetch.as_deref(),
                    )
                    .map_err(|e| {
                        rquickjs::Error::new_from_js_message(
                            "fetch",
                            "request_failed",
                            &format!("Fetch error: {}", e),
                        )
                    })?;

                // Convert response to JSON string
                let response_json = serde_json::to_string(&response).map_err(|e| {
                    rquickjs::Error::new_from_js_message(
                        "fetch",
                        "serialize",
                        &format!("Failed to serialize response: {}", e),
                    )
                })?;

                Ok(response_json)
            },
        )?;

        // The Rust half is installed under a private name. `fetch()` itself is
        // defined by the prelude below, which wraps this envelope in something
        // that can be awaited, read as an object, or parsed as the string this
        // used to return.
        global.set("__hostFetch", fetch_fn)?;

        // Compiled once per process and cached under a stable key, the way the
        // test prelude is. Installing it here covers every context that gets
        // host functions, rather than each entry point remembering to do it.
        crate::bytecode::eval_program(ctx, "engine://fetch-prelude", FETCH_PRELUDE).map_err(
            |e| {
                rquickjs::Error::new_from_js_message(
                    "fetch",
                    "prelude",
                    &format!("fetch prelude failed to load: {}", e),
                )
            },
        )?;

        debug!("fetch() function initialized with secret injection support");

        Ok(())
    }

    /// Setup database functions
    fn setup_database_functions(&self, ctx: &rquickjs::Ctx<'_>, script_uri: &str) -> JsResult<()> {
        let global = ctx.globals();
        let script_uri_owned = script_uri.to_string();
        let user_context = self.user_context.clone();

        // Create the database namespace object for schema management
        let database_obj = rquickjs::Object::new(ctx.clone())?;

        // database.createTable(tableName) - Create a new table for this script
        let script_uri_create = script_uri_owned.clone();
        let user_ctx_create = user_context.clone();
        let create_table = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, table_name: String| -> JsResult<String> {
                debug!(
                    "database.createTable called for script {} with table: {}",
                    script_uri_create, table_name
                );

                // Check permission
                if user_ctx_create
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database schema operations\"}"
                            .to_string(),
                    );
                }

                match crate::repository::create_script_table(&script_uri_create, &table_name) {
                    Ok(physical_name) => Ok(format!(
                        "{{\"success\": true, \"tableName\": \"{}\", \"physicalName\": \"{}\"}}",
                        table_name, physical_name
                    )),
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("createTable", create_table)?;

        // database.addIntegerColumn(tableName, columnName, nullable, defaultValue)
        let script_uri_add_int = script_uri_owned.clone();
        let user_ctx_add_int = user_context.clone();
        let add_integer_column = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  table_name: String,
                  column_name: String,
                  nullable: Opt<bool>,
                  default_value: Opt<String>|
                  -> JsResult<String> {
                debug!(
                    "database.addIntegerColumn called for script {}",
                    script_uri_add_int
                );

                if user_ctx_add_int
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database schema operations\"}"
                            .to_string(),
                    );
                }

                let nullable = nullable.0.unwrap_or(true);
                let default_val = default_value.0.as_deref();

                match crate::repository::add_column_to_script_table(
                    &script_uri_add_int,
                    &table_name,
                    &column_name,
                    crate::db_schema_utils::ColumnType::Integer,
                    nullable,
                    default_val,
                ) {
                    Ok(()) => Ok(format!(
                        "{{\"success\": true, \"column\": \"{}\"}}",
                        column_name
                    )),
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("addIntegerColumn", add_integer_column)?;

        // database.addTextColumn(tableName, columnName, nullable, defaultValue)
        let script_uri_add_text = script_uri_owned.clone();
        let user_ctx_add_text = user_context.clone();
        let add_text_column = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  table_name: String,
                  column_name: String,
                  nullable: Opt<bool>,
                  default_value: Opt<String>|
                  -> JsResult<String> {
                debug!(
                    "database.addTextColumn called for script {}",
                    script_uri_add_text
                );

                if user_ctx_add_text
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database schema operations\"}"
                            .to_string(),
                    );
                }

                let nullable = nullable.0.unwrap_or(true);
                let default_val = default_value.0.as_deref();

                match crate::repository::add_column_to_script_table(
                    &script_uri_add_text,
                    &table_name,
                    &column_name,
                    crate::db_schema_utils::ColumnType::Text,
                    nullable,
                    default_val,
                ) {
                    Ok(()) => Ok(format!(
                        "{{\"success\": true, \"column\": \"{}\"}}",
                        column_name
                    )),
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("addTextColumn", add_text_column)?;

        // database.addBooleanColumn(tableName, columnName, nullable, defaultValue)
        let script_uri_add_bool = script_uri_owned.clone();
        let user_ctx_add_bool = user_context.clone();
        let add_boolean_column = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  table_name: String,
                  column_name: String,
                  nullable: Opt<bool>,
                  default_value: Opt<String>|
                  -> JsResult<String> {
                debug!(
                    "database.addBooleanColumn called for script {}",
                    script_uri_add_bool
                );

                if user_ctx_add_bool
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database schema operations\"}"
                            .to_string(),
                    );
                }

                let nullable = nullable.0.unwrap_or(true);
                let default_val = default_value.0.as_deref();

                match crate::repository::add_column_to_script_table(
                    &script_uri_add_bool,
                    &table_name,
                    &column_name,
                    crate::db_schema_utils::ColumnType::Boolean,
                    nullable,
                    default_val,
                ) {
                    Ok(()) => Ok(format!(
                        "{{\"success\": true, \"column\": \"{}\"}}",
                        column_name
                    )),
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("addBooleanColumn", add_boolean_column)?;

        // database.addTimestampColumn(tableName, columnName, nullable, defaultValue)
        let script_uri_add_ts = script_uri_owned.clone();
        let user_ctx_add_ts = user_context.clone();
        let add_timestamp_column = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  table_name: String,
                  column_name: String,
                  nullable: Opt<bool>,
                  default_value: Opt<String>|
                  -> JsResult<String> {
                debug!(
                    "database.addTimestampColumn called for script {}",
                    script_uri_add_ts
                );

                if user_ctx_add_ts
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database schema operations\"}"
                            .to_string(),
                    );
                }

                let nullable = nullable.0.unwrap_or(true);
                let default_val = default_value.0.as_deref();

                match crate::repository::add_column_to_script_table(
                    &script_uri_add_ts,
                    &table_name,
                    &column_name,
                    crate::db_schema_utils::ColumnType::Timestamp,
                    nullable,
                    default_val,
                ) {
                    Ok(()) => Ok(format!(
                        "{{\"success\": true, \"column\": \"{}\"}}",
                        column_name
                    )),
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("addTimestampColumn", add_timestamp_column)?;

        // database.addReferenceColumn(tableName, columnName, referencedTableName, nullable)
        let script_uri_ref = script_uri_owned.clone();
        let user_ctx_ref = user_context.clone();
        let add_reference_column = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  table_name: String,
                  column_name: String,
                  referenced_table_name: String,
                  nullable: Opt<bool>|
                  -> JsResult<String> {
                debug!(
                    "database.addReferenceColumn called for script {}",
                    script_uri_ref
                );

                if user_ctx_ref
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database schema operations\"}"
                            .to_string(),
                    );
                }

                let nullable = nullable.0.unwrap_or(true);

                match crate::repository::add_reference_column(
                    &script_uri_ref,
                    &table_name,
                    &column_name,
                    &referenced_table_name,
                    nullable,
                ) {
                    Ok(()) => Ok(format!(
                        "{{\"success\": true, \"foreignKey\": \"{}.{} -> {}\", \"nullable\": {}}}",
                        table_name, column_name, referenced_table_name, nullable
                    )),
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("addReferenceColumn", add_reference_column)?;

        // database.dropColumn(tableName, columnName)
        let script_uri_drop_col = script_uri_owned.clone();
        let user_ctx_drop_col = user_context.clone();
        let drop_column = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  table_name: String,
                  column_name: String|
                  -> JsResult<String> {
                debug!(
                    "database.dropColumn called for script {} with table: {}, column: {}",
                    script_uri_drop_col, table_name, column_name
                );

                if user_ctx_drop_col
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database schema operations\"}"
                            .to_string(),
                    );
                }

                match crate::repository::drop_column(
                    &script_uri_drop_col,
                    &table_name,
                    &column_name,
                ) {
                    Ok(existed) => {
                        if existed {
                            Ok(format!(
                                "{{\"success\": true, \"tableName\": \"{}\", \"columnName\": \"{}\", \"dropped\": true}}",
                                table_name, column_name
                            ))
                        } else {
                            Ok(format!(
                                "{{\"success\": true, \"tableName\": \"{}\", \"columnName\": \"{}\", \"dropped\": false, \"message\": \"Column did not exist\"}}",
                                table_name, column_name
                            ))
                        }
                    }
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("dropColumn", drop_column)?;

        // database.dropTable(tableName)
        let script_uri_drop = script_uri_owned.clone();
        let user_ctx_drop = user_context.clone();
        let drop_table = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, table_name: String| -> JsResult<String> {
                debug!(
                    "database.dropTable called for script {} with table: {}",
                    script_uri_drop, table_name
                );

                if user_ctx_drop
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database schema operations\"}"
                            .to_string(),
                    );
                }

                match crate::repository::drop_script_table(&script_uri_drop, &table_name) {
                    Ok(existed) => {
                        if existed {
                            Ok(format!(
                                "{{\"success\": true, \"tableName\": \"{}\", \"dropped\": true}}",
                                table_name
                            ))
                        } else {
                            Ok(format!(
                                "{{\"success\": true, \"tableName\": \"{}\", \"dropped\": false, \"message\": \"Table did not exist\"}}",
                                table_name
                            ))
                        }
                    }
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("dropTable", drop_table)?;

        // database.query(tableName, filters, limit, orderBy, orderDir)
        // filters supports equality {"col": val} and range operators {"col": {"$gt": val, ...}}
        let script_uri_query = script_uri_owned.clone();
        let user_ctx_query = user_context.clone();
        let query_table = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  table_name: String,
                  filters: Opt<String>,
                  limit: Opt<i32>,
                  order_by: Opt<String>,
                  order_dir: Opt<String>|
                  -> JsResult<String> {
                debug!(
                    "database.query called for script {} on table: {}",
                    script_uri_query, table_name
                );

                if user_ctx_query
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database operations\"}"
                            .to_string(),
                    );
                }

                let filters_map = if let Some(filters_str) = filters.0 {
                    match serde_json::from_str::<std::collections::HashMap<String, serde_json::Value>>(
                        &filters_str,
                    ) {
                        Ok(map) => Some(map),
                        Err(e) => {
                            return Ok(format!("{{\"error\": \"Invalid filters JSON: {}\"}}", e));
                        }
                    }
                } else {
                    None
                };

                let limit_val = limit.0.map(|l| l as i64);
                let order_by_ref = order_by.0.as_deref();
                let order_dir_ref = order_dir.0.as_deref();

                match crate::repository::query_table(
                    &script_uri_query,
                    &table_name,
                    filters_map.as_ref(),
                    limit_val,
                    order_by_ref,
                    order_dir_ref,
                ) {
                    Ok(rows) => match serde_json::to_string(&rows) {
                        Ok(json) => Ok(json),
                        Err(e) => Ok(format!("{{\"error\": \"Serialization error: {}\"}}", e)),
                    },
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("query", query_table)?;

        // database.insert(tableName, data) - Insert a row
        let script_uri_insert = script_uri_owned.clone();
        let user_ctx_insert = user_context.clone();
        let insert_row = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, table_name: String, data: String| -> JsResult<String> {
                debug!(
                    "database.insert called for script {} on table: {}",
                    script_uri_insert, table_name
                );

                if user_ctx_insert
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database operations\"}"
                            .to_string(),
                    );
                }

                // Parse data from JSON string
                let data_map = match serde_json::from_str::<
                    std::collections::HashMap<String, serde_json::Value>,
                >(&data)
                {
                    Ok(map) => map,
                    Err(e) => return Ok(format!("{{\"error\": \"Invalid data JSON: {}\"}}", e)),
                };

                match crate::repository::insert_row(&script_uri_insert, &table_name, &data_map) {
                    Ok(row) => match serde_json::to_string(&row) {
                        Ok(json) => Ok(json),
                        Err(e) => Ok(format!("{{\"error\": \"Serialization error: {}\"}}", e)),
                    },
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("insert", insert_row)?;

        // database.update(tableName, id, data) - Update a row
        let script_uri_update = script_uri_owned.clone();
        let user_ctx_update = user_context.clone();
        let update_row = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  table_name: String,
                  id: i32,
                  data: String|
                  -> JsResult<String> {
                debug!(
                    "database.update called for script {} on table: {}, id: {}",
                    script_uri_update, table_name, id
                );

                if user_ctx_update
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database operations\"}"
                            .to_string(),
                    );
                }

                // Parse data from JSON string
                let data_map = match serde_json::from_str::<
                    std::collections::HashMap<String, serde_json::Value>,
                >(&data)
                {
                    Ok(map) => map,
                    Err(e) => return Ok(format!("{{\"error\": \"Invalid data JSON: {}\"}}", e)),
                };

                match crate::repository::update_row(&script_uri_update, &table_name, id, &data_map)
                {
                    Ok(row) => match serde_json::to_string(&row) {
                        Ok(json) => Ok(json),
                        Err(e) => Ok(format!("{{\"error\": \"Serialization error: {}\"}}", e)),
                    },
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("update", update_row)?;

        // database.delete(tableName, id) - Delete a row
        let script_uri_delete = script_uri_owned.clone();
        let user_ctx_delete = user_context.clone();
        let delete_row = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, table_name: String, id: i32| -> JsResult<String> {
                debug!(
                    "database.delete called for script {} on table: {}, id: {}",
                    script_uri_delete, table_name, id
                );

                if user_ctx_delete
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database operations\"}"
                            .to_string(),
                    );
                }

                match crate::repository::delete_row(&script_uri_delete, &table_name, id) {
                    Ok(deleted) => Ok(format!("{{\"success\": true, \"deleted\": {}}}", deleted)),
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("delete", delete_row)?;

        // database.upsert(tableName, keyColumns, data)
        // INSERT … ON CONFLICT DO UPDATE — atomically insert or update by key
        let script_uri_upsert = script_uri_owned.clone();
        let user_ctx_upsert = user_context.clone();
        let upsert_row = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  table_name: String,
                  key_columns_json: String,
                  data: String|
                  -> JsResult<String> {
                debug!(
                    "database.upsert called for script {} on table: {}",
                    script_uri_upsert, table_name
                );

                if user_ctx_upsert
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database operations\"}"
                            .to_string(),
                    );
                }

                // key_columns is a JSON array of strings, or a single string
                let key_cols: Vec<String> = match serde_json::from_str::<serde_json::Value>(
                    &key_columns_json,
                ) {
                    Ok(serde_json::Value::Array(arr)) => arr
                        .into_iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect(),
                    Ok(serde_json::Value::String(s)) => vec![s],
                    _ => {
                        return Ok("{\"error\": \"keyColumns must be a JSON array of strings or a single string\"}".to_string());
                    }
                };

                let data_map = match serde_json::from_str::<
                    std::collections::HashMap<String, serde_json::Value>,
                >(&data)
                {
                    Ok(map) => map,
                    Err(e) => return Ok(format!("{{\"error\": \"Invalid data JSON: {}\"}}", e)),
                };

                match crate::repository::upsert_row(
                    &script_uri_upsert,
                    &table_name,
                    &key_cols,
                    &data_map,
                ) {
                    Ok(row) => match serde_json::to_string(&row) {
                        Ok(json) => Ok(json),
                        Err(e) => Ok(format!("{{\"error\": \"Serialization error: {}\"}}", e)),
                    },
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("upsert", upsert_row)?;

        // database.deleteWhere(tableName, filters)
        // Bulk-delete rows matching filter conditions (equality + range operators)
        let script_uri_dw = script_uri_owned.clone();
        let user_ctx_dw = user_context.clone();
        let delete_where = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  table_name: String,
                  filters: String|
                  -> JsResult<String> {
                debug!(
                    "database.deleteWhere called for script {} on table: {}",
                    script_uri_dw, table_name
                );

                if user_ctx_dw
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database operations\"}"
                            .to_string(),
                    );
                }

                let filters_map = match serde_json::from_str::<
                    std::collections::HashMap<String, serde_json::Value>,
                >(&filters)
                {
                    Ok(map) => map,
                    Err(e) => return Ok(format!("{{\"error\": \"Invalid filters JSON: {}\"}}", e)),
                };

                match crate::repository::delete_where(&script_uri_dw, &table_name, &filters_map) {
                    Ok(count) => Ok(format!("{{\"success\": true, \"deleted\": {}}}", count)),
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("deleteWhere", delete_where)?;

        // database.acquireLease(tableName, leaseId, owner, ttlMs)
        // Atomic compare-and-swap lease acquisition using a script-owned lease table
        let script_uri_lease = script_uri_owned.clone();
        let user_ctx_lease = user_context.clone();
        let acquire_lease = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  table_name: String,
                  lease_id: String,
                  owner: String,
                  ttl_ms: i64|
                  -> JsResult<String> {
                debug!(
                    "database.acquireLease called for script {} on table: {}, lease: {}",
                    script_uri_lease, table_name, lease_id
                );

                if user_ctx_lease
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database operations\"}"
                            .to_string(),
                    );
                }

                match crate::repository::acquire_lease(
                    &script_uri_lease,
                    &table_name,
                    &lease_id,
                    &owner,
                    ttl_ms,
                ) {
                    Ok(result) => match serde_json::to_string(&result) {
                        Ok(json) => Ok(json),
                        Err(e) => Ok(format!("{{\"error\": \"Serialization error: {}\"}}", e)),
                    },
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("acquireLease", acquire_lease)?;

        // database.createLeaseTable(tableName)
        // Create a correctly-structured lease table with a UNIQUE constraint on lease_id
        let script_uri_clt = script_uri_owned.clone();
        let user_ctx_clt = user_context.clone();
        let create_lease_table = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, table_name: String| -> JsResult<String> {
                debug!(
                    "database.createLeaseTable called for script {} with table: {}",
                    script_uri_clt, table_name
                );

                if user_ctx_clt
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database schema operations\"}"
                            .to_string(),
                    );
                }

                match crate::repository::create_lease_table(&script_uri_clt, &table_name) {
                    Ok(physical_name) => Ok(format!(
                        "{{\"success\": true, \"tableName\": \"{}\", \"physicalName\": \"{}\"}}",
                        table_name, physical_name
                    )),
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("createLeaseTable", create_lease_table)?;

        // database.addUniqueIndex(tableName, columns)
        // Add a unique index to enable upsert() with a conflict target
        let script_uri_idx = script_uri_owned.clone();
        let user_ctx_idx = user_context.clone();
        let add_unique_index = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  table_name: String,
                  columns_json: String|
                  -> JsResult<String> {
                debug!(
                    "database.addUniqueIndex called for script {} on table: {}",
                    script_uri_idx, table_name
                );

                if user_ctx_idx
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database schema operations\"}"
                            .to_string(),
                    );
                }

                let columns: Vec<String> = match serde_json::from_str::<serde_json::Value>(
                    &columns_json,
                ) {
                    Ok(serde_json::Value::Array(arr)) => arr
                        .into_iter()
                        .filter_map(|v| v.as_str().map(|s| s.to_string()))
                        .collect(),
                    Ok(serde_json::Value::String(s)) => vec![s],
                    _ => {
                        return Ok(
                                "{\"error\": \"columns must be a JSON array of strings or a single string\"}"
                                    .to_string(),
                            );
                    }
                };

                match crate::repository::add_unique_index(&script_uri_idx, &table_name, &columns) {
                    Ok(()) => Ok(format!(
                        "{{\"success\": true, \"tableName\": \"{}\", \"columns\": {}}}",
                        table_name, columns_json
                    )),
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("addUniqueIndex", add_unique_index)?;

        // database.generateGraphQLForTable
        let script_uri_graphql = script_uri_owned.clone();
        let user_ctx_graphql = user_context.clone();
        let config_graphql = self.config.clone();
        let generate_graphql = Function::new(
            ctx.clone(),
            move |ctx_inner: rquickjs::Ctx<'_>,
                  table_name: String,
                  options: Opt<String>|
                  -> JsResult<String> {
                debug!(
                    "database.generateGraphQLForTable called for script {} on table: {}",
                    script_uri_graphql, table_name
                );

                if user_ctx_graphql
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(
                        "{\"error\": \"Insufficient permissions for database operations\"}"
                            .to_string(),
                    );
                }

                // Parse options (default: ScriptInternal visibility)
                let visibility = if let Some(opts_str) = options.0 {
                    match serde_json::from_str::<serde_json::Value>(&opts_str) {
                        Ok(opts) => opts
                            .get("visibility")
                            .and_then(|v| v.as_str())
                            .unwrap_or("script_internal")
                            .to_string(),
                        Err(_) => "script_internal".to_string(),
                    }
                } else {
                    "script_internal".to_string()
                };

                // Get table schema
                let schema =
                    match crate::repository::get_table_schema(&script_uri_graphql, &table_name) {
                        Ok(s) => s,
                        Err(e) => {
                            return Ok(format!(
                                "{{\"error\": \"Failed to get table schema: {}\"}}",
                                e
                            ));
                        }
                    };

                // Get foreign keys
                let foreign_keys =
                    match crate::repository::get_foreign_keys(&script_uri_graphql, &table_name) {
                        Ok(fks) => fks,
                        Err(e) => {
                            return Ok(format!(
                                "{{\"error\": \"Failed to get foreign keys: {}\"}}",
                                e
                            ));
                        }
                    };

                // Generate GraphQL operations
                let operations = crate::graphql_schema_gen::generate_table_operations(
                    &table_name,
                    &schema,
                    &foreign_keys,
                );

                // Inject resolver functions into JavaScript context
                for query in &operations.queries {
                    // Evaluate resolver code in the current context
                    if let Err(e) = ctx_inner.eval::<(), _>(query.resolver_code.as_str()) {
                        return Ok(format!(
                            "{{\"error\": \"Failed to inject resolver {}: {:?}\"}}",
                            query.resolver_function_name, e
                        ));
                    }
                }

                for mutation in &operations.mutations {
                    if let Err(e) = ctx_inner.eval::<(), _>(mutation.resolver_code.as_str()) {
                        return Ok(format!(
                            "{{\"error\": \"Failed to inject resolver {}: {:?}\"}}",
                            mutation.resolver_function_name, e
                        ));
                    }
                }

                // Register queries
                if config_graphql.is_dry_run() {
                    for query in &operations.queries {
                        config_graphql.collect(
                            CollectedRegistration::new(
                                RegistrationKind::GraphqlQuery,
                                query.name.clone(),
                            )
                            .with_handler(query.resolver_function_name.clone()),
                        );
                    }
                    for mutation in &operations.mutations {
                        config_graphql.collect(
                            CollectedRegistration::new(
                                RegistrationKind::GraphqlMutation,
                                mutation.name.clone(),
                            )
                            .with_handler(mutation.resolver_function_name.clone()),
                        );
                    }
                    return Ok(format!(
                        "{{\"dryRun\": true, \"table\": \"{}\"}}",
                        table_name
                    ));
                }

                for query in &operations.queries {
                    if let Err(e) = crate::graphql::register_graphql_query(
                        query.name.clone(),
                        query.sdl.clone(),
                        query.resolver_function_name.clone(),
                        script_uri_graphql.clone(),
                        visibility.clone(),
                    ) {
                        return Ok(format!(
                            "{{\"error\": \"Failed to register query {}: {}\"}}",
                            query.name, e
                        ));
                    }
                }

                // Register mutations
                for mutation in &operations.mutations {
                    if let Err(e) = crate::graphql::register_graphql_mutation(
                        mutation.name.clone(),
                        mutation.sdl.clone(),
                        mutation.resolver_function_name.clone(),
                        script_uri_graphql.clone(),
                        visibility.clone(),
                    ) {
                        return Ok(format!(
                            "{{\"error\": \"Failed to register mutation {}: {}\"}}",
                            mutation.name, e
                        ));
                    }
                }

                // Return success with operation names
                let query_names: Vec<&str> =
                    operations.queries.iter().map(|q| q.name.as_str()).collect();
                let mutation_names: Vec<&str> = operations
                    .mutations
                    .iter()
                    .map(|m| m.name.as_str())
                    .collect();

                Ok(format!(
                    "{{\"success\": true, \"table\": \"{}\", \"queries\": {:?}, \"mutations\": {:?}}}",
                    table_name, query_names, mutation_names
                ))
            },
        )?;
        database_obj.set("generateGraphQLForTable", generate_graphql)?;

        // Transaction management functions

        // database.beginTransaction(timeoutMs?) - Start a new transaction or savepoint
        let begin_transaction = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, timeout_ms: Opt<u64>| -> JsResult<String> {
                match crate::database::Database::begin_transaction(timeout_ms.0) {
                    Ok(guard) => {
                        // The transaction has to outlive this call: the script
                        // expects it open on the next line, and the handler
                        // boundary commits or rolls it back. Dropping the guard
                        // here would roll it back immediately instead, leaving
                        // every write the script went on to make outside any
                        // transaction and nothing for `rollbackTransaction` to
                        // undo.
                        guard.release();
                        Ok("{\"success\": true, \"message\": \"Transaction started\"}".to_string())
                    }
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("beginTransaction", begin_transaction)?;

        // database.commitTransaction() - Commit the current transaction or release savepoint
        let commit_transaction = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
                match crate::database::Database::commit_transaction() {
                    Ok(()) => Ok(
                        "{\"success\": true, \"message\": \"Transaction committed\"}".to_string(),
                    ),
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("commitTransaction", commit_transaction)?;

        // database.rollbackTransaction() - Rollback the current transaction or to savepoint
        let rollback_transaction = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
                match crate::database::Database::rollback_transaction() {
                    Ok(()) => Ok(
                        "{\"success\": true, \"message\": \"Transaction rolled back\"}".to_string(),
                    ),
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("rollbackTransaction", rollback_transaction)?;

        // database.createSavepoint(name?) - Create a named or auto-generated savepoint
        let create_savepoint = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, name: Opt<String>| -> JsResult<String> {
                match crate::database::Database::create_savepoint(name.0.as_deref()) {
                    Ok(savepoint_name) => Ok(format!(
                        "{{\"success\": true, \"savepoint\": \"{}\"}}",
                        savepoint_name
                    )),
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("createSavepoint", create_savepoint)?;

        // database.rollbackToSavepoint(name) - Rollback to a named savepoint
        let rollback_to_savepoint = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, name: String| -> JsResult<String> {
                match crate::database::Database::rollback_to_savepoint(&name) {
                    Ok(()) => Ok(format!(
                        "{{\"success\": true, \"message\": \"Rolled back to savepoint: {}\"}}",
                        name
                    )),
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("rollbackToSavepoint", rollback_to_savepoint)?;

        // database.releaseSavepoint(name) - Release a named savepoint
        let release_savepoint = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, name: String| -> JsResult<String> {
                match crate::database::Database::release_savepoint(&name) {
                    Ok(()) => Ok(format!(
                        "{{\"success\": true, \"message\": \"Released savepoint: {}\"}}",
                        name
                    )),
                    Err(e) => Ok(format!("{{\"error\": \"{}\"}}", e)),
                }
            },
        )?;
        database_obj.set("releaseSavepoint", release_savepoint)?;

        // Installed under a private name: the prelude below builds `database`
        // from it, wrapping each answer so a result can be awaited and read
        // the same way a `fetch` response is.
        global.set("__hostDatabase", database_obj)?;

        crate::bytecode::eval_program(ctx, "engine://result-prelude", RESULT_PRELUDE).map_err(
            |e| {
                rquickjs::Error::new_from_js_message(
                    "database",
                    "prelude",
                    &format!("result prelude failed to load: {}", e),
                )
            },
        )?;

        debug!(
            "database JavaScript API initialized for script: {}",
            script_uri
        );

        Ok(())
    }

    /// Setup conversion functions (markdown to HTML, etc.)
    fn setup_conversion_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        _script_uri: &str,
    ) -> JsResult<()> {
        let global = ctx.globals();

        // Create the convert namespace object
        let convert_obj = rquickjs::Object::new(ctx.clone())?;

        // convert.markdown_to_html(markdown) - Convert markdown string to HTML
        let markdown_to_html = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, markdown: String| -> JsResult<String> {
                // Call the conversion function
                match crate::conversion::convert_markdown_to_html(&markdown) {
                    Ok(html) => Ok(html),
                    Err(e) => {
                        // Return error as string (following pattern of other APIs)
                        Ok(format!("Error: {}", e))
                    }
                }
            },
        )?;

        // convert.render_handlebars_template(template, data) - Render Handlebars template
        let render_handlebars_template = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, template: String, data: String| -> JsResult<String> {
                // Call the conversion function
                match crate::conversion::render_handlebars_template(&template, &data) {
                    Ok(rendered) => Ok(rendered),
                    Err(e) => {
                        // Return error as string (following pattern of other APIs)
                        Ok(format!("Error: {}", e))
                    }
                }
            },
        )?;

        // convert.btoa(data) - Base64 encode a string (string-only)
        let btoa = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, input: rquickjs::Value| -> JsResult<String> {
                let Some(input_str) = input.as_string() else {
                    return Err(rquickjs::Error::new_from_js_message(
                        "btoa",
                        "type_error",
                        "btoa() expects a string parameter",
                    ));
                };

                let input_str = input_str.to_string().map_err(|e| {
                    rquickjs::Error::new_from_js_message(
                        "btoa",
                        "type_error",
                        &format!("btoa() expects a string parameter: {}", e),
                    )
                })?;

                crate::conversion::convert_btoa(&input_str).map_err(|e| {
                    rquickjs::Error::new_from_js_message(
                        "btoa",
                        "invalid_input",
                        &format!("Invalid input: {}", e),
                    )
                })
            },
        )?;

        // convert.atob(data) - Base64 decode a string (string-only)
        let atob = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, input: rquickjs::Value| -> JsResult<String> {
                let Some(input_str) = input.as_string() else {
                    return Err(rquickjs::Error::new_from_js_message(
                        "atob",
                        "type_error",
                        "atob() expects a string parameter",
                    ));
                };

                let input_str = input_str.to_string().map_err(|e| {
                    rquickjs::Error::new_from_js_message(
                        "atob",
                        "type_error",
                        &format!("atob() expects a string parameter: {}", e),
                    )
                })?;

                crate::conversion::convert_atob(&input_str).map_err(|e| {
                    rquickjs::Error::new_from_js_message(
                        "atob",
                        "invalid_input",
                        &format!("Invalid input: {}", e),
                    )
                })
            },
        )?;

        convert_obj.set("markdown_to_html", markdown_to_html)?;
        convert_obj.set("render_handlebars_template", render_handlebars_template)?;
        convert_obj.set("btoa", btoa)?;
        convert_obj.set("atob", atob)?;
        global.set("convert", convert_obj)?;

        debug!(
            "convert.markdown_to_html() and convert.render_handlebars_template() functions initialized"
        );

        Ok(())
    }

    /// The user the current invocation is running as, if any.
    ///
    /// Personal storage is keyed by it, and every one of its methods needs the
    /// same answer, so the walk down `context.request.auth` lives here rather
    /// than four times over. `None` means there is nobody to store anything
    /// for — which the prelude turns into a `SecurityError`, as a browser does
    /// when storage is not available to the caller.
    fn current_user_id(ctx: &rquickjs::Ctx<'_>) -> Option<String> {
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

    /// The error envelope the storage prelude turns into a `DOMException`.
    ///
    /// `name` is the exception's, so the failure a script catches says which
    /// kind it was rather than being prose it would have to match on.
    fn storage_failure(name: &str, message: &str) -> String {
        serde_json::json!({ "name": name, "message": message }).to_string()
    }

    /// Classifies a write that the repository refused.
    ///
    /// Size is the one a script can do something about, and the one the Web
    /// Storage spec names, so it keeps its own exception; anything else is the
    /// store being unable to answer.
    fn storage_write_failure(error: &crate::error::AppError) -> String {
        let message = error.to_string();
        if message.contains("too large") {
            Self::storage_failure("QuotaExceededError", &message)
        } else {
            Self::storage_failure("UnknownError", &message)
        }
    }

    /// Setup secure script storage functions
    fn setup_script_properties_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
    ) -> JsResult<()> {
        let global = ctx.globals();
        let script_uri_owned = script_uri.to_string();

        // The Rust half of `sharedStorage`. Every method here answers with a
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
                    "sharedStorage.getItem called for script {} with key: {}",
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
                    "sharedStorage.setItem called for script {} with key: {}",
                    script_uri_set, key
                );

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
            move |_ctx: rquickjs::Ctx<'_>, key: String| -> JsResult<()> {
                debug!(
                    "sharedStorage.removeItem called for script {} with key: {}",
                    script_uri_remove, key
                );
                // Whether the key was there is not something `removeItem`
                // reports, in the browser or here.
                crate::repository::remove_script_properties_item(&script_uri_remove, &key);
                Ok(())
            },
        )?;
        host.set("removeItem", remove_item)?;

        let script_uri_clear = script_uri_owned.clone();
        let clear_storage = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<Option<String>> {
                debug!("sharedStorage.clear called for script {}", script_uri_clear);
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
                Ok(crate::repository::list_script_properties_keys(
                    &script_uri_keys,
                ))
            },
        )?;
        host.set("keys", keys)?;

        // Always available: shared storage belongs to the script, not to a
        // user, so there is nobody who could be missing.
        let available =
            Function::new(ctx.clone(), move |_ctx: rquickjs::Ctx<'_>| -> bool { true })?;
        host.set("available", available)?;

        global.set("__hostSharedStorage", host)?;

        debug!(
            "sharedStorage host functions initialized for script: {}",
            script_uri
        );

        Ok(())
    }

    fn setup_user_properties_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
    ) -> JsResult<()> {
        let global = ctx.globals();
        let script_uri_owned = script_uri.to_string();

        // The Rust half of `personalStorage`. It differs from shared storage in
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
                let Some(user_id) = Self::current_user_id(&ctx) else {
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

                let Some(user_id) = Self::current_user_id(&ctx) else {
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
                let Some(user_id) = Self::current_user_id(&ctx) else {
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
                let Some(user_id) = Self::current_user_id(&ctx) else {
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
                let Some(user_id) = Self::current_user_id(&ctx) else {
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
            Self::current_user_id(&ctx).is_some()
        })?;
        host.set("available", available)?;

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
            "sharedStorage and personalStorage initialized for script: {}",
            script_uri
        );

        Ok(())
    }

    fn setup_scheduler_functions(&self, ctx: &rquickjs::Ctx<'_>, script_uri: &str) -> JsResult<()> {
        // `schedulerService` used to be omitted entirely outside the
        // registration phase, which made a shared helper that touches it throw
        // `ReferenceError` when reached from a message listener or a test. The
        // object is now always present; the three registration methods below
        // are the part that depends on the phase.
        let global = ctx.globals();
        let scheduler_obj = rquickjs::Object::new(ctx.clone())?;
        let scheduler_handle = scheduler::get_scheduler();

        let register_once_handle = scheduler_handle.clone();
        let script_uri_once = script_uri.to_string();
        let config_once = self.config.clone();
        let register_once =
            Function::new(
                ctx.clone(),
                move |_ctx: rquickjs::Ctx<'_>, options: rquickjs::Object| -> JsResult<String> {
                    let handler: String = match options.get("handler") {
                        Ok(value) => value,
                        Err(_) => {
                            return Ok("schedulerService.registerOnce requires options.handler"
                                .to_string());
                        }
                    };
                    let handler_name = handler.trim();
                    if handler_name.is_empty() {
                        return Ok(
                            "schedulerService.registerOnce requires a non-empty handler name"
                                .to_string(),
                        );
                    }

                    if !config_once.registration_phase {
                        return Ok(registration_inactive(
                            "schedulerService.registerOnce",
                            handler_name,
                        ));
                    }

                    let run_at_value: String = match options.get("runAt") {
                        Ok(value) => value,
                        Err(_) => return Ok(
                            "schedulerService.registerOnce requires options.runAt (UTC ISO string)"
                                .to_string(),
                        ),
                    };
                    let run_at = match scheduler::parse_utc_timestamp(&run_at_value) {
                        Ok(ts) => ts,
                        Err(err) => return Ok(format!("Scheduler error: {}", err)),
                    };

                    let name = options.get::<_, String>("name").ok();

                    if let Some(reply) = config_once.collect(
                        CollectedRegistration::new(
                            RegistrationKind::ScheduledJob,
                            name.clone().unwrap_or_else(|| handler_name.to_string()),
                        )
                        .with_handler(handler_name),
                    ) {
                        return Ok(reply);
                    }

                    match register_once_handle.register_one_off(
                        &script_uri_once,
                        handler_name,
                        name,
                        run_at,
                    ) {
                        Ok(job) => Ok(format!(
                            "Scheduled one-time job '{}' for {} (id {})",
                            job.key,
                            job.schedule.next_run().to_rfc3339(),
                            job.id
                        )),
                        Err(err) => Ok(format!("Scheduler error: {}", err)),
                    }
                },
            )?;

        let register_recurring_handle = scheduler_handle.clone();
        let script_uri_recurring = script_uri.to_string();
        let config_recurring = self.config.clone();
        let register_recurring = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, options: rquickjs::Object| -> JsResult<String> {
                let handler: String = match options.get("handler") {
                    Ok(value) => value,
                    Err(_) => {
                        return Ok(
                            "schedulerService.registerRecurring requires options.handler"
                                .to_string(),
                        );
                    }
                };
                let handler_name = handler.trim();
                if handler_name.is_empty() {
                    return Ok(
                        "schedulerService.registerRecurring requires a non-empty handler name"
                            .to_string(),
                    );
                }

                if !config_recurring.registration_phase {
                    return Ok(registration_inactive(
                        "schedulerService.registerRecurring",
                        handler_name,
                    ));
                }

                let interval_ms_opt = options.get::<_, f64>("intervalMilliseconds").ok();
                let interval_min_opt = options.get::<_, f64>("intervalMinutes").ok();

                if interval_ms_opt.is_some() && interval_min_opt.is_some() {
                    return Ok(
                        "schedulerService.registerRecurring accepts either intervalMilliseconds or intervalMinutes, not both"
                            .to_string(),
                    );
                }

                let (interval, interval_label) = if let Some(interval_ms_value) = interval_ms_opt {
                    if !interval_ms_value.is_finite() || interval_ms_value < 100.0 {
                        return Ok(
                            "schedulerService.registerRecurring requires intervalMilliseconds >= 100"
                                .to_string(),
                        );
                    }
                    let interval_ms = interval_ms_value.floor() as i64;
                    (
                        ChronoDuration::milliseconds(interval_ms),
                        format!("{} ms", interval_ms),
                    )
                } else if let Some(interval_min_value) = interval_min_opt {
                    if !interval_min_value.is_finite() || interval_min_value < 1.0 {
                        return Ok(
                            "schedulerService.registerRecurring requires intervalMinutes >= 1"
                                .to_string(),
                        );
                    }
                    let interval_minutes = interval_min_value.floor() as i64;
                    (
                        ChronoDuration::minutes(interval_minutes),
                        format!("{} minute(s)", interval_minutes),
                    )
                } else {
                    return Ok(
                        "schedulerService.registerRecurring requires intervalMilliseconds or intervalMinutes"
                            .to_string(),
                    );
                };

                let name = options.get::<_, String>("name").ok();
                let first_run = if let Ok(start_at) = options.get::<_, String>("startAt") {
                    match scheduler::parse_utc_timestamp(&start_at) {
                        Ok(ts) => Some(ts),
                        Err(err) => return Ok(format!("Scheduler error: {}", err)),
                    }
                } else {
                    None
                };

                if let Some(reply) = config_recurring.collect(
                    CollectedRegistration::new(
                        RegistrationKind::ScheduledJob,
                        name.clone().unwrap_or_else(|| handler_name.to_string()),
                    )
                    .with_handler(handler_name),
                ) {
                    return Ok(reply);
                }

                match register_recurring_handle.register_recurring(
                    &script_uri_recurring,
                    handler_name,
                    name,
                    interval,
                    first_run,
                ) {
                    Ok(job) => Ok(format!(
                        "Scheduled recurring job '{}' every {}; next run {} (id {})",
                        job.key,
                        interval_label,
                        job.schedule.next_run().to_rfc3339(),
                        job.id
                    )),
                    Err(err) => Ok(format!("Scheduler error: {}", err)),
                }
            },
        )?;

        let script_uri_clear = script_uri.to_string();
        let config_clear = self.config.clone();
        let clear_all = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
                // Clearing is the inverse of registering and mutates the same
                // registry, so it follows the same phase rule.
                if !config_clear.registration_phase {
                    return Ok(
                        "schedulerService.clearAll: no jobs cleared - scheduled job changes \
                         only take effect during script startup and init()"
                            .to_string(),
                    );
                }

                if config_clear.is_dry_run() {
                    // Clearing mutates the live scheduler exactly as registering
                    // does, and there is nothing to record: a dry run reports
                    // what a script *would* register, and an emptied job table
                    // is not part of that.
                    return Ok(format!(
                        "schedulerService.clearAll: no jobs cleared for {} - this is a dry run",
                        script_uri_clear
                    ));
                }

                let removed = scheduler::clear_script_jobs(&script_uri_clear);
                Ok(format!(
                    "Cleared {} scheduled job(s) for {}",
                    removed, script_uri_clear
                ))
            },
        )?;

        scheduler_obj.set("registerOnce", register_once)?;
        scheduler_obj.set("registerRecurring", register_recurring)?;
        scheduler_obj.set("clearAll", clear_all)?;
        global.set("schedulerService", scheduler_obj)?;

        Ok(())
    }

    /// Setup message dispatcher functions for inter-script communication
    fn setup_dispatcher_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
    ) -> JsResult<()> {
        let global = ctx.globals();
        let dispatcher_obj = rquickjs::Object::new(ctx.clone())?;

        // registerListener(messageType, handlerName)
        let script_uri_register = script_uri.to_string();
        let config_register = self.config.clone();
        let register_listener = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  message_type: String,
                  handler_name: String|
                  -> JsResult<String> {
                // Validate inputs
                if message_type.is_empty() {
                    return Ok(
                        "dispatcher.registerListener: message type cannot be empty".to_string()
                    );
                }
                if handler_name.is_empty() {
                    return Ok(
                        "dispatcher.registerListener: handler name cannot be empty".to_string()
                    );
                }

                // The dispatcher appends listeners without de-duplicating, so a
                // registration made outside the registration phase added one
                // more copy of the same listener on every invocation - a script
                // registering at top level ended up handling each message once
                // per request it had ever served. Same phase rule as every
                // other registry.
                if !config_register.registration_phase {
                    return Ok(registration_inactive(
                        "dispatcher.registerListener",
                        &message_type,
                    ));
                }

                if let Some(reply) = config_register.collect(
                    CollectedRegistration::new(
                        RegistrationKind::MessageListener,
                        message_type.clone(),
                    )
                    .with_handler(handler_name.clone()),
                ) {
                    return Ok(reply);
                }

                // Register the listener
                match crate::dispatcher::GLOBAL_DISPATCHER.register_listener(
                    message_type.clone(),
                    script_uri_register.clone(),
                    handler_name.clone(),
                ) {
                    Ok(()) => {
                        debug!(
                            "Registered listener for message type '{}' in script '{}': handler={}",
                            message_type, script_uri_register, handler_name
                        );
                        Ok(format!(
                            "Registered listener for message type '{}': handler '{}'",
                            message_type, handler_name
                        ))
                    }
                    Err(e) => {
                        error!(
                            "Failed to register listener for message type '{}' in script '{}': {}",
                            message_type, script_uri_register, e
                        );
                        Ok(format!("Failed to register listener: {}", e))
                    }
                }
            },
        )?;

        // sendMessage(messageType, messageData)
        // Note: messageData should be a JSON string or will be converted to empty object
        let config_send = self.config.clone();
        let send_message = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  message_type: String,
                  message_data_json: Opt<String>|
                  -> JsResult<String> {
                // Validate message type
                if message_type.is_empty() {
                    return Ok("dispatcher.sendMessage: message type cannot be empty".to_string());
                }

                if config_send.is_dry_run() {
                    // Dispatching runs *other* scripts' listeners against live
                    // data, and no transaction rolls that back. A check that
                    // deploys nothing must not set the rest of the engine in
                    // motion either.
                    return Ok(format!(
                        "dispatcher.sendMessage: '{}' not dispatched - this is a dry run",
                        message_type
                    ));
                }

                // Get message data as JSON string
                let message_data_json = message_data_json.0.unwrap_or_else(|| "{}".to_string());

                // Get listeners for this message type
                let listeners =
                    match crate::dispatcher::GLOBAL_DISPATCHER.get_listeners(&message_type) {
                        Ok(listeners) => listeners,
                        Err(e) => {
                            error!(
                                "Failed to get listeners for message type '{}': {}",
                                message_type, e
                            );
                            return Ok(format!("Failed to get listeners: {}", e));
                        }
                    };

                if listeners.is_empty() {
                    debug!(
                        "No listeners registered for message type '{}'",
                        message_type
                    );
                    return Ok(format!("No listeners for message type '{}'", message_type));
                }

                debug!(
                    "Dispatching message type '{}' to {} listener(s)",
                    message_type,
                    listeners.len()
                );

                // Invoke each listener handler
                let mut successful = 0;
                let mut failed = 0;

                for listener in listeners.iter() {
                    debug!(
                        "Invoking handler '{}' in script '{}' for message type '{}'",
                        listener.handler_name, listener.script_uri, message_type
                    );

                    // Load the script content
                    let script_content = match repository::fetch_script(&listener.script_uri) {
                        Some(content) => content,
                        None => {
                            warn!(
                                "Script '{}' not found for handler '{}'",
                                listener.script_uri, listener.handler_name
                            );
                            failed += 1;
                            continue;
                        }
                    };

                    // Execute the handler in a new context
                    match execute_message_handler(
                        listener.script_uri.clone(),
                        &script_content,
                        &listener.handler_name,
                        &message_type,
                        &message_data_json,
                    ) {
                        Ok(_) => {
                            debug!(
                                "Successfully invoked handler '{}' in script '{}'",
                                listener.handler_name, listener.script_uri
                            );
                            successful += 1;
                        }
                        Err(e) => {
                            error!(
                                "Failed to invoke handler '{}' in script '{}': {}",
                                listener.handler_name, listener.script_uri, e
                            );
                            failed += 1;
                        }
                    }
                }

                Ok(format!(
                    "Dispatched message type '{}': {} successful, {} failed",
                    message_type, successful, failed
                ))
            },
        )?;

        dispatcher_obj.set("registerListener", register_listener)?;
        dispatcher_obj.set("sendMessage", send_message)?;
        global.set("dispatcher", dispatcher_obj)?;

        debug!("Dispatcher functions initialized");
        Ok(())
    }
}

/// Extract the authenticated user_id from JavaScript `context.request.auth`.
/// Returns `None` if context is missing, request is missing, auth is missing,
/// or the user is not authenticated.
fn get_auth_user_id(globals: &rquickjs::Object<'_>) -> Option<String> {
    let context_obj: rquickjs::Object = globals.get("context").ok()?;
    let request_obj: rquickjs::Object = context_obj.get("request").ok()?;
    let auth_obj: rquickjs::Object = request_obj.get("auth").ok()?;
    let is_authenticated: bool = auth_obj.get("isAuthenticated").unwrap_or_default();
    if !is_authenticated {
        return None;
    }
    auth_obj.get("userId").ok().flatten()
}

/// Execute a message handler function in a script
fn execute_message_handler(
    script_uri: String,
    script_content: &str,
    handler_name: &str,
    message_type: &str,
    message_data_json: &str,
) -> Result<(), String> {
    use rquickjs::{Context, Runtime};

    // Create a new runtime and context for handler execution
    let rt = Runtime::new().map_err(|e| format!("Failed to create runtime: {}", e))?;
    let ctx = Context::full(&rt).map_err(|e| format!("Failed to create context: {}", e))?;

    let setup = ctx.with(|ctx| -> Result<(), String> {
        // Set up minimal secure global functions for handler execution
        let user_context = UserContext::admin("dispatcher".to_string());
        // A listener is a plain handler invocation: it gets the same globals as
        // any other, and only registration is off. It used to run without
        // `assetStorage`, `secretStorage` or `schedulerService` in scope at all,
        // which made shared helpers fail with `ReferenceError` depending on
        // which entry point reached them.
        let security_config = GlobalSecurityConfig {
            registration_phase: false,
            enable_audit_logging: false,
            dry_run_sink: None,
            console_sink: None,
            // A dispatched message is its own invocation: without this its
            // output is indistinguishable from whatever request happened to
            // send the message.
            log_context: crate::js_engine::HandlerInvocationKind::MessageListener.log_context(
                crate::middleware::generate_request_id(),
                Some(message_type.to_string()),
            ),
        };

        let secure_context = SecureGlobalContext::new_with_config(user_context, security_config);
        secure_context
            .setup_secure_functions(&ctx, &script_uri, None)
            .map_err(|e| format!("Failed to setup secure functions: {}", e))?;

        // Evaluate the script
        ctx.eval::<(), _>(script_content)
            .map_err(|e| format!("Script evaluation failed: {}", e))?;

        Ok(())
    });
    setup.map_err(|e| format!("Context execution failed: {}", e))?;

    crate::js_engine::call_and_settle(
        &rt,
        &ctx,
        &script_uri,
        &format!("Message listener '{}'", handler_name),
        crate::js_engine::TransactionHandling::Auto,
        |ctx| {
            // Parse message data back to JavaScript value
            let message_data_value: rquickjs::Value = ctx
                .json_parse(message_data_json)
                .map_err(|e| format!("Failed to parse message data: {}", e))?;

            // Create context object with message data
            let context_obj = rquickjs::Object::new(ctx.clone())
                .map_err(|e| format!("Failed to create context object: {}", e))?;
            context_obj
                .set("messageType", message_type)
                .map_err(|e| format!("Failed to set messageType: {}", e))?;
            context_obj
                .set("messageData", message_data_value)
                .map_err(|e| format!("Failed to set messageData: {}", e))?;

            // Get the handler function
            let global = ctx.globals();
            let handler: rquickjs::Function = global
                .get(handler_name)
                .map_err(|e| format!("Handler function '{}' not found: {}", handler_name, e))?;

            // Call the handler with the context
            let result = handler
                .call::<_, rquickjs::Value>((context_obj,))
                .map_err(|e| format!("Handler execution failed: {}", e))?;

            crate::js_engine::promise_resolve(ctx, result)
        },
        |_ctx, _value| Ok(()),
    )
    .map_err(|e| format!("Context execution failed: {}", e))?;

    Ok(())
}

impl SecureGlobalContext {
    /// Setup JSX factory functions for server-side HTML generation
    fn setup_jsx_functions(&self, ctx: &rquickjs::Ctx<'_>) -> JsResult<()> {
        // Define the h() function and Fragment in JavaScript to properly handle variadic arguments
        // This approach is more compatible with how JSX transpilation works
        ctx.eval::<(), _>(
            r#"
            // Helper to mark HTML as safe (already escaped)
            function SafeHTML(html) {
                this.__html = html;
                this.__safe = true;
            }
            SafeHTML.prototype.toString = function() {
                return this.__html;
            };
            SafeHTML.prototype.valueOf = function() {
                return this.__html;
            };
            // Make it JSON-serializable
            SafeHTML.prototype.toJSON = function() {
                return this.__html;
            };
            
            globalThis.h = function(tag, props, ...children) {
                // Handle function components (React-style components)
                if (typeof tag === 'function') {
                    // Merge children into props if they exist
                    const componentProps = props || {};
                    if (children.length > 0) {
                        componentProps.children = children.length === 1 ? children[0] : children;
                    }
                    // Call the component function and return its result
                    return tag(componentProps);
                }
                
                // Handle HTML elements (string tags)
                // Build attributes string from props
                let attrsStr = '';
                if (props && typeof props === 'object' && !Array.isArray(props)) {
                    for (const key in props) {
                        if (key === 'children') continue;
                        
                        // Basic attribute validation (prevent XSS)
                        if (!/^[a-zA-Z][a-zA-Z0-9\-]*$/.test(key)) continue;
                        
                        // Skip dangerous event handlers
                        if (/^on/i.test(key)) continue;
                        
                        const value = props[key];
                        if (typeof value === 'boolean') {
                            if (value) {
                                attrsStr += ' ' + key;
                            }
                        } else {
                            // HTML escape the attribute value
                            const escaped = String(value)
                                .replace(/&/g, '&amp;')
                                .replace(/"/g, '&quot;')
                                .replace(/'/g, '&#x27;')
                                .replace(/</g, '&lt;')
                                .replace(/>/g, '&gt;');
                            attrsStr += ' ' + key + '="' + escaped + '"';
                        }
                    }
                }
                
                // Process children
                const processChildren = (items) => {
                    return items.map(child => {
                        if (child === null || child === undefined) return '';
                        
                        // Check if it's safe HTML (from another h() call)
                        if (child && typeof child === 'object' && child.__safe) {
                            return child.__html;
                        }
                        
                        // Check if it's already a SafeHTML result (happens with component returns)
                        if (child instanceof SafeHTML) {
                            return child.__html;
                        }
                        
                        if (typeof child === 'string') {
                            // HTML escape text content (this is raw text from JSX)
                            return child
                                .replace(/&/g, '&amp;')
                                .replace(/</g, '&lt;')
                                .replace(/>/g, '&gt;')
                                .replace(/"/g, '&quot;')
                                .replace(/'/g, '&#x27;');
                        }
                        if (Array.isArray(child)) {
                            return processChildren(child);
                        }
                        return String(child);
                    }).join('');
                };
                
                const childrenHtml = processChildren(children);
                
                // Self-closing tags
                const selfClosing = ['area', 'base', 'br', 'col', 'embed', 'hr', 'img', 
                    'input', 'link', 'meta', 'param', 'source', 'track', 'wbr'];
                if (selfClosing.includes(tag)) {
                    return new SafeHTML('<' + tag + attrsStr + '/>');
                }
                
                // Regular tags with children - return as SafeHTML to prevent double-escaping
                return new SafeHTML('<' + tag + attrsStr + '>' + childrenHtml + '</' + tag + '>');
            };
            
            globalThis.Fragment = function(props, ...children) {
                // Fragment just returns children without a wrapper
                const processChildren = (items) => {
                    return items.map(child => {
                        if (child === null || child === undefined) return '';
                        
                        // Check if it's safe HTML
                        if (child && typeof child === 'object' && child.__safe) {
                            return child.__html;
                        }
                        if (child instanceof SafeHTML) {
                            return child.__html;
                        }
                        
                        if (typeof child === 'string') {
                            return child
                                .replace(/&/g, '&amp;')
                                .replace(/</g, '&lt;')
                                .replace(/>/g, '&gt;')
                                .replace(/"/g, '&quot;')
                                .replace(/'/g, '&#x27;');
                        }
                        if (Array.isArray(child)) {
                            return processChildren(child);
                        }
                        return String(child);
                    }).join('');
                };
                return new SafeHTML(processChildren(children));
            };
            "#,
        )?;

        Ok(())
    }
}

#[cfg(test)]
mod metadata_tests {
    use super::extract_route_metadata;
    use rquickjs::{Context, Runtime};

    #[test]
    fn extract_route_metadata_reads_tags_summary_description() {
        let rt = Runtime::new().unwrap();
        let ctx = Context::full(&rt).unwrap();
        ctx.with(|ctx| {
            // Build a metadata object like `{ tags: ["Foo"], summary: "S" }`.
            let obj = rquickjs::Object::new(ctx.clone()).unwrap();
            let arr = rquickjs::Array::new(ctx.clone()).unwrap();
            arr.set(0, "Foo").unwrap();
            arr.set(1, "Bar").unwrap();
            obj.set("tags", arr).unwrap();
            obj.set("summary", "S").unwrap();

            let (tags, summary, description) = extract_route_metadata(Some(&obj));
            assert_eq!(tags, vec!["Foo".to_string(), "Bar".to_string()]);
            assert_eq!(summary, Some("S".to_string()));
            assert_eq!(description, None);
        });
    }

    #[test]
    fn extract_route_metadata_handles_missing_object() {
        let (tags, summary, description) = extract_route_metadata(None);
        assert!(tags.is_empty());
        assert_eq!(summary, None);
        assert_eq!(description, None);
    }
}

#[cfg(test)]
mod api_surface_tests {
    use super::*;
    use rquickjs::{Context, Runtime};

    /// Evaluate `expr` against the globals a handler sees outside the
    /// registration phase — an HTTP handler, a scheduled job, a message
    /// listener or a test all get this surface.
    fn eval_outside_registration_phase(expr: &str) -> String {
        let rt = Runtime::new().expect("runtime");
        let ctx = Context::full(&rt).expect("context");
        ctx.with(|ctx| {
            let config = GlobalSecurityConfig {
                registration_phase: false,
                enable_audit_logging: false,
                dry_run_sink: None,
                console_sink: None,
                log_context: repository::LogContext::default(),
            };
            let context =
                SecureGlobalContext::new_with_config(UserContext::admin("t".into()), config);
            context
                .setup_secure_functions(&ctx, "test://script", None)
                .expect("install globals");
            ctx.eval::<String, _>(expr).expect("eval")
        })
    }

    /// The whole API surface is present regardless of how a script was entered,
    /// so shared helpers never need `typeof x === "undefined"` guards.
    #[test]
    fn every_global_is_installed_outside_the_registration_phase() {
        for global in [
            "routeRegistry",
            "assetStorage",
            "sharedStorage",
            "personalStorage",
            "secretStorage",
            "schedulerService",
            "graphQLRegistry",
            "mcpRegistry",
            "database",
            "console",
            "dispatcher",
            "convert",
            "McpClient",
            "fetch",
        ] {
            assert_eq!(
                eval_outside_registration_phase(&format!("typeof {}", global)),
                if global == "fetch" {
                    "function"
                } else {
                    "object"
                },
                "global `{}` is missing outside the registration phase",
                global
            );
        }
    }

    /// Registration methods stay callable everywhere. They must not throw: a
    /// script's top-level program re-runs on every invocation, so a script that
    /// registers at top level rather than in `init()` would fail on every
    /// request if these raised.
    #[test]
    fn registration_methods_report_instead_of_throwing_or_registering() {
        for (call, subject) in [
            ("routeRegistry.registerRoute('/r', 'h', 'GET')", "/r"),
            ("routeRegistry.registerStreamRoute('/s')", "/s"),
            ("routeRegistry.registerAssetRoute('/a', 'a.txt')", "/a"),
            (
                "graphQLRegistry.registerQuery('q', 'q: String', 'h', 'external')",
                "q",
            ),
            (
                "graphQLRegistry.registerMutation('m', 'm: String', 'h', 'external')",
                "m",
            ),
            (
                "graphQLRegistry.registerSubscription('s', 's: String', 'h', 'external')",
                "s",
            ),
            ("mcpRegistry.registerTool('t', 'd', '{}', 'h')", "t"),
            ("mcpRegistry.registerPrompt('p', 'd', '[]', 'h')", "p"),
            (
                "schedulerService.registerOnce({handler: 'h', runAt: ''})",
                "h",
            ),
            ("schedulerService.registerRecurring({handler: 'h'})", "h"),
            ("dispatcher.registerListener('type', 'h')", "type"),
        ] {
            let result = eval_outside_registration_phase(&format!("String({})", call));
            assert!(
                result.contains("not registered") && result.contains(subject),
                "`{}` should report that it did not register, got: {}",
                call,
                result
            );
        }
    }

    /// Argument validation is context-independent: a malformed call is reported
    /// the same way wherever it is made, rather than being masked by the phase.
    #[test]
    fn argument_validation_runs_before_the_phase_check() {
        assert!(
            eval_outside_registration_phase("dispatcher.registerListener('', 'h')")
                .contains("cannot be empty")
        );
        assert!(
            eval_outside_registration_phase("routeRegistry.registerStreamRoute('no-slash')")
                .contains("must start with"),
        );
    }
}
