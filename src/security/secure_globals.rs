use base64::Engine;
use chrono::Duration as ChronoDuration;
use rquickjs::{Function, Result as JsResult, function::Opt};
use std::collections::HashMap;
use tracing::{debug, info};

/// The JavaScript half of `fetch()`: wraps the Rust call's JSON envelope in a
/// response that can be awaited, read as an object, or parsed as a string.
const FETCH_PRELUDE: &str = include_str!("../../assets/fetch_prelude.js");

/// Gives the host namespaces that answer with a JSON string the same shape a
/// `fetch` response has.
const DATABASE_PRELUDE: &str = include_str!("../../assets/database_prelude.js");

/// The JavaScript half of `console`: joins a variadic call, fills in format
/// specifiers and renders values, so the host binding — which takes one string
/// and throws on anything else — is handed something it accepts.
const CONSOLE_PRELUDE: &str = include_str!("../../assets/console_prelude.js");

/// Builds the Web Storage interface — `length`, `key(i)`, named access, and
/// failures that throw rather than being returned — over the two host stores.
const STORAGE_PRELUDE: &str = include_str!("../../assets/storage_prelude.js");
const TASKS_PRELUDE: &str = include_str!("../../assets/tasks_prelude.js");

/// Builds `sandbox` over the host call that runs a narrowed sub-execution.
const SANDBOX_PRELUDE: &str = include_str!("../../assets/sandbox_prelude.js");
const CRYPTO_PRELUDE: &str = include_str!("../../assets/crypto_prelude.js");
const ENGINE_PRELUDE: &str = include_str!("../../assets/engine_prelude.js");
const MCP_PRELUDE: &str = include_str!("../../assets/mcp_prelude.js");
/// Builds `routeRegistry` over `__hostRouteRegistry`.
const ROUTE_PRELUDE: &str = include_str!("../../assets/route_prelude.js");
/// Builds `files` over `__hostFiles`.
const FILES_PRELUDE: &str = include_str!("../../assets/files_prelude.js");
/// Builds `schedulerService` over `__hostScheduler`.
const SCHEDULER_PRELUDE: &str = include_str!("../../assets/scheduler_prelude.js");
/// Builds `secretStorage` over `__hostSecrets`.
const SECRETS_PRELUDE: &str = include_str!("../../assets/secrets_prelude.js");
/// Builds `mcpRegistry` over `__hostMcpRegistry`.
const MCP_REGISTRY_PRELUDE: &str = include_str!("../../assets/mcp_registry_prelude.js");

/// What `secretStorage`'s mutating methods throw in a delegated execution.
const DELEGATED_SECRET_MANAGEMENT_REFUSAL: &str = "Secrets cannot be changed by background work acting on somebody's behalf. \
     Storing, replacing or deleting a key is something the person does in their own session.";

/// `Headers`, `URLSearchParams`, and the methods `context.request` gains so a
/// body a script receives reads the way a body it fetched does.
const REQUEST_PRELUDE: &str = include_str!("../../assets/request_prelude.js");

use crate::repository;
use crate::scheduler;
use crate::security::{
    Capability, SecurityAuditor, SecurityEventType, SecuritySeverity, UserContext,
};

/// Why `files` refuses to touch a script's entrypoint.
///
/// Merging the root source into the tree made `main.*` reachable by every
/// path that reaches a file, and this one is gated by `WriteAssets` and
/// `DeleteAssets` alone — no ownership check is needed here, because a script
/// only ever reaches its *own* files. That combination would have let a script
/// rewrite or delete its own program while serving a request from anybody
/// holding the editor tier, which is a thing the file API could not do before
/// the merge and a thing nobody asked for it to start doing.
///
/// Refused rather than re-gated on `WriteScripts`, because the engine never
/// offered a script a way to edit its own program and a merge of two storage
/// shapes is not the moment to start. `engine.call("write_file", ...)` is the
/// deliberate way, and it applies the same rules the endpoint does.
const ENTRYPOINT_IS_NOT_A_FILE: &str = "a script's entrypoint is not writable through files. \
     Use engine.call(\"write_file\", { script, path, text }), which applies \
     the checks writing a script's program takes.";

/// A file path a script may write: the same rules the repository applies.
fn validate_file_path(path: &str) -> Result<(), String> {
    if path.is_empty() || path.len() > repository::MAX_ASSET_URI_CHARS {
        return Err(format!(
            "a path is 1-{} characters",
            repository::MAX_ASSET_URI_CHARS
        ));
    }
    if path.contains("..") || path.contains('\\') || path.starts_with('/') {
        return Err(format!(
            "'{}' is not a path inside this script's tree",
            path
        ));
    }
    Ok(())
}

fn millis_since_epoch(time: std::time::SystemTime) -> f64 {
    time.duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as f64
}

/// File a write or a removal through `files`, off the JavaScript thread.
fn audit_file_change(
    auditor: &SecurityAuditor,
    user: &UserContext,
    action: &str,
    severity: SecuritySeverity,
    script_uri: &str,
    path: &str,
) {
    let Ok(rt) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let auditor = auditor.clone();
    let user_id = user.user_id.clone();
    let action = action.to_string();
    let script_uri = script_uri.to_string();
    let path = path.to_string();
    rt.spawn(async move {
        let _ = auditor
            .log_event(
                crate::security::SecurityEvent::new(
                    SecurityEventType::SystemSecurityEvent,
                    severity,
                    user_id,
                )
                .with_resource("asset".to_string())
                .with_action(action)
                .with_detail("uri", &path)
                .with_detail("script_uri", &script_uri),
            )
            .await;
    });
}

/// A `{"error": "..."}` answer, built by the serializer rather than by string
/// formatting.
///
/// The messages that reach here carry quotes and newlines of their own — a
/// Postgres duplicate-key error names the constraint in quotes, and a
/// JavaScript exception arrives with its stack attached. Formatting one
/// straight into a JSON literal produced a string `JSON.parse` rejects, so
/// `.json()` threw on exactly the errors a script most needs to read, and the
/// only way to see one was to treat the answer as a string.
fn error_answer(message: impl std::fmt::Display) -> String {
    serde_json::json!({ "error": message.to_string() }).to_string()
}

/// Read an optional argument a script may pass as `null` to skip it.
///
/// `Opt<T>` treats a *missing* argument as absent but refuses a literal
/// `null`, and the documented calling convention uses `null` to skip a
/// positional one — `query(table, null, 100, null, "asc")`. Skipping an
/// argument that way raised a type error naming a conversion the script never
/// asked for, so the only way to reach a later argument was to pass a value
/// for every earlier one.
fn optional_arg<'js, T: rquickjs::FromJs<'js>>(
    value: Opt<rquickjs::Value<'js>>,
    name: &str,
) -> Result<Option<T>, String> {
    let Some(value) = value.0 else {
        return Ok(None);
    };
    if value.is_null() || value.is_undefined() {
        return Ok(None);
    }
    // Taken from the value rather than passed in: a separately supplied
    // context is a second lifetime, and the two are invariant.
    let ctx = value.ctx().clone();
    T::from_js(&ctx, value)
        .map(Some)
        .map_err(|e| format!("{} is not valid: {}", name, e))
}

/// Read an argument a script may pass either as JSON text or as the value that
/// text describes.
///
/// The host bindings behind `sendStreamMessage` and
/// `sendStreamMessageFiltered` took a `String`, while the type declarations
/// typed the same argument `any` and every example passed an object — so the
/// documented call raised `TypeError: Error converting from js 'object' into
/// type 'string'` out of the binding, QuickJS having no coercion to offer it.
/// Serializing here is what the declarations already promise ("will be JSON
/// serialized"), and a string is passed through untouched rather than being
/// wrapped in quotes, because every script written against the binding as it
/// was sends `JSON.stringify(...)` already.
fn json_arg<'js>(value: rquickjs::Value<'js>, name: &str) -> JsResult<String> {
    if let Some(string) = value.as_string() {
        return string.to_string();
    }
    if value.is_undefined() || value.is_null() {
        return Ok(String::new());
    }
    let ctx = value.ctx().clone();
    match ctx.json_stringify(value) {
        Ok(Some(string)) => string.to_string(),
        // `JSON.stringify` answers `undefined` for a function, a symbol, and
        // for `undefined` itself. Naming the argument is the whole of the
        // diagnosis, so the error says which one could not be serialized.
        Ok(None) => Err(rquickjs::Error::new_from_js_message(
            "value",
            "json",
            &format!("{} cannot be serialized as JSON", name),
        )),
        Err(e) => Err(e),
    }
}

/// Turn the arguments a script passed to `database.query` into the options the
/// repository runs it under.
///
/// Every one of them is validated here rather than nearer the statement, so a
/// query that cannot mean what it says is refused before anything runs. Two of
/// those refusals are new: a sort direction that is neither `asc` nor `desc`
/// used to sort ascending, and an unrecognised option key used to be dropped.
/// Both handed back a query that read like the one the script asked for and
/// was not.
fn build_query_options(
    limit: Option<i32>,
    order_by: Option<String>,
    order_dir: Option<String>,
    options_json: Option<&str>,
) -> Result<crate::repository::QueryOptions, String> {
    let mut options = crate::repository::QueryOptions {
        limit: limit.map(i64::from),
        order_by,
        ..Default::default()
    };

    if let Some(raw) = order_dir {
        options.order_dir = crate::repository::OrderDirection::parse(&raw)
            .ok_or_else(|| format!("orderDir must be \"asc\" or \"desc\", got \"{}\"", raw))?;
    }

    let Some(raw) = options_json else {
        return Ok(options);
    };
    if raw.trim().is_empty() {
        return Ok(options);
    }

    let parsed: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("Invalid options JSON: {}", e))?;
    let object = parsed.as_object().ok_or_else(|| {
        "options must be a JSON object, for example {\"forUpdate\": true}".to_string()
    })?;

    for (key, value) in object {
        match key.as_str() {
            "forUpdate" => {
                options.for_update = value.as_bool().ok_or_else(|| {
                    format!("options.forUpdate must be true or false, got {}", value)
                })?;
            }
            other => {
                return Err(format!(
                    "Unknown query option '{}'; the supported options are: forUpdate",
                    other
                ));
            }
        }
    }

    Ok(options)
}

/// Reads the schema a script wants a table to have.
///
/// `{ "columns": [{ "name", "type", "nullable"?, "default"?, "references"? }],
/// "uniqueIndexes"?: [["col"]] }`. A `"reference"` column names the table it
/// points at in `references`. `nullable` defaults to true,
/// because a column added to a table that already has rows cannot be `NOT NULL`
/// without a default, and the whole point of this call is that it is safe to
/// make against a table that is already in use.
fn parse_table_spec(schema_json: &str) -> Result<crate::repository::TableSpec, String> {
    use std::str::FromStr;

    let parsed: serde_json::Value =
        serde_json::from_str(schema_json).map_err(|e| format!("Invalid schema JSON: {}", e))?;

    let columns = parsed
        .get("columns")
        .and_then(|columns| columns.as_array())
        .ok_or("schema must carry a \"columns\" array")?;

    let mut spec = crate::repository::TableSpec::default();

    for column in columns {
        let name = column
            .get("name")
            .and_then(|name| name.as_str())
            .ok_or("every column needs a \"name\"")?;
        let type_name = column
            .get("type")
            .and_then(|kind| kind.as_str())
            .ok_or_else(|| format!("column \"{}\" needs a \"type\"", name))?;
        let references = column
            .get("references")
            .and_then(|table| table.as_str())
            .map(str::to_string);
        let column_type = if type_name == "reference" {
            if references.is_none() {
                return Err(format!(
                    "column \"{}\" is a reference and needs \"references\": the table it points at",
                    name
                ));
            }
            crate::db_schema_utils::ColumnType::Integer
        } else {
            if references.is_some() {
                return Err(format!(
                    "column \"{}\" has \"references\" but is not of type \"reference\"",
                    name
                ));
            }
            crate::db_schema_utils::ColumnType::from_str(type_name)
                .map_err(|e| format!("column \"{}\": {}", name, e))?
        };

        spec.columns.push(crate::repository::EnsuredColumn {
            name: name.to_string(),
            column_type,
            nullable: column
                .get("nullable")
                .and_then(|nullable| nullable.as_bool())
                .unwrap_or(true),
            default_value: column
                .get("default")
                .and_then(|default| default.as_str())
                .map(str::to_string),
            references,
        });
    }

    if let Some(indexes) = parsed.get("uniqueIndexes").and_then(|i| i.as_array()) {
        for index in indexes {
            let columns = index
                .as_array()
                .ok_or("every entry in \"uniqueIndexes\" must be an array of column names")?
                .iter()
                .map(|column| {
                    column
                        .as_str()
                        .map(str::to_string)
                        .ok_or("index columns must be strings")
                })
                .collect::<Result<Vec<_>, _>>()?;
            spec.unique_indexes.push(columns);
        }
    }

    Ok(spec)
}

/// A `{"success": true, ...}` answer carrying `fields`.
///
/// Serialised for the same reason [`error_answer`] is: the table and column
/// names echoed back come from the script, and a name the engine would reject
/// still has to produce a readable answer saying so.
///
/// `fields` is expected to be an object; anything else contributes nothing, so
/// every call site passes a `json!({ ... })` literal.
fn success_answer(fields: serde_json::Value) -> String {
    let mut answer = serde_json::Map::new();
    answer.insert("success".to_string(), serde_json::Value::Bool(true));
    if let serde_json::Value::Object(fields) = fields {
        answer.extend(fields);
    }
    serde_json::Value::Object(answer).to_string()
}

/// Where a registration lands.
///
/// `Rc` rather than `Box` because more than one registry function records
/// through it: a route and a file route are one kind of registration
/// differing only in what the engine does once the path matches, so they
/// share one sink rather than each owning a registry.
type RouteRegisterFn = std::rc::Rc<
    dyn Fn(&str, &repository::RouteMetadata, Option<&str>) -> Result<(), rquickjs::Error>,
>;

/// Which registry a [`CollectedRegistration`] would have been written to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RegistrationKind {
    Route,
    Stream,
    AssetRoute,
    McpTool,
    McpPrompt,
    McpResource,
    ScheduledJob,
}

impl RegistrationKind {
    /// The registering API's name, for messages that have to say what was
    /// skipped.
    pub fn api(self) -> &'static str {
        match self {
            RegistrationKind::Route | RegistrationKind::Stream | RegistrationKind::AssetRoute => {
                "routeRegistry.registerRoute"
            }
            RegistrationKind::McpTool => "mcpRegistry.registerTool",
            RegistrationKind::McpPrompt => "mcpRegistry.registerPrompt",
            RegistrationKind::McpResource => "mcpRegistry.registerResource",
            RegistrationKind::ScheduledJob => "schedulerService",
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

/// `{ name, description, mimeType }` off an optional metadata object, for
/// `mcpRegistry.registerResource`.
///
/// Every field is optional and a missing one comes back as `None` rather than
/// as a default, because the caller decides what to fall back to: the name
/// falls back to the asset's, and the MIME type falls back to the asset's own
/// at read time rather than at registration, so an asset re-uploaded as
/// something else is not described by a type recorded at `init()`.
fn extract_resource_metadata(
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

/// Secure wrapper for JavaScript global functions that enforces Rust-level validation
pub struct SecureGlobalContext {
    user_context: UserContext,
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
    /// (streams, asset routes, MCP, scheduler) is a process-wide
    /// singleton written to directly, so a candidate's `init()` would
    /// otherwise replace the deployed script's resolvers and jobs with its
    /// own, and a broken candidate would take the live script
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
    /// When this execution is acting on somebody's behalf, what they
    /// authorised ([`crate::delegation::Scope`]).
    ///
    /// `None` means this is not a delegated execution, and nothing is
    /// narrowed. That is every path but a delegated task: an ordinary request
    /// *is* the person, so there is no grant to hold it to and no question of
    /// exceeding one. Making `None` the permissive case is what keeps this
    /// change invisible to everything that existed before delegation did.
    ///
    /// `Some` is the narrow case, and it is narrow by omission: a scope not
    /// listed is not granted. So a grant covering only `personal_storage`
    /// reaches this person's storage and *not* their secrets, which is what
    /// the consent page said and what was previously only decoration.
    pub delegated_scopes: Option<Vec<crate::delegation::Scope>>,
    /// Whether this execution may reach the engine's own management tools
    /// through the `engine` global.
    ///
    /// **False by default, and every construction site states its answer**,
    /// because the capability check underneath is not enough on its own. Two
    /// kinds of execution hold capabilities that were never a person's:
    ///
    /// - The engine's own actors. A scheduled job runs as
    ///   `UserContext::admin("scheduler")` and `init()` as
    ///   `admin("script-init")` — synthetic administrators with nobody behind
    ///   them. `engine.call` there would mean any script in the engine
    ///   administers it from a cron line, which is not a capability anybody
    ///   granted.
    ///
    /// - A narrowed sub-execution. `sandbox.run` hands model-authored code a
    ///   chosen subset, and the subset cannot express this: the agent grants
    ///   `view_logs` so that `console` works, and `read_logs` is gated on
    ///   exactly that capability while taking *any* script's URI as an
    ///   argument. Granting one would hand over the other.
    ///
    /// So it is on for the two executions whose authority came from a
    /// credential — a script serving a request, and a delegated task, where
    /// the person consented on a page that named what they were consenting to
    /// — and off everywhere else.
    pub engine_api: bool,
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
            // Not acting for anybody, so nothing to narrow.
            delegated_scopes: None,
            // Fail closed, like `registration_phase` above: an execution that
            // did not ask for the management surface does not get it.
            engine_api: false,
        }
    }
}

impl GlobalSecurityConfig {
    /// Whether this execution is acting on somebody's behalf.
    ///
    /// The question is not "is there a user" — an ordinary request has one too.
    /// It is whether the user is *absent*, which is what makes a grant the
    /// only authority for touching anything of theirs.
    pub fn is_delegated(&self) -> bool {
        self.delegated_scopes.is_some()
    }

    /// Whether `scope` may be exercised here.
    ///
    /// True for every execution that is not delegated, because the person is
    /// present and acting for themselves. For a delegated one it is exactly
    /// what they ticked.
    pub fn allows_delegated(&self, scope: crate::delegation::Scope) -> bool {
        match &self.delegated_scopes {
            None => true,
            Some(scopes) => scopes.contains(&scope),
        }
    }

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

/// One entry of a `fetchAll` list, as JavaScript writes it.
///
/// `{ url, options }` rather than the positional pair `fetch` takes, because
/// a list of two-element arrays is the shape nobody reads back correctly six
/// months later.
#[derive(serde::Deserialize)]
struct FetchRequestSpec {
    url: String,
    #[serde(default)]
    options: crate::http_client::FetchOptions,
}

/// Why an API refused, when what it refused on was a capability.
///
/// Names the capability rather than saying "insufficient permissions",
/// because the caller of a narrowed context is usually the script itself and
/// the missing name is the whole of what it needs to know. And it says
/// *narrowed* when the context was attenuated: an administrator's script
/// being told it may not write a row is otherwise the most confusing message
/// the engine produces, and the reason is that it asked for this.
fn capability_refusal(api: &str, capability: &Capability, user: &UserContext) -> String {
    if user.attenuated {
        format!(
            "{}: refused - this execution was narrowed and does not hold '{}'",
            api,
            capability.as_str()
        )
    } else {
        format!(
            "{}: refused - this caller does not hold '{}'",
            api,
            capability.as_str()
        )
    }
}

/// What `crypto.hmacVerify` is handed, as JavaScript writes it.
///
/// `secretName` and not `secret`. The value never enters the runtime — that is
/// the whole of what this API is for — and a field called `secret` invites
/// somebody to pass one, which would work, and would silently give up the
/// property they came here for.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct HmacVerifyOptions {
    secret_name: String,
    message: String,
    signature: String,
    #[serde(default)]
    algorithm: Option<String>,
    #[serde(default)]
    encoding: Option<String>,
}

/// Refusal for a name that is not one of a fixed set.
///
/// Names what was asked for *and* what there is, because the caller who typed
/// `sha-256` cannot otherwise tell whether the problem is the hyphen or the
/// algorithm.
fn unknown_name_error(api: &str, what: &str, given: &str, known: &[&str]) -> rquickjs::Error {
    rquickjs::Error::new_from_js_message(
        "crypto",
        "type_error",
        &format!(
            "{}: '{}' is not a known {} — one of {}",
            api,
            given,
            what,
            known.join(", ")
        ),
    )
}

/// The value behind a secret's name, or a refusal naming the secret.
///
/// Missing throws rather than answering `false`. A verification that quietly
/// fails because nobody stored the key looks exactly like a verification that
/// failed because the request was forged — so a deployment that was never
/// finished would present as an endpoint under permanent attack, and the log
/// would agree. The `{{secret:...}}` rule, in the place it matters most: an
/// unresolvable secret is an error, because behaving as though it resolved is
/// worse than stopping.
fn resolve_named_secret(
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

/// [`capability_refusal`] as a thrown JavaScript error, for the APIs that
/// throw rather than returning an envelope.
fn capability_error(
    api: &'static str,
    capability: &Capability,
    user: &UserContext,
) -> rquickjs::Error {
    rquickjs::Error::new_from_js_message(
        api,
        "capability",
        &capability_refusal(api, capability, user),
    )
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
fn mcp_client_capabilities(api: &'static str, user: &UserContext) -> JsResult<()> {
    if !user.has_capability(&Capability::UseNetwork) {
        return Err(capability_error(api, &Capability::UseNetwork, user));
    }
    if !user.has_capability(&Capability::ReadSecrets) {
        return Err(capability_error(api, &Capability::ReadSecrets, user));
    }
    Ok(())
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

/// `{ok: value}` — the envelope every host-object prelude unwraps into a
/// return value.
fn host_ok(value: serde_json::Value) -> String {
    serde_json::json!({ "ok": value }).to_string()
}

/// `{error: {name, message}}` — the envelope a prelude turns into a thrown
/// error of that name. A host binding cannot throw a JavaScript exception of
/// the right type, which is why the throw happens on the JavaScript side.
fn host_failure(name: &str, message: &str) -> String {
    serde_json::json!({ "error": { "name": name, "message": message } }).to_string()
}

/// What a registered path leads to.
#[derive(Debug, Clone, PartialEq)]
enum RouteTarget {
    /// A script function, called for one HTTP method.
    Handler { name: String, method: String },
    /// A server-sent event stream the engine holds open. `authorize` names
    /// the function that decides who may connect.
    Stream { authorize: Option<String> },
    /// A file of the script's tree, served by the engine. `authorize` names
    /// the function that decides who may read it.
    File {
        path: String,
        authorize: Option<String>,
    },
}

/// One `registerRoute` spec, once read.
#[derive(Debug, Clone, PartialEq)]
struct RouteSpec {
    target: RouteTarget,
    summary: Option<String>,
    description: Option<String>,
    tags: Vec<String>,
    parameters: Option<serde_json::Value>,
    request_body: Option<serde_json::Value>,
}

/// The keys a spec may carry. Anything else is refused rather than ignored,
/// since a misspelt `authorize` that was silently dropped would publish the
/// file it was meant to guard.
const ROUTE_SPEC_KEYS: [&str; 10] = [
    "handler",
    "stream",
    "file",
    "method",
    "authorize",
    "summary",
    "description",
    "tags",
    "parameters",
    "requestBody",
];

/// Read a `registerRoute` spec. The error is the message a `TypeError`
/// carries: every way a spec can be wrong is a mistake in the call.
fn parse_route_spec(value: &serde_json::Value) -> Result<RouteSpec, String> {
    let object = value
        .as_object()
        .ok_or("the spec is an object: { handler }, { stream: true } or { file }")?;
    if let Some(unknown) = object
        .keys()
        .find(|key| !ROUTE_SPEC_KEYS.contains(&key.as_str()))
    {
        return Err(format!(
            "unknown spec key '{}' (expected one of {})",
            unknown,
            ROUTE_SPEC_KEYS.join(", ")
        ));
    }

    let text = |key: &str| -> Result<Option<String>, String> {
        match object.get(key) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(serde_json::Value::String(s)) if !s.trim().is_empty() => Ok(Some(s.clone())),
            Some(_) => Err(format!("'{}' must be a non-empty string", key)),
        }
    };

    let handler = text("handler")?;
    let file = text("file")?;
    let stream = match object.get("stream") {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::Bool(true)) => true,
        Some(_) => return Err("'stream' is either true or absent".to_string()),
    };
    let targets = [handler.is_some(), stream, file.is_some()]
        .into_iter()
        .filter(|present| *present)
        .count();
    if targets != 1 {
        return Err(
            "a spec names exactly one target: { handler }, { stream: true } or { file }"
                .to_string(),
        );
    }

    let authorize = text("authorize")?;
    if let Some(name) = authorize.as_deref() {
        validate_function_name("authorize", name)?;
    }
    let parameters = object.get("parameters").filter(|v| !v.is_null()).cloned();
    let request_body = object.get("requestBody").filter(|v| !v.is_null()).cloned();
    let method = text("method")?;

    let target = if let Some(name) = handler {
        // The handler *is* a handler route's authorization: it runs as the
        // requesting user and answers whatever it decides to. A second
        // function in front of it would be two places deciding one thing.
        if authorize.is_some() {
            return Err(
                "'authorize' is for streams and files; a handler route's handler is \
                 where it decides who may call it"
                    .to_string(),
            );
        }
        validate_function_name("handler", &name)?;
        RouteTarget::Handler {
            name,
            method: method.unwrap_or_else(|| "GET".to_string()),
        }
    } else {
        if method.is_some() {
            return Err("'method' is for handler routes; streams and files answer GET".to_string());
        }
        if parameters.is_some() || request_body.is_some() {
            return Err("'parameters' and 'requestBody' describe a handler route".to_string());
        }
        match file {
            Some(path) => RouteTarget::File { path, authorize },
            None => RouteTarget::Stream { authorize },
        }
    };

    let tags = match object.get("tags") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| "'tags' is an array of strings".to_string())
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("'tags' is an array of strings".to_string()),
    };

    Ok(RouteSpec {
        target,
        summary: text("summary")?,
        description: text("description")?,
        tags,
        parameters,
        request_body,
    })
}

/// A function a registration names must be one a script can define.
fn validate_function_name(key: &str, name: &str) -> Result<(), String> {
    if name.len() > 100 {
        return Err(format!("'{}' is too long (max 100 characters)", key));
    }
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
    {
        return Err(format!(
            "'{}' must name a function: letters, digits, '_' and '$' only",
            key
        ));
    }
    Ok(())
}

/// `{ ok: false, reason }` in the host envelope: a registration refused for
/// a reason the script is told about rather than thrown at.
fn refusal_answer(reason: String) -> String {
    host_ok(serde_json::json!({ "ok": false, "reason": reason }))
}

/// What a well-formed registration came to.
#[derive(Debug, PartialEq)]
enum Registered {
    Done,
    /// Not registered, for a reason the script is told about rather than
    /// thrown at, so its other registrations survive.
    Refused(String),
}

/// A registration that could not be attempted: the name is the error type
/// the prelude throws.
struct RegistrationFailure {
    name: &'static str,
    message: String,
}

impl RegistrationFailure {
    fn type_error(message: impl Into<String>) -> Self {
        Self {
            name: "TypeError",
            message: message.into(),
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Self {
            name: "Error",
            message: message.into(),
        }
    }
}

/// Everything `registerRoute` needs, captured once per execution.
struct RouteRegistrar {
    user_context: UserContext,
    auditor: SecurityAuditor,
    script_uri: String,
    config: GlobalSecurityConfig,
    record: Option<RouteRegisterFn>,
}

const REGISTER_ROUTE: &str = "routeRegistry.registerRoute";

impl RouteRegistrar {
    fn register(&self, path: &str, spec: RouteSpec) -> Result<Registered, RegistrationFailure> {
        // Path checks run in every context, so a bad path is reported the same
        // way wherever the call is made.
        if let Some(prefix) = crate::engine_api::reserved_route_prefix(path) {
            return Err(RegistrationFailure::error(format!(
                "path '{}' is reserved for the engine (prefix '{}')",
                path, prefix
            )));
        }
        if !path.starts_with('/') {
            return Err(RegistrationFailure::type_error(format!(
                "path '{}' must start with '/'",
                path
            )));
        }
        if path.len() > 500 {
            return Err(RegistrationFailure::type_error(
                "path too long (max 500 characters)",
            ));
        }
        if path.contains("..") || path.contains('\\') {
            return Err(RegistrationFailure::type_error(format!(
                "path '{}' may not contain '..' or '\\'",
                path
            )));
        }

        let RouteSpec {
            target,
            summary,
            description,
            tags,
            parameters,
            request_body,
        } = spec;

        let (mut meta, method) = match target {
            RouteTarget::Handler { name, method } => {
                let Some(record) = self.record.as_ref() else {
                    return Ok(Registered::Refused(registration_inactive(
                        REGISTER_ROUTE,
                        path,
                    )));
                };
                let mut meta = repository::RouteMetadata::simple(name);
                meta.summary = summary;
                meta.description = description;
                meta.tags = tags;
                meta.parameters = parameters;
                meta.request_body = request_body;
                record(path, &meta, Some(&method))
                    .map_err(|e| RegistrationFailure::error(e.to_string()))?;
                return Ok(Registered::Done);
            }
            RouteTarget::Stream { authorize } => {
                self.require(&crate::security::Capability::ManageStreams, "stream", path)?;
                if !self.config.registration_phase {
                    return Ok(Registered::Refused(registration_inactive(
                        REGISTER_ROUTE,
                        path,
                    )));
                }
                let registration = CollectedRegistration::new(RegistrationKind::Stream, path);
                let registration = match authorize.as_ref() {
                    Some(function) => registration.with_handler(function.clone()),
                    None => registration,
                };
                if self.config.collect(registration).is_some() {
                    return Ok(Registered::Done);
                }
                (
                    repository::RouteMetadata::stream(authorize),
                    repository::STREAM_METHOD,
                )
            }
            RouteTarget::File {
                path: file,
                authorize,
            } => {
                self.require(&crate::security::Capability::WriteAssets, "file", path)?;
                if file.len() > 255 || file.contains("..") || file.contains('\\') {
                    return Err(RegistrationFailure::type_error(format!(
                        "'{}' is not a file path of this script's tree",
                        file
                    )));
                }
                if !self.config.registration_phase {
                    return Ok(Registered::Refused(registration_inactive(
                        REGISTER_ROUTE,
                        path,
                    )));
                }
                if repository::fetch_asset(&self.script_uri, &file).is_none() {
                    return Ok(Registered::Refused(format!(
                        "'{}' is not a file of this script",
                        file
                    )));
                }
                // Exposure belongs to the tree, not to this call. A file
                // outside `public/` is one the directory says is private, and
                // publishing it was the mistake nothing else in the engine
                // could see — not the write path, not the revision manifest,
                // not a git diff. Refused rather than warned about, which is
                // what makes the directory the answer rather than a
                // suggestion; publishing a file is now moving it, which is a
                // reviewable act.
                if !crate::exposure::is_publishable(&file) {
                    crate::exposure::note_refusal(&self.script_uri, false, path, &file);
                    tracing::warn!(
                        script = %self.script_uri,
                        path = %path,
                        file = %file,
                        "Refused to publish a file from outside '{}'",
                        crate::exposure::PUBLIC_DIR,
                    );
                    return Ok(Registered::Refused(format!(
                        "'{}' is not under '{}', so it is not a file the world may read. \
                         Move it to '{}{}' and register that. A file's directory is what \
                         says whether it is public; see GET /engine/exposure.",
                        file,
                        crate::exposure::PUBLIC_DIR,
                        crate::exposure::PUBLIC_DIR,
                        file,
                    )));
                }
                if self
                    .config
                    .collect(CollectedRegistration::new(
                        RegistrationKind::AssetRoute,
                        path,
                    ))
                    .is_some()
                {
                    return Ok(Registered::Done);
                }
                (
                    repository::RouteMetadata::file(file, authorize),
                    repository::ASSET_METHOD,
                )
            }
        };

        // A stream and a file route record into the sink a handler route
        // does: one kind of registration, differing only in what the engine
        // does once the path matches. Being in the script's registrations is
        // what gives them `:param` and `/*`, the host filter, and an
        // unregistration when the script stops making the call.
        let Some(record) = self.record.as_ref() else {
            return Ok(Registered::Refused(registration_inactive(
                REGISTER_ROUTE,
                path,
            )));
        };
        meta.summary = summary;
        meta.description = description;
        meta.tags = tags;
        record(path, &meta, Some(method)).map_err(|e| RegistrationFailure::error(e.to_string()))?;
        Ok(Registered::Done)
    }

    /// The capability a stream or a file route takes, audited when missing.
    fn require(
        &self,
        capability: &crate::security::Capability,
        resource: &str,
        path: &str,
    ) -> Result<(), RegistrationFailure> {
        let Err(e) = self.user_context.require_capability(capability) else {
            return Ok(());
        };
        if self.config.enable_audit_logging
            && let Ok(rt) = tokio::runtime::Handle::try_current()
        {
            let auditor = self.auditor.clone();
            let user_id = self.user_context.user_id.clone();
            let resource = resource.to_string();
            let path = path.to_string();
            rt.spawn(async move {
                let _ = auditor
                    .log_event(
                        crate::security::SecurityEvent::new(
                            crate::security::SecurityEventType::AuthorizationFailure,
                            crate::security::SecuritySeverity::Medium,
                            user_id,
                        )
                        .with_resource(resource)
                        .with_action("register".to_string())
                        .with_detail("path", &path),
                    )
                    .await;
            });
        }
        Err(RegistrationFailure::error(e.to_string()))
    }
}

/// Send to a stream's connections, answering in the host envelope.
///
/// The shared `/system/` namespace is open to every script. The engine's own
/// script-update stream is deliberately not: it is broadcast to from Rust
/// (`engine_api::broadcast_script_update`), which never passes through here,
/// so exempting it would only let a script forge engine notifications to
/// every subscriber.
fn send_stream_message(
    user_context: &UserContext,
    auditor: &SecurityAuditor,
    api: &str,
    path: &str,
    message: &str,
    filter: Option<(
        &HashMap<String, String>,
        crate::stream_registry::FilterMatchMode,
    )>,
) -> String {
    let audit = |event_type, severity, action: &str| {
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            let auditor = auditor.clone();
            let user_id = user_context.user_id.clone();
            let action = action.to_string();
            let path = path.to_string();
            let length = message.len().to_string();
            rt.spawn(async move {
                let _ = auditor
                    .log_event(
                        crate::security::SecurityEvent::new(event_type, severity, user_id)
                            .with_resource("stream".to_string())
                            .with_action(action)
                            .with_detail("path", &path)
                            .with_detail("message_length", length),
                    )
                    .await;
            });
        }
    };

    if !path.starts_with("/system/")
        && let Err(e) = user_context.require_capability(&crate::security::Capability::ManageStreams)
    {
        audit(
            crate::security::SecurityEventType::AuthorizationFailure,
            crate::security::SecuritySeverity::Medium,
            api,
        );
        return host_failure("Error", &format!("routeRegistry.{}: {}", api, e));
    }
    audit(
        crate::security::SecurityEventType::SystemSecurityEvent,
        crate::security::SecuritySeverity::Low,
        api,
    );

    let registry = &crate::stream_registry::GLOBAL_STREAM_REGISTRY;
    let result = match filter {
        Some((filter, mode)) => {
            registry.broadcast_to_stream_with_filter_mode(path, message, filter, mode)
        }
        None => registry.broadcast_to_stream(path, message),
    };
    match result {
        Ok(sent) => host_ok(serde_json::json!({
            "delivered": sent.successful_sends,
            "connections": sent.total_connections,
            "failed": sent.failed_connections.len(),
        })),
        Err(e) => host_failure(
            "Error",
            &format!("routeRegistry.{}: '{}': {}", api, path, e),
        ),
    }
}

impl SecureGlobalContext {
    pub fn new(user_context: UserContext) -> Self {
        let pool = crate::database::get_global_database().map(|db| db.pool().clone());

        Self {
            user_context,
            auditor: SecurityAuditor::new(pool),
            config: GlobalSecurityConfig::default(),
        }
    }

    pub fn new_with_config(user_context: UserContext, config: GlobalSecurityConfig) -> Self {
        let pool = crate::database::get_global_database().map(|db| db.pool().clone());

        Self {
            user_context,
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
        self.setup_mcp_functions(ctx, script_uri)?;
        self.setup_scheduler_functions(ctx, script_uri)?;
        self.setup_task_functions(ctx, script_uri)?;
        self.setup_sandbox_functions(ctx, script_uri)?;
        self.setup_crypto_object(ctx, script_uri)?;
        self.setup_engine_object(ctx, script_uri)?;

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
                // Capture before the capability check, not after it. The two
                // are different channels: `ViewLogs` gates writing to the
                // script's stored log, while capture hands the output back to
                // whoever asked for this run — an `/engine/eval` caller, or a
                // script that started a narrowed sub-execution. A planning
                // turn holding no `view_logs` should still be able to show its
                // caller what it printed; losing the output as well as the log
                // line would make a narrowed turn silent for no reason anybody
                // chose.
                config_write.capture_console(&level, &message);

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
    /// Install `__hostFiles` and the prelude that builds `files` over it.
    ///
    /// A script's own tree, read and written by path. This was `assetStorage`
    /// — `listAssets`, `fetchAsset`, `upsertAsset`, `deleteAsset` — which
    /// handed back base64 for every read, a sentence for a missing file that
    /// was not base64, and `"Error: ..."` as a value, so each caller decoded,
    /// then guessed which of the three it had. Every caller in practice wanted
    /// text. A read now answers text, or `null` when there is no such file;
    /// binary is asked for by name; failures throw.
    ///
    /// The entrypoint stays out of reach, for the reason
    /// [`ENTRYPOINT_IS_NOT_A_FILE`] gives.
    fn setup_asset_management_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
    ) -> JsResult<()> {
        let host = rquickjs::Object::new(ctx.clone())?;

        let user_list = self.user_context.clone();
        let script_list = script_uri.to_string();
        let list = Function::new(ctx.clone(), move || -> String {
            if let Err(e) = user_list.require_capability(&Capability::ReadAssets) {
                return host_failure("Error", &format!("files.list: {}", e));
            }
            let mut entries: Vec<serde_json::Value> = repository::fetch_assets(&script_list)
                .values()
                .map(|asset| {
                    serde_json::json!({
                        "path": asset.uri,
                        "size": asset.content.len(),
                        "mimetype": asset.mimetype,
                        "createdAt": millis_since_epoch(asset.created_at),
                        "updatedAt": millis_since_epoch(asset.updated_at),
                    })
                })
                .collect();
            // Sorted, so a listing is the same bytes every time it is the
            // same tree — which matters to a script that puts one into a
            // prompt it wants cached.
            entries.sort_by(|a, b| a["path"].as_str().cmp(&b["path"].as_str()));
            host_ok(serde_json::Value::Array(entries))
        })?;
        host.set("list", list)?;

        let user_read = self.user_context.clone();
        let script_read = script_uri.to_string();
        let read = Function::new(
            ctx.clone(),
            move |path: String, base64: Option<bool>| -> String {
                if let Err(e) = user_read.require_capability(&Capability::ReadAssets) {
                    return host_failure("Error", &format!("files.read: {}", e));
                }
                let Some(asset) = repository::fetch_asset(&script_read, &path) else {
                    return host_ok(serde_json::Value::Null);
                };
                if base64.unwrap_or(false) {
                    return host_ok(serde_json::Value::String(
                        base64::engine::general_purpose::STANDARD.encode(asset.content),
                    ));
                }
                match String::from_utf8(asset.content) {
                    Ok(text) => host_ok(serde_json::Value::String(text)),
                    Err(_) => host_failure(
                        "TypeError",
                        &format!(
                            "files.read: '{}' is not text; read it with {{ encoding: \"base64\" }}",
                            path
                        ),
                    ),
                }
            },
        )?;
        host.set("read", read)?;

        let user_write = self.user_context.clone();
        let auditor_write = self.auditor.clone();
        let script_write = script_uri.to_string();
        let write = Function::new(
            ctx.clone(),
            move |path: String,
                  content: String,
                  base64: Option<bool>,
                  mimetype: Option<String>|
                  -> String {
                if crate::module_loader::is_root_module_name(&path) {
                    return host_failure("Error", ENTRYPOINT_IS_NOT_A_FILE);
                }
                if let Err(e) = user_write.require_capability(&Capability::WriteAssets) {
                    return host_failure("Error", &format!("files.write: {}", e));
                }
                if let Err(message) = validate_file_path(&path) {
                    return host_failure("TypeError", &format!("files.write: {}", message));
                }
                let bytes = if base64.unwrap_or(false) {
                    match base64::engine::general_purpose::STANDARD.decode(&content) {
                        Ok(bytes) => bytes,
                        Err(e) => {
                            return host_failure(
                                "TypeError",
                                &format!("files.write: the content is not base64: {}", e),
                            );
                        }
                    }
                } else {
                    content.into_bytes()
                };
                // The storage-side limit, so this refuses exactly what the
                // repository would refuse rather than a little more.
                if bytes.len() > repository::MAX_ASSET_CONTENT_BYTES {
                    return host_failure(
                        "RangeError",
                        &format!(
                            "files.write: '{}' is {} bytes (max {})",
                            path,
                            bytes.len(),
                            repository::MAX_ASSET_CONTENT_BYTES
                        ),
                    );
                }
                let mimetype = mimetype
                    .filter(|m| !m.trim().is_empty())
                    .unwrap_or_else(|| crate::engine_api::mimetype_for(&path).to_string());

                audit_file_change(
                    &auditor_write,
                    &user_write,
                    "upsert",
                    SecuritySeverity::Medium,
                    &script_write,
                    &path,
                );

                let now = std::time::SystemTime::now();
                let asset = repository::Asset {
                    uri: path.clone(),
                    name: Some(path.clone()),
                    mimetype,
                    content: bytes,
                    created_at: now,
                    updated_at: now,
                    script_uri: script_write.clone(),
                };
                match repository::upsert_asset(asset) {
                    Ok(_) => {
                        // A write here is a write to the script, and every
                        // other path that changes a script's files records
                        // what it consisted of afterwards — so a file a script
                        // writes (an agent's skill, say: model-authored content
                        // somebody may want to read back or undo) has history,
                        // can be reverted, and moves a git binding off
                        // `in_sync`. Recorded after the write and not instead
                        // of it: a history that cannot be written is worth
                        // less than the content, so `record_blocking` reports
                        // failure rather than propagating it.
                        crate::revisions::record_blocking(
                            &script_write,
                            crate::revisions::Origin::Sandbox,
                            user_write.user_id.as_deref(),
                        );
                        host_ok(serde_json::Value::Null)
                    }
                    Err(e) => host_failure("Error", &format!("files.write: {}", e)),
                }
            },
        )?;
        host.set("write", write)?;

        let user_delete = self.user_context.clone();
        let auditor_delete = self.auditor.clone();
        let script_delete = script_uri.to_string();
        let delete = Function::new(ctx.clone(), move |path: String| -> String {
            if crate::module_loader::is_root_module_name(&path) {
                return host_failure("Error", ENTRYPOINT_IS_NOT_A_FILE);
            }
            if let Err(e) = user_delete.require_capability(&Capability::DeleteAssets) {
                if let Ok(rt) = tokio::runtime::Handle::try_current() {
                    let auditor = auditor_delete.clone();
                    let user_id = user_delete.user_id.clone();
                    rt.spawn(async move {
                        let _ = auditor
                            .log_authz_failure(
                                user_id,
                                "asset".to_string(),
                                "delete".to_string(),
                                "DeleteAssets".to_string(),
                            )
                            .await;
                    });
                }
                return host_failure("Error", &format!("files.delete: {}", e));
            }
            audit_file_change(
                &auditor_delete,
                &user_delete,
                "delete",
                SecuritySeverity::High,
                &script_delete,
                &path,
            );
            if !repository::delete_asset(&script_delete, &path) {
                return host_ok(serde_json::Value::Bool(false));
            }
            // After a removal nothing else in the engine still holds the
            // content, which makes this the revision most worth having.
            crate::revisions::record_blocking(
                &script_delete,
                crate::revisions::Origin::Delete,
                user_delete.user_id.as_deref(),
            );
            host_ok(serde_json::Value::Bool(true))
        })?;
        host.set("delete", delete)?;

        ctx.globals().set("__hostFiles", host)?;
        crate::bytecode::eval_program(ctx, "engine://files-prelude", FILES_PRELUDE).map_err(
            |e| {
                rquickjs::Error::new_from_js_message(
                    "files",
                    "prelude",
                    &format!("files prelude failed to load: {}", e),
                )
            },
        )?;
        Ok(())
    }

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
    fn setup_secrets_functions(&self, ctx: &rquickjs::Ctx<'_>, script_uri: &str) -> JsResult<()> {
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
                    return Ok(refusal_answer(format!(
                        "Asset '{}' not found or not owned by script '{}'",
                        asset_name, script_uri_resource
                    )));
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
                    return Ok(refusal_answer(format!(
                        "Refused: '{}' is not under '{}', so it is not a file an MCP client \
                         may read. Move it to '{}{}' and register that. See \
                         GET /engine/exposure.",
                        asset_name,
                        crate::exposure::RESOURCE_DIR,
                        crate::exposure::RESOURCE_DIR,
                        asset_name,
                    )));
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

    /// `mcp.ask` / `mcp.canAsk` / `mcp.once`, over the thread-local exchange.
    ///
    /// Every binding here reads [`crate::mcp_elicitation`]'s thread-local
    /// rather than anything captured at install time, which is the one thing
    /// that has to be true of them: globals are installed once per execution,
    /// and what a call must see is the exchange the *current* tool call
    /// established. Installed unconditionally, because an execution with no
    /// exchange is not an error — it is a scheduled job or a listener, where
    /// `canAsk` is false and `ask` throws.
    fn setup_mcp_elicitation(&self, ctx: &rquickjs::Ctx<'_>, script_uri: &str) -> JsResult<()> {
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
    /// This was ungated entirely until now, which made it the way around
    /// [`super::capabilities::UserContext::attenuated`]: `sandbox.run` narrowed
    /// to hold neither capability still installed this class, so model-authored
    /// source could reach any public address and spend the person's API key
    /// getting there. A capability model enforced everywhere except through a
    /// secret name is not enforced where it matters.
    fn setup_mcp_client_class(&self, ctx: &rquickjs::Ctx<'_>, script_uri: &str) -> JsResult<()> {
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
        global.set("McpClient", mcp_client_class)?;

        debug!("McpClient class initialized for external MCP server connections");

        Ok(())
    }

    /// Install `__hostRouteRegistry` and the prelude that builds
    /// `routeRegistry` over it.
    ///
    /// One registration call, `registerRoute(path, spec)`, whose spec says what
    /// the path leads to: `{ handler }`, `{ stream: true }` or `{ file }`. The
    /// three used to be three functions with three signatures, and the internals
    /// had already become one record (`RouteMetadata` carries a `RouteKind`), so
    /// the only thing three names still bought was three ways to answer.
    ///
    /// They answered with strings. A refusal, a success and a misuse were all a
    /// sentence the script could not tell apart without parsing English. Now
    /// misuse throws — a malformed spec, a reserved path, a capability the
    /// caller lacks — and a *refusal* is a value, `{ ok: false, reason }`, so a
    /// script that gets one path wrong keeps the rest of its registrations.
    fn setup_route_registry(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
        register_fn: Option<RouteRegisterFn>,
    ) -> JsResult<()> {
        let host = rquickjs::Object::new(ctx.clone())?;

        let registrar = RouteRegistrar {
            user_context: self.user_context.clone(),
            auditor: self.auditor.clone(),
            script_uri: script_uri.to_string(),
            config: self.config.clone(),
            record: register_fn,
        };
        let register = Function::new(
            ctx.clone(),
            move |path: String, spec_json: String| -> String {
                let spec = match serde_json::from_str::<serde_json::Value>(&spec_json)
                    .map_err(|e| e.to_string())
                    .and_then(|value| parse_route_spec(&value))
                {
                    Ok(spec) => spec,
                    Err(message) => {
                        return host_failure(
                            "TypeError",
                            &format!("routeRegistry.registerRoute: {}", message),
                        );
                    }
                };
                match registrar.register(&path, spec) {
                    Ok(Registered::Done) => host_ok(serde_json::json!({ "ok": true })),
                    Ok(Registered::Refused(reason)) => {
                        host_ok(serde_json::json!({ "ok": false, "reason": reason }))
                    }
                    Err(failure) => host_failure(
                        failure.name,
                        &format!("routeRegistry.registerRoute: {}", failure.message),
                    ),
                }
            },
        )?;
        host.set("register", register)?;

        let user_ctx_send = self.user_context.clone();
        let auditor_send = self.auditor.clone();
        let send = Function::new(
            ctx.clone(),
            move |path: String, message: rquickjs::Value<'_>| -> JsResult<String> {
                // Typed `any` in the declarations and serialized here, so the
                // object every example passes is the object that arrives.
                let message = json_arg(message, "data")?;
                Ok(send_stream_message(
                    &user_ctx_send,
                    &auditor_send,
                    "sendStreamMessage",
                    &path,
                    &message,
                    None,
                ))
            },
        )?;
        host.set("send", send)?;

        let user_ctx_filtered = self.user_context.clone();
        let auditor_filtered = self.auditor.clone();
        let send_filtered = Function::new(
            ctx.clone(),
            move |path: String,
                  message: rquickjs::Value<'_>,
                  filter_json: Option<String>,
                  match_mode: Option<String>|
                  -> JsResult<String> {
                let message = json_arg(message, "data")?;
                // The prelude serialises the filter object, so a parse failure
                // here is the engine's bug rather than the caller's.
                let filter: HashMap<String, String> = match filter_json {
                    Some(json) => match serde_json::from_str(&json) {
                        Ok(filter) => filter,
                        Err(e) => {
                            return Ok(host_failure(
                                "TypeError",
                                &format!(
                                    "routeRegistry.sendStreamMessageFiltered: the filter maps \
                                     names to strings: {}",
                                    e
                                ),
                            ));
                        }
                    },
                    None => HashMap::new(),
                };
                let match_mode = match match_mode
                    .map(|raw| raw.parse::<crate::stream_registry::FilterMatchMode>())
                    .transpose()
                {
                    Ok(mode) => mode.unwrap_or(crate::stream_registry::FilterMatchMode::Subset),
                    Err(message) => {
                        return Ok(host_failure(
                            "TypeError",
                            &format!("routeRegistry.sendStreamMessageFiltered: {}", message),
                        ));
                    }
                };
                Ok(send_stream_message(
                    &user_ctx_filtered,
                    &auditor_filtered,
                    "sendStreamMessageFiltered",
                    &path,
                    &message,
                    Some((&filter, match_mode)),
                ))
            },
        )?;
        host.set("sendFiltered", send_filtered)?;

        ctx.globals().set("__hostRouteRegistry", host)?;
        crate::bytecode::eval_program(ctx, "engine://route-prelude", ROUTE_PRELUDE).map_err(
            |e| {
                rquickjs::Error::new_from_js_message(
                    "routeRegistry",
                    "prelude",
                    &format!("route prelude failed to load: {}", e),
                )
            },
        )?;

        Ok(())
    }

    /// Setup fetch() function for HTTP requests with secret injection
    fn setup_fetch_function(&self, ctx: &rquickjs::Ctx<'_>, script_uri: &str) -> JsResult<()> {
        let global = ctx.globals();
        let script_uri_owned = script_uri.to_string();
        // Capture the user_id at script setup time for secret lookup in
        // user_secrets.
        //
        // Withheld when this execution is acting for somebody who did not
        // authorise it to use their secrets. The lookup then finds no personal
        // key and falls back to the script's own, which is what an undelegated
        // background task already gets — so a narrower grant lands the caller
        // in the weaker position rather than in an error, and `{{secret:...}}`
        // goes on meaning what it means.
        let user_id_for_fetch = if self
            .config
            .allows_delegated(crate::delegation::Scope::Secrets)
        {
            self.user_context.user_id.clone()
        } else {
            None
        };

        // Checked inside the call rather than by withholding `__hostFetch`:
        // the prelude defines `fetch` on top of it and a missing global would
        // be a `ReferenceError` naming an engine-private name, where a script
        // that is not allowed out wants to hear that and catch it.
        let user_ctx_fetch = self.user_context.clone();
        // Whether a `{{secret:...}}` template in this request may be resolved.
        // Withheld separately from the network itself, because "may call an
        // API" and "may use the key" are different grants: model-authored code
        // that may fetch a public endpoint should not thereby reach the
        // account's credentials.
        let may_read_secrets = self.user_context.has_capability(&Capability::ReadSecrets);

        // Clones for the parallel and streaming bindings below, which resolve
        // secrets against the same person this one does.
        let user_id_all = user_id_for_fetch.clone();
        let user_id_stream = user_id_for_fetch.clone();

        // Create the fetch function (synchronous version)
        let fetch_fn = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  url: String,
                  options_json: Option<String>|
                  -> JsResult<String> {
                if !user_ctx_fetch.has_capability(&Capability::UseNetwork) {
                    return Err(capability_error(
                        "fetch",
                        &Capability::UseNetwork,
                        &user_ctx_fetch,
                    ));
                }

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
                // The engine's field, written after the caller's options are
                // parsed. It is `#[serde(skip)]`, so nothing the script wrote
                // could have set it — this is the only way it is ever filled.
                let mut options = options;
                options.network_scope = user_ctx_fetch.network_scope.clone();

                // A template this execution may not resolve is refused, not
                // sent as itself: a request carrying the literal
                // `{{secret:...}}` in an `Authorization` header is a
                // credential-shaped string going to a third party, and the
                // 401 that comes back explains nothing. This differs from a
                // missing delegation scope on purpose — there the person is
                // absent and falling back to the script's own key is the
                // weaker position, here the caller asked to hold less and
                // should hear that it does.
                if !may_read_secrets && crate::http_client::names_a_secret(&url, &options) {
                    return Err(capability_error(
                        "fetch",
                        &Capability::ReadSecrets,
                        &user_ctx_fetch,
                    ));
                }

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

        // `__hostFetchAll` — several requests in flight at once.
        //
        // `fetch` is a synchronous host call, so `Promise.all` over three of
        // them sequences them and the wall clock is the sum. For an agent
        // running three tool calls that is the difference between fitting
        // inside the execution budget and not.
        //
        // Same checks per request as a single `fetch`, because it *is* a
        // single fetch per request — only on the blocking pool rather than
        // on this thread. The capability gates are here rather than inside,
        // so a refused batch costs no connections.
        let script_uri_all = script_uri.to_string();
        let user_ctx_all = self.user_context.clone();
        let fetch_all = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, requests_json: String| -> JsResult<String> {
                if !user_ctx_all.has_capability(&Capability::UseNetwork) {
                    return Err(capability_error(
                        "fetchAll",
                        &Capability::UseNetwork,
                        &user_ctx_all,
                    ));
                }

                let described: Vec<FetchRequestSpec> = serde_json::from_str(&requests_json)
                    .map_err(|e| {
                        rquickjs::Error::new_from_js_message(
                            "fetchAll",
                            "requests",
                            &format!("Invalid request list: {}", e),
                        )
                    })?;

                // Checked across the whole batch before any of it is sent,
                // for the reason a single `fetch` checks before sending: a
                // template this execution may not resolve must not reach a
                // third party as itself.
                if !may_read_secrets
                    && described.iter().any(|request| {
                        crate::http_client::names_a_secret(&request.url, &request.options)
                    })
                {
                    return Err(capability_error(
                        "fetchAll",
                        &Capability::ReadSecrets,
                        &user_ctx_all,
                    ));
                }

                let requests = described
                    .into_iter()
                    .map(|request| crate::http_client::ParallelRequest {
                        url: request.url,
                        options: crate::http_client::FetchOptions {
                            // Per request, because each carries its own
                            // options object and a batch that bounded only the
                            // first would bound nothing.
                            network_scope: user_ctx_all.network_scope.clone(),
                            ..request.options
                        },
                    })
                    .collect();

                // Each answer is its own envelope. One refused URL is an
                // error in its own slot rather than a failed batch: the
                // caller asked for several answers and has a use for the
                // ones that arrived.
                let client = crate::http_client::HttpClient::new().map_err(|e| {
                    rquickjs::Error::new_from_js_message(
                        "fetchAll",
                        "client_init",
                        &format!("Failed to create HTTP client: {}", e),
                    )
                })?;

                let answers: Vec<serde_json::Value> = client
                    .fetch_all(requests, Some(&script_uri_all), user_id_all.as_deref())
                    .into_iter()
                    .map(|answer| match answer {
                        Ok(response) => serde_json::json!({
                            "ok": true,
                            "response": serde_json::to_value(&response).unwrap_or_default(),
                        }),
                        Err(e) => serde_json::json!({
                            "ok": false,
                            "error": e.to_string(),
                        }),
                    })
                    .collect();

                Ok(serde_json::Value::Array(answers).to_string())
            },
        )?;
        global.set("__hostFetchAll", fetch_all)?;

        // `__hostFetchStreamStart` — a response read a piece at a time.
        //
        // What a buffered `fetch` cannot do: consume a model's token stream,
        // an events endpoint, a log tail on another service. The connection
        // stays open between host calls, held against this execution and
        // dropped with it.
        let script_uri_stream = script_uri.to_string();
        let user_ctx_stream = self.user_context.clone();
        let stream_start = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  url: String,
                  options_json: Option<String>|
                  -> JsResult<String> {
                if !user_ctx_stream.has_capability(&Capability::UseNetwork) {
                    return Err(capability_error(
                        "fetchStream",
                        &Capability::UseNetwork,
                        &user_ctx_stream,
                    ));
                }

                let options: crate::http_client::FetchOptions = match options_json {
                    Some(json) => serde_json::from_str(&json).map_err(|e| {
                        rquickjs::Error::new_from_js_message(
                            "fetchStream",
                            "options",
                            &format!("Invalid fetch options: {}", e),
                        )
                    })?,
                    None => Default::default(),
                };
                let mut options = options;
                options.network_scope = user_ctx_stream.network_scope.clone();

                if !may_read_secrets && crate::http_client::names_a_secret(&url, &options) {
                    return Err(capability_error(
                        "fetchStream",
                        &Capability::ReadSecrets,
                        &user_ctx_stream,
                    ));
                }

                let client = crate::http_client::HttpClient::new().map_err(|e| {
                    rquickjs::Error::new_from_js_message(
                        "fetchStream",
                        "client_init",
                        &format!("Failed to create HTTP client: {}", e),
                    )
                })?;

                let stream = client
                    .fetch_streaming(
                        url,
                        options,
                        Some(&script_uri_stream),
                        user_id_stream.as_deref(),
                    )
                    .map_err(|e| {
                        rquickjs::Error::new_from_js_message(
                            "fetchStream",
                            "request_failed",
                            &format!("Fetch error: {}", e),
                        )
                    })?;

                let opening = serde_json::json!({
                    "status": stream.status,
                    "ok": stream.ok,
                    "headers": stream.headers,
                });

                let id = crate::http_client::register_stream(stream).map_err(|e| {
                    rquickjs::Error::new_from_js_message("fetchStream", "too_many", &e.to_string())
                })?;

                let mut opening = opening;
                opening["streamId"] = serde_json::json!(id.to_string());
                Ok(opening.to_string())
            },
        )?;
        global.set("__hostFetchStreamStart", stream_start)?;

        // Blocks until the next piece arrives, which is the point: the
        // caller has asked for the next token and has nothing to do until it
        // has one. An ended stream answers `done` rather than an error, so a
        // loop over it terminates without a `try`.
        let stream_read = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, stream_id: String| -> JsResult<String> {
                let Ok(id) = stream_id.parse::<u64>() else {
                    return Err(rquickjs::Error::new_from_js_message(
                        "fetchStream",
                        "read",
                        "that is not a stream id",
                    ));
                };

                match crate::http_client::read_stream(id) {
                    Ok(Some(chunk)) => Ok(serde_json::json!({
                        "done": false,
                        "value": chunk,
                    })
                    .to_string()),
                    Ok(None) => Ok(serde_json::json!({ "done": true }).to_string()),
                    Err(e) => Err(rquickjs::Error::new_from_js_message(
                        "fetchStream",
                        "read",
                        &e.to_string(),
                    )),
                }
            },
        )?;
        global.set("__hostFetchStreamRead", stream_read)?;

        let stream_close = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, stream_id: String| -> JsResult<bool> {
                Ok(stream_id
                    .parse::<u64>()
                    .map(crate::http_client::close_stream)
                    .unwrap_or(false))
            },
        )?;
        global.set("__hostFetchStreamClose", stream_close)?;

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

        // database.ensureTable(tableName, schemaJson) - Converge a table's shape
        let script_uri_ensure = script_uri_owned.clone();
        let user_ctx_ensure = user_context.clone();
        let ensure_table = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  table_name: String,
                  schema_json: String|
                  -> JsResult<String> {
                debug!(
                    "database.ensureTable called for script {} with table: {}",
                    script_uri_ensure, table_name
                );

                if user_ctx_ensure
                    .require_capability(&crate::security::Capability::ManageScriptDatabase)
                    .is_err()
                {
                    return Ok(error_answer(
                        "Insufficient permissions for database schema operations",
                    ));
                }

                let spec = match parse_table_spec(&schema_json) {
                    Ok(spec) => spec,
                    Err(e) => return Ok(error_answer(e)),
                };

                match crate::repository::ensure_script_table(&script_uri_ensure, &table_name, &spec)
                {
                    Ok(ensured) => Ok(success_answer(
                        serde_json::to_value(&ensured).unwrap_or_else(|_| serde_json::json!({})),
                    )),
                    Err(e) => Ok(error_answer(e)),
                }
            },
        )?;
        database_obj.set("ensureTable", ensure_table)?;

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
                            Ok(success_answer(serde_json::json!({
                                "tableName": table_name,
                                "columnName": column_name,
                                "dropped": true,
                            })))
                        } else {
                            Ok(success_answer(serde_json::json!({
                                "tableName": table_name,
                                "columnName": column_name,
                                "dropped": false,
                                "message": "Column did not exist",
                            })))
                        }
                    }
                    Err(e) => Ok(error_answer(e)),
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
                            Ok(success_answer(serde_json::json!({
                                "tableName": table_name,
                                "dropped": true,
                            })))
                        } else {
                            Ok(success_answer(serde_json::json!({
                                "tableName": table_name,
                                "dropped": false,
                                "message": "Table did not exist",
                            })))
                        }
                    }
                    Err(e) => Ok(error_answer(e)),
                }
            },
        )?;
        database_obj.set("dropTable", drop_table)?;

        // query(tableName, filters, limit, orderBy, orderDir, options): the prelude
        // maps `database.query`'s options object onto these positions.
        // filters supports equality {"col": val} and range operators {"col": {"$gt": val, ...}}
        // options supports {"forUpdate": true} to hold the returned rows for
        // the rest of the transaction.
        let script_uri_query = script_uri_owned.clone();
        let user_ctx_query = user_context.clone();
        let query_table = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  table_name: String,
                  filters: Opt<rquickjs::Value<'_>>,
                  limit: Opt<rquickjs::Value<'_>>,
                  order_by: Opt<rquickjs::Value<'_>>,
                  order_dir: Opt<rquickjs::Value<'_>>,
                  options: Opt<rquickjs::Value<'_>>|
                  -> JsResult<String> {
                debug!(
                    "database.query called for script {} on table: {}",
                    script_uri_query, table_name
                );

                if !user_ctx_query.has_capability(&Capability::ReadScriptData) {
                    return Ok(error_answer(capability_refusal(
                        "database",
                        &Capability::ReadScriptData,
                        &user_ctx_query,
                    )));
                }

                let filters_arg: Option<String> = match optional_arg(filters, "filters") {
                    Ok(value) => value,
                    Err(message) => return Ok(error_answer(message)),
                };
                let limit_arg: Option<i32> = match optional_arg(limit, "limit") {
                    Ok(value) => value,
                    Err(message) => return Ok(error_answer(message)),
                };
                let order_by_arg: Option<String> = match optional_arg(order_by, "orderBy") {
                    Ok(value) => value,
                    Err(message) => return Ok(error_answer(message)),
                };
                let order_dir_arg: Option<String> = match optional_arg(order_dir, "orderDir") {
                    Ok(value) => value,
                    Err(message) => return Ok(error_answer(message)),
                };
                let options_arg: Option<String> = match optional_arg(options, "options") {
                    Ok(value) => value,
                    Err(message) => return Ok(error_answer(message)),
                };

                let filters_map = if let Some(filters_str) = filters_arg {
                    match serde_json::from_str::<std::collections::HashMap<String, serde_json::Value>>(
                        &filters_str,
                    ) {
                        Ok(map) => Some(map),
                        Err(e) => {
                            return Ok(error_answer(format!("Invalid filters JSON: {}", e)));
                        }
                    }
                } else {
                    None
                };

                let query_options = match build_query_options(
                    limit_arg,
                    order_by_arg,
                    order_dir_arg,
                    options_arg.as_deref(),
                ) {
                    Ok(parsed) => parsed,
                    Err(message) => return Ok(error_answer(message)),
                };

                match crate::repository::query_table(
                    &script_uri_query,
                    &table_name,
                    filters_map.as_ref(),
                    &query_options,
                ) {
                    Ok(rows) => match serde_json::to_string(&rows) {
                        Ok(json) => Ok(json),
                        Err(e) => Ok(error_answer(format!("Serialization error: {}", e))),
                    },
                    Err(e) => Ok(error_answer(e)),
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

                if !user_ctx_insert.has_capability(&Capability::WriteScriptData) {
                    return Ok(error_answer(capability_refusal(
                        "database",
                        &Capability::WriteScriptData,
                        &user_ctx_insert,
                    )));
                }

                // Parse data from JSON string
                let data_map = match serde_json::from_str::<
                    std::collections::HashMap<String, serde_json::Value>,
                >(&data)
                {
                    Ok(map) => map,
                    Err(e) => return Ok(error_answer(format!("Invalid data JSON: {}", e))),
                };

                match crate::repository::insert_row(&script_uri_insert, &table_name, &data_map) {
                    Ok(row) => match serde_json::to_string(&row) {
                        Ok(json) => Ok(json),
                        Err(e) => Ok(error_answer(format!("Serialization error: {}", e))),
                    },
                    Err(e) => Ok(error_answer(e)),
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

                if !user_ctx_update.has_capability(&Capability::WriteScriptData) {
                    return Ok(error_answer(capability_refusal(
                        "database",
                        &Capability::WriteScriptData,
                        &user_ctx_update,
                    )));
                }

                // Parse data from JSON string
                let data_map = match serde_json::from_str::<
                    std::collections::HashMap<String, serde_json::Value>,
                >(&data)
                {
                    Ok(map) => map,
                    Err(e) => return Ok(error_answer(format!("Invalid data JSON: {}", e))),
                };

                match crate::repository::update_row(&script_uri_update, &table_name, id, &data_map)
                {
                    Ok(row) => match serde_json::to_string(&row) {
                        Ok(json) => Ok(json),
                        Err(e) => Ok(error_answer(format!("Serialization error: {}", e))),
                    },
                    Err(e) => Ok(error_answer(e)),
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

                if !user_ctx_delete.has_capability(&Capability::WriteScriptData) {
                    return Ok(error_answer(capability_refusal(
                        "database",
                        &Capability::WriteScriptData,
                        &user_ctx_delete,
                    )));
                }

                match crate::repository::delete_row(&script_uri_delete, &table_name, id) {
                    Ok(deleted) => Ok(success_answer(serde_json::json!({ "deleted": deleted }))),
                    Err(e) => Ok(error_answer(e)),
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

                if !user_ctx_upsert.has_capability(&Capability::WriteScriptData) {
                    return Ok(error_answer(capability_refusal(
                        "database",
                        &Capability::WriteScriptData,
                        &user_ctx_upsert,
                    )));
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
                    Err(e) => return Ok(error_answer(format!("Invalid data JSON: {}", e))),
                };

                match crate::repository::upsert_row(
                    &script_uri_upsert,
                    &table_name,
                    &key_cols,
                    &data_map,
                ) {
                    Ok(row) => match serde_json::to_string(&row) {
                        Ok(json) => Ok(json),
                        Err(e) => Ok(error_answer(format!("Serialization error: {}", e))),
                    },
                    Err(e) => Ok(error_answer(e)),
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

                if !user_ctx_dw.has_capability(&Capability::WriteScriptData) {
                    return Ok(error_answer(capability_refusal(
                        "database",
                        &Capability::WriteScriptData,
                        &user_ctx_dw,
                    )));
                }

                let filters_map = match serde_json::from_str::<
                    std::collections::HashMap<String, serde_json::Value>,
                >(&filters)
                {
                    Ok(map) => map,
                    Err(e) => return Ok(error_answer(format!("Invalid filters JSON: {}", e))),
                };

                match crate::repository::delete_where(&script_uri_dw, &table_name, &filters_map) {
                    Ok(count) => Ok(success_answer(serde_json::json!({ "deleted": count }))),
                    Err(e) => Ok(error_answer(e)),
                }
            },
        )?;
        database_obj.set("deleteWhere", delete_where)?;

        // Transaction management functions

        // beginTransaction(timeoutMs?): start a transaction, or a savepoint inside one.
        // These three are the host half of `database.transaction(fn)`.
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
                    Err(e) => Ok(error_answer(e)),
                }
            },
        )?;
        database_obj.set("beginTransaction", begin_transaction)?;

        // commitTransaction(): commit, or release the innermost savepoint
        let commit_transaction = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
                match crate::database::Database::commit_transaction() {
                    Ok(()) => Ok(
                        "{\"success\": true, \"message\": \"Transaction committed\"}".to_string(),
                    ),
                    Err(e) => Ok(error_answer(e)),
                }
            },
        )?;
        database_obj.set("commitTransaction", commit_transaction)?;

        // rollbackTransaction(): roll back, or to the innermost savepoint
        let rollback_transaction = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
                match crate::database::Database::rollback_transaction() {
                    Ok(()) => Ok(
                        "{\"success\": true, \"message\": \"Transaction rolled back\"}".to_string(),
                    ),
                    Err(e) => Ok(error_answer(e)),
                }
            },
        )?;
        database_obj.set("rollbackTransaction", rollback_transaction)?;

        // Installed under a private name: the prelude below builds `database`
        // from it, turning each JSON answer into a value and each `{error}`
        // into a thrown error.
        global.set("__hostDatabase", database_obj)?;

        crate::bytecode::eval_program(ctx, "engine://database-prelude", DATABASE_PRELUDE).map_err(
            |e| {
                rquickjs::Error::new_from_js_message(
                    "database",
                    "prelude",
                    &format!("database prelude failed to load: {}", e),
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
    fn delegated_user_id(ctx: &rquickjs::Ctx<'_>, allowed: bool) -> Option<String> {
        if !allowed {
            return None;
        }
        Self::current_user_id(ctx)
    }

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

    fn setup_user_properties_functions(
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

    fn setup_scheduler_functions(&self, ctx: &rquickjs::Ctx<'_>, script_uri: &str) -> JsResult<()> {
        // `__hostScheduler` answers in the envelope `scheduler_prelude.js`
        // unwraps, and the prelude builds `schedulerService` from it. The
        // object is present in every context; registering is what depends on
        // the phase, and outside it a registration is refused as a value —
        // `{ ok: false, reason }` — exactly as `routeRegistry.registerRoute`'s
        // is, since top-level code runs on every execution and must not throw.
        let host = rquickjs::Object::new(ctx.clone())?;
        let scheduler_handle = scheduler::get_scheduler();

        /// Read `handler` off a registration's options, or say what is wrong.
        fn handler_of(options: &rquickjs::Object<'_>, api: &str) -> Result<String, String> {
            let handler: String = options
                .get("handler")
                .map_err(|_| format!("{}: options.handler is required", api))?;
            let handler = handler.trim();
            if handler.is_empty() {
                return Err(format!("{}: options.handler must name a function", api));
            }
            Ok(handler.to_string())
        }

        fn refused(reason: String) -> String {
            host_ok(serde_json::json!({ "ok": false, "reason": reason }))
        }

        let register_once_handle = scheduler_handle.clone();
        let script_uri_once = script_uri.to_string();
        let config_once = self.config.clone();
        let register_once = Function::new(
            ctx.clone(),
            move |options: rquickjs::Object<'_>| -> String {
                const API: &str = "schedulerService.registerOnce";
                let handler_name = match handler_of(&options, API) {
                    Ok(handler) => handler,
                    Err(message) => return host_failure("TypeError", &message),
                };
                let run_at_value: String = match options.get("runAt") {
                    Ok(value) => value,
                    Err(_) => {
                        return host_failure(
                            "TypeError",
                            &format!("{}: options.runAt is required (a UTC ISO string)", API),
                        );
                    }
                };
                let run_at = match scheduler::parse_utc_timestamp(&run_at_value) {
                    Ok(ts) => ts,
                    Err(err) => return host_failure("TypeError", &format!("{}: {}", API, err)),
                };
                if !config_once.registration_phase {
                    return refused(registration_inactive(API, &handler_name));
                }

                let name = options.get::<_, String>("name").ok();
                if config_once
                    .collect(
                        CollectedRegistration::new(
                            RegistrationKind::ScheduledJob,
                            name.clone().unwrap_or_else(|| handler_name.clone()),
                        )
                        .with_handler(handler_name.clone()),
                    )
                    .is_some()
                {
                    return host_ok(serde_json::json!({ "ok": true }));
                }

                match register_once_handle.register_one_off(
                    &script_uri_once,
                    &handler_name,
                    name,
                    run_at,
                ) {
                    Ok(job) => host_ok(serde_json::json!({
                        "ok": true,
                        "jobId": job.id.to_string(),
                        "name": job.key,
                        "nextRun": job.schedule.next_run().to_rfc3339(),
                    })),
                    Err(err) => host_failure("Error", &format!("{}: {}", API, err)),
                }
            },
        )?;

        let register_recurring_handle = scheduler_handle.clone();
        let script_uri_recurring = script_uri.to_string();
        let config_recurring = self.config.clone();
        let register_recurring = Function::new(
            ctx.clone(),
            move |options: rquickjs::Object<'_>| -> String {
                const API: &str = "schedulerService.registerRecurring";
                let handler_name = match handler_of(&options, API) {
                    Ok(handler) => handler,
                    Err(message) => return host_failure("TypeError", &message),
                };

                let interval_ms_opt = options.get::<_, f64>("intervalMilliseconds").ok();
                let interval_min_opt = options.get::<_, f64>("intervalMinutes").ok();
                let interval = match (interval_ms_opt, interval_min_opt) {
                    (Some(_), Some(_)) => {
                        return host_failure(
                            "TypeError",
                            &format!(
                                "{}: give intervalMilliseconds or intervalMinutes, not both",
                                API
                            ),
                        );
                    }
                    (Some(ms), None) if ms.is_finite() && ms >= 100.0 => {
                        ChronoDuration::milliseconds(ms.floor() as i64)
                    }
                    (Some(_), None) => {
                        return host_failure(
                            "RangeError",
                            &format!("{}: intervalMilliseconds must be at least 100", API),
                        );
                    }
                    (None, Some(min)) if min.is_finite() && min >= 1.0 => {
                        ChronoDuration::minutes(min.floor() as i64)
                    }
                    (None, Some(_)) => {
                        return host_failure(
                            "RangeError",
                            &format!("{}: intervalMinutes must be at least 1", API),
                        );
                    }
                    (None, None) => {
                        return host_failure(
                            "TypeError",
                            &format!(
                                "{}: options.intervalMilliseconds or options.intervalMinutes is required",
                                API
                            ),
                        );
                    }
                };

                let first_run = match options.get::<_, String>("startAt") {
                    Ok(start_at) => match scheduler::parse_utc_timestamp(&start_at) {
                        Ok(ts) => Some(ts),
                        Err(err) => {
                            return host_failure("TypeError", &format!("{}: {}", API, err));
                        }
                    },
                    Err(_) => None,
                };
                if !config_recurring.registration_phase {
                    return refused(registration_inactive(API, &handler_name));
                }

                let name = options.get::<_, String>("name").ok();
                if config_recurring
                    .collect(
                        CollectedRegistration::new(
                            RegistrationKind::ScheduledJob,
                            name.clone().unwrap_or_else(|| handler_name.clone()),
                        )
                        .with_handler(handler_name.clone()),
                    )
                    .is_some()
                {
                    return host_ok(serde_json::json!({ "ok": true }));
                }

                match register_recurring_handle.register_recurring(
                    &script_uri_recurring,
                    &handler_name,
                    name,
                    interval,
                    first_run,
                ) {
                    Ok(job) => host_ok(serde_json::json!({
                        "ok": true,
                        "jobId": job.id.to_string(),
                        "name": job.key,
                        "nextRun": job.schedule.next_run().to_rfc3339(),
                    })),
                    Err(err) => host_failure("Error", &format!("{}: {}", API, err)),
                }
            },
        )?;

        let script_uri_clear = script_uri.to_string();
        let config_clear = self.config.clone();
        let clear_all = Function::new(ctx.clone(), move || -> String {
            // Clearing is the inverse of registering and mutates the same
            // registry, so it follows the same phase rule.
            if !config_clear.registration_phase {
                return refused(
                    "schedulerService.clearAll: no jobs cleared - scheduled job changes only \
                     take effect during script startup and init()"
                        .to_string(),
                );
            }
            // A dry run reports what a script would register, and an emptied
            // job table is not part of that.
            if config_clear.is_dry_run() {
                return host_ok(serde_json::json!({ "ok": true, "cleared": 0 }));
            }
            let removed = scheduler::clear_script_jobs(&script_uri_clear);
            host_ok(serde_json::json!({ "ok": true, "cleared": removed }))
        })?;

        host.set("registerOnce", register_once)?;
        host.set("registerRecurring", register_recurring)?;
        host.set("clearAll", clear_all)?;
        ctx.globals().set("__hostScheduler", host)?;
        crate::bytecode::eval_program(ctx, "engine://scheduler-prelude", SCHEDULER_PRELUDE)
            .map_err(|e| {
                rquickjs::Error::new_from_js_message(
                    "schedulerService",
                    "prelude",
                    &format!("scheduler prelude failed to load: {}", e),
                )
            })?;

        Ok(())
    }

    /// The Rust half of `scriptTasks` — the durable queue ([`crate::tasks`]).
    ///
    /// Deliberately *not* phase-gated, unlike `schedulerService` beside it.
    /// That gate is right for a declaration, which belongs to the version of
    /// the code that declared it; a task is a piece of work that exists because
    /// something happened, and the shape it is for — start during a request,
    /// answer, finish afterwards — only works if a handler can enqueue.
    ///
    /// Each method answers with an envelope rather than a value, and
    /// `tasks_prelude.js` turns that into a returned object or a thrown
    /// `Error`. A host binding cannot throw the right kind of exception, and
    /// answering `"Error: ..."` as the value is indistinguishable from an
    /// answer that happens to be a string.
    fn setup_task_functions(&self, ctx: &rquickjs::Ctx<'_>, script_uri: &str) -> JsResult<()> {
        let global = ctx.globals();
        let host = rquickjs::Object::new(ctx.clone())?;

        let script_uri_enqueue = script_uri.to_string();
        let config_enqueue = self.config.clone();
        let user_enqueue = self.user_context.clone();
        let enqueue = Function::new(
            ctx.clone(),
            move |options_json: String| -> JsResult<String> {
                if config_enqueue.is_dry_run() {
                    // A check that deploys nothing must not leave work behind
                    // for a worker to pick up afterwards.
                    return Ok(host_failure(
                        "DryRunError",
                        "scriptTasks.enqueue: nothing was enqueued - this is a dry run",
                    ));
                }

                // Queueing is how an execution outlives itself: a task runs
                // later, in script context, holding what the script holds
                // rather than what this turn was narrowed to. So a turn that
                // may not write must not be able to queue a write for
                // afterwards — that is the whole of the loophole, and it is
                // closed here rather than at claim time because the narrowing
                // is a fact about this execution and nothing in the row
                // remembers it.
                if !user_enqueue.has_capability(&Capability::EnqueueTasks) {
                    return Ok(host_failure(
                        "SecurityError",
                        &capability_refusal(
                            "scriptTasks.enqueue",
                            &Capability::EnqueueTasks,
                            &user_enqueue,
                        ),
                    ));
                }

                let options: serde_json::Value = match serde_json::from_str(&options_json) {
                    Ok(options) => options,
                    Err(e) => {
                        return Ok(host_failure(
                            "TypeError",
                            &format!("scriptTasks.enqueue: options are not valid JSON: {}", e),
                        ));
                    }
                };

                let lane = match Self::lane_from_options(&options) {
                    Ok(lane) => lane,
                    Err(message) => {
                        return Ok(host_failure(
                            "TypeError",
                            &format!("scriptTasks.enqueue: {}", message),
                        ));
                    }
                };

                let run_at = match options.get("runAt").and_then(|v| v.as_str()) {
                    Some(value) => match crate::scheduler::parse_utc_timestamp(value) {
                        Ok(parsed) => Some(parsed),
                        Err(_) => {
                            return Ok(host_failure(
                                "RangeError",
                                "scriptTasks.enqueue: runAt must be a UTC timestamp ending with 'Z'",
                            ));
                        }
                    },
                    None => None,
                };

                let max_attempts = match options.get("maxAttempts") {
                    Some(serde_json::Value::Null) | None => None,
                    Some(value) => match value.as_i64() {
                        Some(number) if (i32::MIN as i64..=i32::MAX as i64).contains(&number) => {
                            Some(number as i32)
                        }
                        _ => {
                            return Ok(host_failure(
                                "RangeError",
                                "scriptTasks.enqueue: maxAttempts must be a whole number",
                            ));
                        }
                    },
                };

                let new_task = crate::tasks::NewTask {
                    script_uri: script_uri_enqueue.clone(),
                    handler_name: options
                        .get("handler")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    payload: options
                        .get("payload")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({})),
                    run_at,
                    max_attempts,
                    // Recorded as where the work came from, not as an authority
                    // it runs under: a task runs in script context.
                    enqueued_by: user_enqueue.user_id.clone(),
                    // Script context. `personalTasks.enqueue` is the one that
                    // acts as somebody, and it takes a grant to do it.
                    run_as: None,
                    // No default here, unlike the personal queue. A script
                    // task belongs to the solution rather than to one
                    // person, so there is nothing for the engine to infer a
                    // lane from — and defaulting every script task into one
                    // lane would serialise the whole queue.
                    lane: lane.flatten(),
                };

                match crate::tasks::blocking::enqueue(new_task) {
                    Ok(task) => Ok(host_ok(crate::tasks::to_json(&task))),
                    Err(e) => Ok(host_failure(
                        Self::task_error_name(&e),
                        &format!("scriptTasks.enqueue: {}", e),
                    )),
                }
            },
        )?;
        host.set("enqueue", enqueue)?;

        let config_cancel = self.config.clone();
        let user_cancel = self.user_context.clone();
        let cancel = Function::new(ctx.clone(), move |task_id: String| -> JsResult<String> {
            if config_cancel.is_dry_run() {
                return Ok(host_failure(
                    "DryRunError",
                    "scriptTasks.cancel: nothing was cancelled - this is a dry run",
                ));
            }

            // Cancelling is changing the queue, so it takes the same
            // capability enqueueing does. A narrowed turn that could delete
            // work the script had accepted would be a write by another name.
            if !user_cancel.has_capability(&Capability::EnqueueTasks) {
                return Ok(host_failure(
                    "SecurityError",
                    &capability_refusal(
                        "scriptTasks.cancel",
                        &Capability::EnqueueTasks,
                        &user_cancel,
                    ),
                ));
            }

            let Ok(parsed) = uuid::Uuid::parse_str(task_id.trim()) else {
                return Ok(host_failure(
                    "TypeError",
                    "scriptTasks.cancel: that is not a task id",
                ));
            };

            match crate::tasks::blocking::cancel(parsed) {
                Ok(cancelled) => Ok(host_ok(serde_json::Value::Bool(cancelled))),
                Err(e) => Ok(host_failure("Error", &format!("scriptTasks.cancel: {}", e))),
            }
        })?;
        host.set("cancel", cancel)?;

        // Scoped to the calling script, so one script cannot read another's
        // queue by holding an id. Everything else here is already per script
        // because the URI comes from the binding rather than the caller.
        let script_uri_get = script_uri.to_string();
        let get = Function::new(ctx.clone(), move |task_id: String| -> JsResult<String> {
            let Ok(parsed) = uuid::Uuid::parse_str(task_id.trim()) else {
                return Ok(host_failure(
                    "TypeError",
                    "scriptTasks.get: that is not a task id",
                ));
            };

            match crate::tasks::blocking::get(parsed) {
                Ok(Some(task)) if task.script_uri == script_uri_get => {
                    Ok(host_ok(crate::tasks::to_json(&task)))
                }
                Ok(_) => Ok(host_ok(serde_json::Value::Null)),
                Err(e) => Ok(host_failure("Error", &format!("scriptTasks.get: {}", e))),
            }
        })?;
        host.set("get", get)?;

        // `personalTasks` — the same queue, acting as the person who asked.
        //
        // Three things have to hold before a row is written, and all three are
        // checked again when the task runs: there is an authenticated user,
        // they have granted this script a delegation, and it has not lapsed.
        // Checking here as well is not redundant — it is what lets a script
        // find out *now* that it needs to send someone to the consent page,
        // rather than queueing work that will be abandoned later.
        let script_uri_personal = script_uri.to_string();
        let config_personal = self.config.clone();
        let user_personal_enqueue = self.user_context.clone();
        let personal_enqueue = Function::new(
            ctx.clone(),
            move |ctx: rquickjs::Ctx<'_>, options_json: String| -> JsResult<String> {
                if config_personal.is_dry_run() {
                    return Ok(host_failure(
                        "DryRunError",
                        "personalTasks.enqueue: nothing was enqueued - this is a dry run",
                    ));
                }

                // As `scriptTasks.enqueue`: work queued now runs later under
                // what the grant allows, which is not what this turn was
                // narrowed to.
                if !user_personal_enqueue.has_capability(&Capability::EnqueueTasks) {
                    return Ok(host_failure(
                        "SecurityError",
                        &capability_refusal(
                            "personalTasks.enqueue",
                            &Capability::EnqueueTasks,
                            &user_personal_enqueue,
                        ),
                    ));
                }

                // The person this would act as is the one making the request,
                // read from the live context rather than from the binding: a
                // script serving a request runs under the requesting user, and
                // that is who is in a position to have consented.
                let Some(user_id) = Self::current_user_id(&ctx) else {
                    return Ok(host_failure(
                        "SecurityError",
                        "personalTasks.enqueue requires an authenticated user",
                    ));
                };

                match crate::database::run_blocking(crate::delegation::get(
                    &user_id,
                    &script_uri_personal,
                )) {
                    Ok(Some(grant)) if grant.is_live(chrono::Utc::now()) => {}
                    Ok(Some(_)) => {
                        return Ok(host_failure(
                            "SecurityError",
                            "personalTasks.enqueue: this person's authorisation for this script has expired",
                        ));
                    }
                    Ok(None) => {
                        return Ok(host_failure(
                            "SecurityError",
                            "personalTasks.enqueue: this person has not authorised this script to act for them",
                        ));
                    }
                    Err(e) => {
                        return Ok(host_failure(
                            "Error",
                            &format!(
                                "personalTasks.enqueue: could not read the authorisation: {}",
                                e
                            ),
                        ));
                    }
                }

                let options: serde_json::Value = match serde_json::from_str(&options_json) {
                    Ok(options) => options,
                    Err(e) => {
                        return Ok(host_failure(
                            "TypeError",
                            &format!("personalTasks.enqueue: options are not valid JSON: {}", e),
                        ));
                    }
                };

                let lane = match Self::lane_from_options(&options) {
                    Ok(lane) => lane,
                    Err(message) => {
                        return Ok(host_failure(
                            "TypeError",
                            &format!("personalTasks.enqueue: {}", message),
                        ));
                    }
                };

                let run_at = match options.get("runAt").and_then(|v| v.as_str()) {
                    Some(value) => match crate::scheduler::parse_utc_timestamp(value) {
                        Ok(parsed) => Some(parsed),
                        Err(_) => {
                            return Ok(host_failure(
                                "RangeError",
                                "personalTasks.enqueue: runAt must be a UTC timestamp ending with 'Z'",
                            ));
                        }
                    },
                    None => None,
                };

                let new_task = crate::tasks::NewTask {
                    script_uri: script_uri_personal.clone(),
                    handler_name: options
                        .get("handler")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    payload: options
                        .get("payload")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({})),
                    run_at,
                    max_attempts: options
                        .get("maxAttempts")
                        .and_then(|v| v.as_i64())
                        .map(|n| n.clamp(i32::MIN as i64, i32::MAX as i64) as i32),
                    enqueued_by: Some(user_id.clone()),
                    run_as: Some(user_id.clone()),
                    // Per person by default, and this is the correctness fix
                    // rather than a convenience. Two prompts from one person
                    // otherwise become two runs interleaving turn for turn,
                    // each reading and overwriting the same
                    // `personalStorage` — which every script queueing
                    // per-person work had to work around with a table of its
                    // own. A caller that really wants them in parallel says
                    // `lane: null`, and one that wants a finer lane than the
                    // person names its own.
                    lane: match lane {
                        Some(Some(named)) => Some(named),
                        Some(None) => Some(Self::person_lane(&user_id)),
                        None => None,
                    },
                };

                match crate::tasks::blocking::enqueue(new_task) {
                    Ok(task) => Ok(host_ok(crate::tasks::to_json(&task))),
                    Err(e) => Ok(host_failure(
                        Self::task_error_name(&e),
                        &format!("personalTasks.enqueue: {}", e),
                    )),
                }
            },
        )?;
        host.set("enqueuePersonal", personal_enqueue)?;

        // `personalTasks.enqueueFrom` — work for a person the script did not
        // serve, named by the sender a message came from.
        //
        // This is the only way an execution with nobody signed in can act as
        // somebody, and it exists because an inbound webhook is exactly that:
        // a Telegram or Slack message arrives, there is no session, so there
        // is no person, so there was nothing to enqueue against and every
        // channel an agent might live on was out of reach.
        //
        // The shape is the security decision. A script cannot name a *person*
        // — no user id is accepted here — it names a sender as the channel
        // reports them, and the engine resolves that through the bindings
        // people have consented to (`delegation::resolve_channel`). So a
        // script that trusts the wrong field in a request body can be made to
        // claim the wrong sender, and not to name a different account: an
        // unbound sender resolves to nobody and nothing runs, and there is no
        // id to enumerate.
        //
        // What the engine cannot do is verify the message really came from
        // that sender. Telegram and Slack sign their webhooks and email
        // largely does not; checking the signature is the script's job, and
        // is said so in the documentation rather than pretended at here.
        let script_uri_from = script_uri.to_string();
        let config_from = self.config.clone();
        let user_from = self.user_context.clone();
        let enqueue_from = Function::new(
            ctx.clone(),
            move |options_json: String| -> JsResult<String> {
                if config_from.is_dry_run() {
                    return Ok(host_failure(
                        "DryRunError",
                        "personalTasks.enqueueFrom: nothing was enqueued - this is a dry run",
                    ));
                }

                if !user_from.has_capability(&Capability::EnqueueTasks) {
                    return Ok(host_failure(
                        "SecurityError",
                        &capability_refusal(
                            "personalTasks.enqueueFrom",
                            &Capability::EnqueueTasks,
                            &user_from,
                        ),
                    ));
                }

                let options: serde_json::Value = match serde_json::from_str(&options_json) {
                    Ok(options) => options,
                    Err(e) => {
                        return Ok(host_failure(
                            "TypeError",
                            &format!(
                                "personalTasks.enqueueFrom: options are not valid JSON: {}",
                                e
                            ),
                        ));
                    }
                };

                let (channel, identity) = match Self::sender_from_options(&options) {
                    Ok(pair) => pair,
                    Err(message) => {
                        return Ok(host_failure("TypeError", &message));
                    }
                };

                // Who that sender is here. Refused before anything else is
                // read, so an unlinked sender costs one indexed lookup.
                let user_id = match crate::database::run_blocking(
                    crate::delegation::resolve_channel(&script_uri_from, &channel, &identity),
                ) {
                    Ok(Some(user_id)) => user_id,
                    Ok(None) => {
                        return Ok(host_failure(
                            "SecurityError",
                            "personalTasks.enqueueFrom: nobody has linked that sender to this \
                             script - `personalTasks.inviteLink()` mints a link to send them",
                        ));
                    }
                    Err(e) => {
                        return Ok(host_failure(
                            "Error",
                            &format!("personalTasks.enqueueFrom: could not read the link: {}", e),
                        ));
                    }
                };

                // The same grant check the caller-facing enqueue makes, and
                // for the same reason: a link says which sender may trigger
                // the work, and the grant says whether there is any work to
                // trigger. Both, or neither.
                match crate::database::run_blocking(crate::delegation::get(
                    &user_id,
                    &script_uri_from,
                )) {
                    Ok(Some(grant)) if grant.is_live(chrono::Utc::now()) => {}
                    Ok(Some(_)) => {
                        return Ok(host_failure(
                            "SecurityError",
                            "personalTasks.enqueueFrom: this person's authorisation for this \
                             script has expired",
                        ));
                    }
                    Ok(None) => {
                        return Ok(host_failure(
                            "SecurityError",
                            "personalTasks.enqueueFrom: this person has not authorised this \
                             script to act for them",
                        ));
                    }
                    Err(e) => {
                        return Ok(host_failure(
                            "Error",
                            &format!(
                                "personalTasks.enqueueFrom: could not read the authorisation: {}",
                                e
                            ),
                        ));
                    }
                }

                // The one budget in the engine whose spender is chosen by
                // whoever sent the message. Spent after the link and the
                // grant are established, so an unlinked sender cannot drain
                // a stranger's budget by guessing at it.
                if let Err(message) =
                    Self::spend_channel_budget(&script_uri_from, &channel, &identity)
                {
                    return Ok(host_failure("RangeError", &message));
                }

                let lane = match Self::lane_from_options(&options) {
                    Ok(lane) => lane,
                    Err(message) => {
                        return Ok(host_failure(
                            "TypeError",
                            &format!("personalTasks.enqueueFrom: {}", message),
                        ));
                    }
                };

                let run_at = match options.get("runAt").and_then(|v| v.as_str()) {
                    Some(value) => match crate::scheduler::parse_utc_timestamp(value) {
                        Ok(parsed) => Some(parsed),
                        Err(_) => {
                            return Ok(host_failure(
                                "RangeError",
                                "personalTasks.enqueueFrom: runAt must be a UTC timestamp ending \
                                 with 'Z'",
                            ));
                        }
                    },
                    None => None,
                };

                let new_task = crate::tasks::NewTask {
                    script_uri: script_uri_from.clone(),
                    handler_name: options
                        .get("handler")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string(),
                    payload: options
                        .get("payload")
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({})),
                    run_at,
                    max_attempts: options
                        .get("maxAttempts")
                        .and_then(|v| v.as_i64())
                        .map(|n| n.clamp(i32::MIN as i64, i32::MAX as i64) as i32),
                    // Nobody signed in, so there is nobody to record as
                    // having asked. `run_as` is the whole of the authority
                    // and it was worked out above, never taken from input.
                    enqueued_by: None,
                    run_as: Some(user_id.clone()),
                    // Per person by default, as the caller-facing enqueue
                    // is, and here it is the case the lane key was written
                    // for: two messages arriving a second apart from the
                    // same chat are exactly the two runs that must not
                    // interleave.
                    lane: match lane {
                        Some(Some(named)) => Some(named),
                        Some(None) => Some(Self::person_lane(&user_id)),
                        None => None,
                    },
                };

                match crate::tasks::blocking::enqueue(new_task) {
                    Ok(task) => {
                        // Worth a line: this is work queued to act as
                        // somebody by a request they did not make, and the
                        // person asking later how their agent came to run
                        // needs to see which sender set it going.
                        info!(
                            script_uri = %script_uri_from,
                            user_id = %user_id,
                            channel = %channel,
                            task_id = %task.task_id,
                            "Delegated work queued by an inbound sender"
                        );
                        Ok(host_ok(crate::tasks::to_json(&task)))
                    }
                    Err(e) => Ok(host_failure(
                        Self::task_error_name(&e),
                        &format!("personalTasks.enqueueFrom: {}", e),
                    )),
                }
            },
        )?;
        host.set("enqueueFrom", enqueue_from)?;

        // Whether this sender is linked, and where to send them if not — so a
        // bot can answer an unknown sender with "authorise me here" instead of
        // failing at the enqueue.
        let script_uri_sender = script_uri.to_string();
        let sender_state = Function::new(
            ctx.clone(),
            move |options_json: String| -> JsResult<String> {
                let options: serde_json::Value = match serde_json::from_str(&options_json) {
                    Ok(options) => options,
                    Err(e) => {
                        return Ok(host_failure(
                            "TypeError",
                            &format!("personalTasks.sender: options are not valid JSON: {}", e),
                        ));
                    }
                };

                let (channel, identity) = match Self::sender_from_options(&options) {
                    Ok(pair) => pair,
                    Err(message) => return Ok(host_failure("TypeError", &message)),
                };

                let linked = match crate::database::run_blocking(
                    crate::delegation::resolve_channel(&script_uri_sender, &channel, &identity),
                ) {
                    Ok(linked) => linked,
                    Err(e) => {
                        return Ok(host_failure(
                            "Error",
                            &format!("personalTasks.sender: {}", e),
                        ));
                    }
                };

                // Deliberately not the user id. A script has no use for it —
                // everything it can do for that person goes through
                // `enqueueFrom`, which names the sender — and handing it over
                // would put an account identifier into whatever the bot logs
                // or echoes back to the chat.
                let Some(user_id) = linked else {
                    return Ok(host_ok(serde_json::json!({
                        "linked": false,
                        "granted": false,
                        "channel": channel,
                        "identity": identity,
                    })));
                };

                let grant = crate::database::run_blocking(crate::delegation::get(
                    &user_id,
                    &script_uri_sender,
                ));

                let (granted, expired, scopes) = match grant {
                    Ok(Some(grant)) => {
                        let live = grant.is_live(chrono::Utc::now());
                        (
                            live,
                            !live,
                            grant
                                .scopes
                                .iter()
                                .map(|scope| scope.as_str())
                                .collect::<Vec<_>>(),
                        )
                    }
                    _ => (false, false, Vec::new()),
                };

                Ok(host_ok(serde_json::json!({
                    "linked": true,
                    "granted": granted,
                    "expired": expired,
                    "scopes": scopes,
                    "channel": channel,
                    "identity": identity,
                })))
            },
        )?;
        host.set("sender", sender_state)?;

        // The invitation to link a sender, minted in reply to a message.
        //
        // Separate from `sender()` above, which is a read, because this
        // writes: it mints a single-use token and invalidates whatever was
        // outstanding for the same sender. Folding it into `sender()` would
        // mean a bot polling "is this person linked yet" quietly invalidated
        // the link it had already sent them.
        //
        // The URL carries a token rather than the sender, and that is the
        // security of the whole scheme. `?channel=telegram&identity=12345`
        // would be a URL anybody could construct for anybody, so linking
        // would be first come first served on a guessable string — and the
        // harm is interception rather than squatting: bind somebody else's
        // id before they do and every message they send that bot becomes
        // your turn, with their text in your storage. A token delivered into
        // the sender's own chat is the only evidence of ownership the engine
        // can have.
        let script_uri_invite = script_uri.to_string();
        let config_invite = self.config.clone();
        let user_invite = self.user_context.clone();
        let invite_link = Function::new(
            ctx.clone(),
            move |options_json: String| -> JsResult<String> {
                if config_invite.is_dry_run() {
                    return Ok(host_failure(
                        "DryRunError",
                        "personalTasks.inviteLink: nothing was minted - this is a dry run",
                    ));
                }

                // It writes a row, so it is on the write side of the surface.
                // Nothing an invitation does is dangerous on its own — it
                // binds nothing until somebody signs in and agrees — but "a
                // narrowed read-only turn writes nothing" is worth more as a
                // rule without exceptions than this is as a convenience.
                if !user_invite.has_capability(&Capability::EnqueueTasks) {
                    return Ok(host_failure(
                        "SecurityError",
                        &capability_refusal(
                            "personalTasks.inviteLink",
                            &Capability::EnqueueTasks,
                            &user_invite,
                        ),
                    ));
                }

                let options: serde_json::Value = match serde_json::from_str(&options_json) {
                    Ok(options) => options,
                    Err(e) => {
                        return Ok(host_failure(
                            "TypeError",
                            &format!(
                                "personalTasks.inviteLink: options are not valid JSON: {}",
                                e
                            ),
                        ));
                    }
                };

                let (channel, identity) = match Self::sender_from_options(&options) {
                    Ok(pair) => pair,
                    Err(message) => return Ok(host_failure("TypeError", &message)),
                };

                // Minting is inbound-triggered and writes, so it spends the
                // same budget a trigger does. Without it a stranger could
                // make the engine write a row per message.
                if let Err(message) =
                    Self::spend_channel_budget(&script_uri_invite, &channel, &identity)
                {
                    return Ok(host_failure("RangeError", &message));
                }

                match crate::database::run_blocking(crate::delegation::invite_link(
                    &script_uri_invite,
                    &channel,
                    &identity,
                )) {
                    Ok(url) => Ok(host_ok(serde_json::json!({
                        "linkUrl": url,
                        "channel": channel,
                        "identity": identity,
                        "expiresInMinutes": crate::delegation::LINK_TOKEN_MINUTES,
                    }))),
                    Err(refusal) => Ok(host_failure(
                        "Error",
                        &format!("personalTasks.inviteLink: {}", refusal),
                    )),
                }
            },
        )?;
        host.set("inviteLink", invite_link)?;

        // Whether this person has authorised this script, and for what — so a
        // script can offer the consent page instead of failing at the enqueue.
        let script_uri_grant = script_uri.to_string();
        let delegation_state = Function::new(
            ctx.clone(),
            move |ctx: rquickjs::Ctx<'_>| -> JsResult<String> {
                let Some(user_id) = Self::current_user_id(&ctx) else {
                    return Ok(host_ok(serde_json::json!({
                        "authenticated": false,
                        "granted": false,
                        "scopes": [],
                    })));
                };

                match crate::database::run_blocking(crate::delegation::get(
                    &user_id,
                    &script_uri_grant,
                )) {
                    Ok(Some(grant)) => {
                        let live = grant.is_live(chrono::Utc::now());
                        Ok(host_ok(serde_json::json!({
                            "authenticated": true,
                            "granted": live,
                            "expired": !live,
                            "expiresAt": grant.expires_at.to_rfc3339(),
                            "scopes": grant.scopes.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                            "consentUrl": crate::delegation::consent_url(&script_uri_grant),
                        })))
                    }
                    Ok(None) => Ok(host_ok(serde_json::json!({
                        "authenticated": true,
                        "granted": false,
                        "expired": false,
                        "scopes": [],
                        "consentUrl": crate::delegation::consent_url(&script_uri_grant),
                    }))),
                    Err(e) => Ok(host_failure(
                        "Error",
                        &format!("personalTasks.authorization: {}", e),
                    )),
                }
            },
        )?;
        host.set("authorization", delegation_state)?;

        global.set("__hostScriptTasks", host)?;

        crate::bytecode::eval_program(ctx, "engine://tasks-prelude", TASKS_PRELUDE).map_err(
            |e| {
                rquickjs::Error::new_from_js_message(
                    "tasks",
                    "prelude",
                    &format!("tasks prelude failed to load: {}", e),
                )
            },
        )?;

        debug!("scriptTasks initialized for script: {}", script_uri);
        Ok(())
    }

    /// The lane a person's own work runs in when nobody named one.
    ///
    /// Prefixed, so it reads as what it is wherever a lane is shown — and
    /// so a script choosing its own lane names is unlikely to collide with
    /// it by accident. A script that *wants* to share this lane can spell
    /// it out; that is a reasonable thing to want and not a hazard, since
    /// the only consequence of sharing a lane is running one at a time.
    fn person_lane(user_id: &str) -> String {
        format!("person:{}", user_id)
    }

    /// The lane a caller asked for, distinguishing "omitted" from "null".
    ///
    /// `Ok(None)` is an explicit `lane: null` — the caller saying this must
    /// not be serialised — and `Ok(Some(None))` is the field being absent,
    /// which lets each surface pick its own default. `personalTasks` reads
    /// the difference: absent means "serialise per person", which is almost
    /// always what per-person work wants, and `null` is how a script opts
    /// out of that on purpose.
    #[allow(clippy::type_complexity)]
    fn lane_from_options(options: &serde_json::Value) -> Result<Option<Option<String>>, String> {
        match options.get("lane") {
            None => Ok(Some(None)),
            Some(serde_json::Value::Null) => Ok(None),
            Some(serde_json::Value::String(lane)) => Ok(Some(Some(lane.clone()))),
            Some(_) => Err("lane must be a string, or null for no lane".to_string()),
        }
    }

    /// The `{ channel, identity }` pair a sender is named by, validated.
    ///
    /// Shared by `enqueueFrom` and `sender` so the two cannot come to
    /// disagree about what counts as a sender — a pair one accepted and the
    /// other refused would make "is this sender linked" answer about a
    /// different sender than the one the enqueue then looked up.
    fn sender_from_options(options: &serde_json::Value) -> Result<(String, String), String> {
        let channel = options
            .get("channel")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let identity = options
            .get("identity")
            .and_then(|value| value.as_str())
            .unwrap_or_default();

        crate::delegation::normalize_channel(channel, identity)
            .map_err(|refusal| format!("the sender is not usable: {}", refusal))
    }

    /// Spend one token of this binding's trigger budget.
    ///
    /// Blocking, because every caller is already on the JavaScript thread.
    fn spend_channel_budget(script_uri: &str, channel: &str, identity: &str) -> Result<(), String> {
        let Some(limiter) = crate::security::rate_limiting::shared() else {
            // Startup has not built one — a unit test. Carrying on is what
            // `git_sync::spend_budget` does in the same case and for the same
            // reason: refusing work for want of a budget to check would fail
            // closed on something that is not a security decision. The
            // decisions above it — the link, the grant — have already been
            // made by then.
            return Ok(());
        };

        let key = crate::security::rate_limiting::RateLimitKey::ChannelTrigger(format!(
            "{}:{}:{}",
            script_uri, channel, identity
        ));

        let allowed =
            crate::database::run_blocking(
                async move { limiter.check_rate_limit(key, 1).await.allowed },
            );

        if allowed {
            Ok(())
        } else {
            Err("personalTasks.enqueueFrom: this sender has queued too much too quickly - the                  budget refills over the next few minutes"
                .to_string())
        }
    }

    /// Which kind of exception a refusal becomes, so a script can tell a
    /// mistake in its own call from the engine being unable to answer.
    fn task_error_name(error: &crate::tasks::EnqueueError) -> &'static str {
        use crate::tasks::EnqueueError;
        match error {
            EnqueueError::MissingHandler
            | EnqueueError::InvalidHandler
            | EnqueueError::PayloadNotAnObject => "TypeError",
            EnqueueError::PayloadTooLarge
            | EnqueueError::InvalidMaxAttempts
            | EnqueueError::InvalidRunAt => "RangeError",
            EnqueueError::Unavailable | EnqueueError::Storage(_) => "Error",
        }
    }

    /// The Rust half of `sandbox` — [`crate::sandbox`].
    ///
    /// One host call that runs a whole second execution of this script, in a
    /// context holding a chosen subset of what this one holds. What makes
    /// that affordable is that nothing here is new: `evaluate_snippet` already
    /// evaluates caller-authored source against a script's program with a
    /// caller-chosen `UserContext`, and a stream customization function
    /// already builds a nested runtime from inside a running host call. This is those
    /// two facts put together and pointed at the calling script itself.
    fn setup_sandbox_functions(&self, ctx: &rquickjs::Ctx<'_>, script_uri: &str) -> JsResult<()> {
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
                    // Off by default here, on by default at `/engine/eval`.
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
    fn sandbox_auth_context(ctx: &rquickjs::Ctx<'_>) -> Option<crate::auth::JsAuthContext> {
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

    /// The envelope shape `sandbox_prelude.js` unwraps. Flatter than the
    /// task one because a sandbox result is already an object with its own
    /// `error` field, and nesting a second `error` beside it would be two
    /// different failures spelled the same way.
    fn sandbox_ok(result: serde_json::Value) -> String {
        serde_json::json!({ "ok": true, "result": result }).to_string()
    }

    fn sandbox_failure(name: &str, message: &str) -> String {
        serde_json::json!({ "ok": false, "name": name, "message": message }).to_string()
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
    fn setup_crypto_object(&self, ctx: &rquickjs::Ctx<'_>, script_uri: &str) -> JsResult<()> {
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

    /// `engine` — the engine's own management tools, reachable from a script.
    ///
    /// Engine administration was deliberately not exposed to JavaScript: all
    /// scripts are equal, so exposing it would have meant every script seeing
    /// it. What answers that is not a privileged-script list but the thing the
    /// capability model already does — **every call is authorized against the
    /// calling `UserContext`, by the same function `/mcp` calls
    /// ([`crate::engine_api::execute_native_mcp_tool`])**. A script holding
    /// this global holds nothing its caller does not, and there is one
    /// implementation of each tool rather than an in-process copy that could
    /// drift from the HTTP one.
    ///
    /// Installed only where [`GlobalSecurityConfig::engine_api`] says so,
    /// which is the request path and a delegated task. That flag exists
    /// because the capability check is not sufficient on its own: a scheduled
    /// job and `init()` run as synthetic administrators with nobody behind
    /// them, and a `sandbox.run` subset cannot distinguish "my own `console`"
    /// from "every script's logs". Its documentation has the argument.
    ///
    /// What is *not* here is a second authorization model. This adds no
    /// capability, no bypass and no special case; it adds a way to reach
    /// functions that were previously only reachable over HTTP, with the
    /// checks they already had.
    fn setup_engine_object(&self, ctx: &rquickjs::Ctx<'_>, script_uri: &str) -> JsResult<()> {
        if !self.config.engine_api {
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
mod route_spec_tests {
    use super::{RouteTarget, parse_route_spec};
    use serde_json::json;

    #[test]
    fn a_handler_spec_defaults_to_get_and_keeps_its_documentation() {
        let spec = parse_route_spec(&json!({
            "handler": "getThing",
            "summary": "One thing",
            "tags": ["Things"],
            "parameters": [{ "name": "id", "in": "path" }],
        }))
        .expect("a handler spec should parse");
        assert_eq!(
            spec.target,
            RouteTarget::Handler {
                name: "getThing".to_string(),
                method: "GET".to_string()
            }
        );
        assert_eq!(spec.summary.as_deref(), Some("One thing"));
        assert_eq!(spec.tags, vec!["Things".to_string()]);
        assert!(spec.parameters.is_some(), "parameters are an object now");
    }

    #[test]
    fn streams_and_files_carry_their_authorize_function() {
        let stream = parse_route_spec(&json!({ "stream": true, "authorize": "mayWatch" }))
            .expect("a stream spec should parse");
        assert_eq!(
            stream.target,
            RouteTarget::Stream {
                authorize: Some("mayWatch".to_string())
            }
        );
        let file =
            parse_route_spec(&json!({ "file": "public/a.css" })).expect("a file spec should parse");
        assert_eq!(
            file.target,
            RouteTarget::File {
                path: "public/a.css".to_string(),
                authorize: None
            }
        );
    }

    /// Every one of these is a mistake in the call, and every one is the kind
    /// that would otherwise do something other than what was written: a
    /// silently dropped `authorize` publishes the file it was meant to guard.
    #[test]
    fn a_spec_that_is_not_exactly_one_thing_is_refused() {
        for (spec, why) in [
            (json!({}), "no target"),
            (json!({ "handler": "h", "file": "public/a" }), "two targets"),
            (json!({ "stream": false }), "stream must be true"),
            (
                json!({ "file": "public/a", "authorise": "f" }),
                "a misspelt key",
            ),
            (
                json!({ "handler": "h", "authorize": "f" }),
                "authorize on a handler",
            ),
            (
                json!({ "stream": true, "method": "POST" }),
                "a method on a stream",
            ),
            (
                json!({ "file": "public/a", "parameters": [] }),
                "parameters on a file",
            ),
            (
                json!({ "handler": "not a name" }),
                "a handler that is not a name",
            ),
            (
                json!({ "handler": "h", "tags": "x" }),
                "tags that are not an array",
            ),
            (json!("h"), "not an object"),
        ] {
            assert!(parse_route_spec(&spec).is_err(), "should refuse {}", why);
        }
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
                engine_api: false,
                dry_run_sink: None,
                console_sink: None,
                log_context: repository::LogContext::default(),
                // Not acting for anybody: nothing to narrow.
                delegated_scopes: None,
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
            "files",
            "scriptStorage",
            "personalStorage",
            "secretStorage",
            "schedulerService",
            "scriptTasks",
            "personalTasks",
            "mcpRegistry",
            "database",
            "console",
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
        // Every registration answers with a value rather than a sentence:
        // outside the registration phase the answer is `{ ok: false, reason }`.
        for (call, subject) in [
            ("routeRegistry.registerRoute('/r', { handler: 'h' })", "/r"),
            ("routeRegistry.registerRoute('/s', { stream: true })", "/s"),
            ("routeRegistry.registerRoute('/a', { file: 'a.txt' })", "/a"),
            (
                "mcpRegistry.registerTool('t', { description: 'd', inputSchema: {}, handler: 'h' })",
                "t",
            ),
            (
                "mcpRegistry.registerPrompt('p', { description: 'd', arguments: [], handler: 'h' })",
                "p",
            ),
            (
                "schedulerService.registerOnce({ handler: 'h', runAt: '2030-01-01T00:00:00Z' })",
                "h",
            ),
            (
                "schedulerService.registerRecurring({ handler: 'h', intervalMinutes: 5 })",
                "h",
            ),
        ] {
            let result = eval_outside_registration_phase(&format!(
                "(function (r) {{ return r.ok + '|' + r.reason; }})({})",
                call
            ));
            assert!(
                result.starts_with("false|")
                    && result.contains("not registered")
                    && result.contains(subject),
                "`{}` should report that it did not register, got: {}",
                call,
                result
            );
        }
    }

    /// The arguments the type declarations type `any` reach the host as JSON.
    ///
    /// Every one of these used to raise `TypeError: Error converting from js
    /// 'object' into type 'string'`: the bindings took a `String`, QuickJS
    /// does not coerce one, and the declarations — and every example in them —
    /// passed an object. The tests are here rather than around the host
    /// functions because what broke was the JavaScript surface, and that is
    /// the only place it shows.
    #[test]
    fn fetch_serializes_the_options_object_it_documents() {
        // `__hostFetch` is replaced so this tests the marshalling and not the
        // network: the prelude looks the name up on each call.
        let seen = eval_outside_registration_phase(
            r#"(function () {
                 var seen = null;
                 globalThis.__hostFetch = function (url, options) {
                   seen = options;
                   return JSON.stringify({ status: 200, ok: true, headers: {}, body: "" });
                 };
                 var response = fetch("https://example.com/x", {
                   method: "POST",
                   headers: { "Content-Type": "application/json" },
                   body: "{}",
                 });
                 return typeof seen + "|" + seen + "|" + response.status;
               })()"#,
        );
        assert!(
            seen.starts_with("string|"),
            "options should reach the host as JSON text, got: {}",
            seen
        );
        assert!(
            seen.contains(r#""method":"POST""#) && seen.ends_with("|200"),
            "the options the script wrote should be the options the host sees, got: {}",
            seen
        );
    }

    /// A script written against the host call sends JSON text already, and it
    /// must not be re-encoded into a quoted string.
    #[test]
    fn fetch_passes_a_string_of_options_through_untouched() {
        let seen = eval_outside_registration_phase(
            r#"(function () {
                 var seen = null;
                 globalThis.__hostFetch = function (url, options) {
                   seen = options;
                   return JSON.stringify({ status: 200, ok: true, headers: {}, body: "" });
                 };
                 fetch("https://example.com/x", JSON.stringify({ method: "PUT" }));
                 return seen;
               })()"#,
        );
        assert_eq!(seen, r#"{"method":"PUT"}"#);
    }

    /// Omitting the options must stay omitted rather than becoming `"null"`,
    /// which the host would fail to parse as `FetchOptions`.
    #[test]
    fn fetch_without_options_hands_the_host_nothing() {
        let seen = eval_outside_registration_phase(
            r#"(function () {
                 var seen = "not called";
                 globalThis.__hostFetch = function (url, options) {
                   seen = typeof options;
                   return JSON.stringify({ status: 200, ok: true, headers: {}, body: "" });
                 };
                 fetch("https://example.com/x");
                 return seen;
               })()"#,
        );
        assert_eq!(seen, "undefined");
    }

    /// The stream and dispatch calls take the object their examples pass. The
    /// assertion is that they answer at all: a conversion failure throws out
    /// of the binding before any of these can report anything.
    // The stream binding files an audit event on a spawned task, so this one
    // needs a runtime where the others do not.
    #[tokio::test]
    async fn the_data_arguments_take_the_object_their_examples_pass() {
        for call in [
            "routeRegistry.sendStreamMessage('/events/x', { type: 'alert', n: 1 })",
            "routeRegistry.sendStreamMessageFiltered('/events/x', { type: 'alert' }, \
             { role: 'admin' })",
        ] {
            // The call is wrapped in JavaScript so a refusal comes back as
            // text: what is being asserted is which refusal it is, and an
            // exception out of `eval` would take that with it.
            let result = eval_outside_registration_phase(&format!(
                "(function () {{ try {{ return JSON.stringify({}); }} \
                 catch (e) {{ return 'threw: ' + e; }} }})()",
                call
            ));
            assert!(
                !result.starts_with("threw: "),
                "`{}` should serialize its data rather than refusing it, got: {}",
                call,
                result
            );
        }
    }

    /// Argument validation is context-independent: a malformed call is reported
    /// the same way wherever it is made, rather than being masked by the phase.
    #[test]
    fn argument_validation_runs_before_the_phase_check() {
        let thrown = eval_outside_registration_phase(
            "(function () { try { routeRegistry.registerRoute('no-slash', { stream: true }); \
             return 'did not throw'; } catch (e) { return e.name + ': ' + e.message; } })()",
        );
        assert!(
            thrown.starts_with("TypeError: ") && thrown.contains("must start with"),
            "a malformed path is a mistake in the call, got: {}",
            thrown
        );
    }
}

#[cfg(test)]
mod json_arg_tests {
    use super::json_arg;
    use rquickjs::{Context, Runtime};

    /// Evaluate `expr` and marshal the result the way a host binding does.
    fn marshal(expr: &str) -> Result<String, String> {
        let rt = Runtime::new().expect("runtime");
        let ctx = Context::full(&rt).expect("context");
        ctx.with(|ctx| {
            let value = ctx.eval::<rquickjs::Value<'_>, _>(expr).expect("eval");
            json_arg(value, "data").map_err(|e| e.to_string())
        })
    }

    #[test]
    fn an_object_is_serialized_the_way_the_declarations_promise() {
        assert_eq!(
            marshal("({ type: 'alert', n: 1 })"),
            Ok(r#"{"type":"alert","n":1}"#.to_string())
        );
    }

    /// The scripts that exist send `JSON.stringify(...)`, and re-encoding that
    /// would deliver a quoted string to every listener reading it.
    #[test]
    fn a_string_is_the_message_rather_than_a_value_to_encode() {
        assert_eq!(
            marshal(r#"JSON.stringify({ a: 1 })"#),
            Ok(r#"{"a":1}"#.to_string())
        );
        assert_eq!(marshal("'plain text'"), Ok("plain text".to_string()));
    }

    #[test]
    fn arrays_and_scalars_serialize_as_themselves() {
        assert_eq!(marshal("[1, 2]"), Ok("[1,2]".to_string()));
        assert_eq!(marshal("42"), Ok("42".to_string()));
        assert_eq!(marshal("true"), Ok("true".to_string()));
    }

    #[test]
    fn nothing_at_all_is_the_empty_message() {
        assert_eq!(marshal("undefined"), Ok(String::new()));
        assert_eq!(marshal("null"), Ok(String::new()));
    }

    /// `JSON.stringify` has no answer for a function, and the caller is told
    /// which argument it could not serialize rather than being handed the
    /// conversion error QuickJS would have raised.
    #[test]
    fn a_value_json_cannot_describe_names_the_argument() {
        let error = marshal("(function () {})").expect_err("a function has no JSON");
        assert!(
            error.contains("data"),
            "the refusal should name the argument, got: {}",
            error
        );
    }
}

#[cfg(test)]
mod query_option_tests {
    use super::build_query_options;
    use crate::repository::OrderDirection;

    #[test]
    fn omitting_everything_gives_the_defaults() {
        let options = build_query_options(None, None, None, None).expect("defaults are valid");

        assert_eq!(options.limit, None);
        assert_eq!(options.order_by, None);
        assert_eq!(options.order_dir, OrderDirection::Ascending);
        assert!(!options.for_update);
    }

    #[test]
    fn the_arguments_land_where_the_query_will_look_for_them() {
        let options = build_query_options(
            Some(25),
            Some("ts".to_string()),
            Some("desc".to_string()),
            Some(r#"{"forUpdate": true}"#),
        )
        .expect("a fully specified query is valid");

        assert_eq!(options.limit, Some(25));
        assert_eq!(options.order_by.as_deref(), Some("ts"));
        assert_eq!(options.order_dir, OrderDirection::Descending);
        assert!(options.for_update);
    }

    #[test]
    fn a_sort_direction_that_is_neither_is_refused() {
        // It used to sort ascending. A script asking for "descending" got the
        // opposite of what it asked for, in silence.
        let error = build_query_options(None, None, Some("descending".to_string()), None)
            .expect_err("'descending' is not a direction");

        assert!(
            error.contains("descending") && error.contains("desc"),
            "the refusal should show what was passed and what is accepted: {error}"
        );

        for accepted in ["asc", "ASC", "desc", "DESC", " Desc "] {
            assert!(
                build_query_options(None, None, Some(accepted.to_string()), None).is_ok(),
                "{accepted} should be accepted"
            );
        }
    }

    #[test]
    fn an_unrecognised_option_is_refused_rather_than_dropped() {
        // Dropping it would hand back an unguarded query to a caller who asked
        // for a guarded one — this option's whole failure mode, in silence.
        let error = build_query_options(None, None, None, Some(r#"{"forupdate": true}"#))
            .expect_err("a misspelled key is not an option");

        assert!(
            error.contains("forupdate") && error.contains("forUpdate"),
            "the refusal should name both what was passed and what is supported: {error}"
        );
    }

    #[test]
    fn an_option_of_the_wrong_type_is_refused() {
        let error = build_query_options(None, None, None, Some(r#"{"forUpdate": "yes"}"#))
            .expect_err("a string is not a boolean");
        assert!(error.contains("true or false"), "{error}");

        let error = build_query_options(None, None, None, Some("[]"))
            .expect_err("an array is not an options object");
        assert!(error.contains("JSON object"), "{error}");

        let error = build_query_options(None, None, None, Some("{not json"))
            .expect_err("this is not JSON at all");
        assert!(error.contains("Invalid options JSON"), "{error}");
    }

    #[test]
    fn an_empty_options_string_is_the_same_as_none() {
        // A script building the argument conditionally can end up passing "".
        let options =
            build_query_options(None, None, None, Some("   ")).expect("blank is not an error");
        assert!(!options.for_update);
    }
}
