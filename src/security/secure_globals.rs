use base64::Engine;
use chrono::Duration as ChronoDuration;
use rquickjs::{Function, Result as JsResult, function::Opt};
use std::collections::HashMap;
use tracing::{debug, error, info, warn};

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
const TASKS_PRELUDE: &str = include_str!("../../assets/tasks_prelude.js");

/// Builds `sandbox` over the host call that runs a narrowed sub-execution.
const SANDBOX_PRELUDE: &str = include_str!("../../assets/sandbox_prelude.js");

/// What `secretStorage`'s mutating methods answer in a delegated execution.
///
/// Phrased as the other "Error: ..." strings on that object are, since the
/// interface returns refusals as values rather than throwing, and a script
/// that already handles "not authenticated" handles this the same way.
const DELEGATED_SECRET_MANAGEMENT_REFUSAL: &str = "Error: Secrets cannot be changed by background work acting on somebody's behalf. \
     Storing, replacing or deleting a key is something the person does in their own session.";

/// `Headers`, `URLSearchParams`, and the methods `context.request` gains so a
/// body a script receives reads the way a body it fetched does.
const REQUEST_PRELUDE: &str = include_str!("../../assets/request_prelude.js");

use crate::repository;
use crate::scheduler;
use crate::security::{
    Capability, SecureOperations, SecurityAuditor, SecurityEventType, SecuritySeverity, UserContext,
};

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
/// The host bindings behind `sendStreamMessage`, `sendStreamMessageFiltered`
/// and `dispatcher.sendMessage` took a `String`, while the type declarations
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
/// Shaped like the `columns` a script would otherwise pass to `addTextColumn`
/// and friends one at a time: `{ "columns": [{ "name", "type", "nullable"?,
/// "default"? }], "uniqueIndexes"?: [["col"]] }`. `nullable` defaults to true,
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
        let column_type = crate::db_schema_utils::ColumnType::from_str(type_name)
            .map_err(|e| format!("column \"{}\": {}", name, e))?;

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

/// A GraphQL `{"errors": [{"message": "..."}]}` answer.
///
/// The shape a GraphQL client expects, with the same escaping guarantee
/// [`error_answer`] gives.
fn graphql_errors_answer(message: impl std::fmt::Display) -> String {
    serde_json::json!({ "errors": [{ "message": message.to_string() }] }).to_string()
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

/// Longest query `graphQLRegistry.executeGraphQL` accepts, in characters, and
/// longest JSON `variables` string beside it. Named so the limits the engine
/// publishes are the ones this call enforces.
pub const MAX_GRAPHQL_QUERY_CHARS: usize = 100_000;
/// See [`MAX_GRAPHQL_QUERY_CHARS`].
pub const MAX_GRAPHQL_VARIABLES_CHARS: usize = 50_000;

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
        // After the dispatcher: the queue's prelude installs `scriptTasks` and
        // also puts `post` on the dispatcher, which has to be there already.
        self.setup_task_functions(ctx, script_uri)?;
        self.setup_sandbox_functions(ctx, script_uri)?;

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
                if uri.is_empty() || uri.len() > repository::MAX_ASSET_URI_CHARS {
                    return Ok(format!(
                        "Invalid asset URI: must be 1-{} characters",
                        repository::MAX_ASSET_URI_CHARS
                    ));
                }
                if uri.contains("..") || uri.contains('\\') {
                    return Ok("Invalid asset URI: path traversal not allowed".to_string());
                }

                // The storage-side limit, so this path refuses exactly what
                // the repository would refuse rather than a little more.
                if content.len() > repository::MAX_ASSET_CONTENT_BYTES {
                    return Ok(format!(
                        "Asset too large (max {} bytes)",
                        repository::MAX_ASSET_CONTENT_BYTES
                    ));
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
            format!(
                "Error: {}",
                capability_refusal(
                    "secretStorage",
                    &Capability::WriteSecrets,
                    &self.user_context
                )
            )
        };
        let manage_refusal_set = manage_refusal.clone();

        let secret_storage_obj = rquickjs::Object::new(ctx.clone())?;

        // secretStorage.exists(key) - Check if secret exists in user_secrets or script_secrets
        let script_uri_exists = script_uri_owned.clone();
        let exists_fn = Function::new(
            ctx.clone(),
            move |ctx: rquickjs::Ctx<'_>, key: String| -> JsResult<bool> {
                let globals = ctx.globals();
                // Check user_secrets first (if authenticated, and if this
                // execution was authorised to reach that person's secrets).
                if secrets_allowed
                    && let Some(user_id) = get_auth_user_id(&globals)
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
                if !may_manage_secrets {
                    return Ok(manage_refusal_set.clone());
                }
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
                // `false` is what this already answers with no person signed
                // in, and it means the same thing here: nothing was removed.
                if !may_manage_secrets {
                    return Ok(false);
                }
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
                if !may_manage_secrets {
                    return Ok(manage_refusal.clone());
                }
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
                    return Ok(graphql_errors_answer(e));
                }

                // Validate query
                if query.is_empty() || query.len() > MAX_GRAPHQL_QUERY_CHARS {
                    return Ok("{\"errors\": [{\"message\": \"Invalid query: must be between 1 and 100,000 characters\"}]}".to_string());
                }

                // Parse variables if provided
                let variables = if let Some(vars_json) = variables_json {
                    if vars_json.len() > MAX_GRAPHQL_VARIABLES_CHARS {
                        return Ok("{\"errors\": [{\"message\": \"Variables too large: max 50,000 characters\"}]}".to_string());
                    }
                    match serde_json::from_str::<serde_json::Value>(&vars_json) {
                        Ok(v) => Some(v),
                        Err(e) => {
                            return Ok(graphql_errors_answer(format!(
                                "Invalid variables JSON: {}",
                                e
                            )));
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
                        Ok(graphql_errors_answer(format!(
                            "GraphQL execution failed: {}",
                            e
                        )))
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
            move |_ctx: rquickjs::Ctx<'_>,
                  path: String,
                  message: rquickjs::Value<'_>|
                  -> JsResult<String> {
                // Typed `any` in the declarations and serialized here, so the
                // object every example passes is the object that arrives.
                let message = json_arg(message, "data")?;
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
                  message: rquickjs::Value<'_>,
                  filter_json: Option<String>,
                  match_mode: Option<String>|
                  -> JsResult<String> {
                // As in `sendStreamMessage`: the data is whatever the script
                // has, the filter is the JSON string the declarations ask for.
                let message = json_arg(message, "data")?;
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
                        options: request.options,
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
                    Ok(physical_name) => Ok(success_answer(serde_json::json!({
                        "tableName": table_name,
                        "physicalName": physical_name,
                    }))),
                    Err(e) => Ok(error_answer(e)),
                }
            },
        )?;
        database_obj.set("createTable", create_table)?;

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
                    Ok(()) => Ok(success_answer(serde_json::json!({
                        "column": column_name,
                    }))),
                    Err(e) => Ok(error_answer(e)),
                }
            },
        )?;
        database_obj.set("addIntegerColumn", add_integer_column)?;

        // database.addBigintColumn(tableName, columnName, nullable, defaultValue)
        let script_uri_add_bigint = script_uri_owned.clone();
        let user_ctx_add_bigint = user_context.clone();
        let add_bigint_column = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  table_name: String,
                  column_name: String,
                  nullable: Opt<bool>,
                  default_value: Opt<String>|
                  -> JsResult<String> {
                debug!(
                    "database.addBigintColumn called for script {}",
                    script_uri_add_bigint
                );

                if user_ctx_add_bigint
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
                    &script_uri_add_bigint,
                    &table_name,
                    &column_name,
                    crate::db_schema_utils::ColumnType::Bigint,
                    nullable,
                    default_val,
                ) {
                    Ok(()) => Ok(success_answer(serde_json::json!({
                        "column": column_name,
                    }))),
                    Err(e) => Ok(error_answer(e)),
                }
            },
        )?;
        database_obj.set("addBigintColumn", add_bigint_column)?;

        // database.addFloatColumn(tableName, columnName, nullable, defaultValue)
        let script_uri_add_float = script_uri_owned.clone();
        let user_ctx_add_float = user_context.clone();
        let add_float_column = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  table_name: String,
                  column_name: String,
                  nullable: Opt<bool>,
                  default_value: Opt<String>|
                  -> JsResult<String> {
                debug!(
                    "database.addFloatColumn called for script {}",
                    script_uri_add_float
                );

                if user_ctx_add_float
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
                    &script_uri_add_float,
                    &table_name,
                    &column_name,
                    crate::db_schema_utils::ColumnType::Float,
                    nullable,
                    default_val,
                ) {
                    Ok(()) => Ok(success_answer(serde_json::json!({
                        "column": column_name,
                    }))),
                    Err(e) => Ok(error_answer(e)),
                }
            },
        )?;
        database_obj.set("addFloatColumn", add_float_column)?;

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
                    Ok(()) => Ok(success_answer(serde_json::json!({
                        "column": column_name,
                    }))),
                    Err(e) => Ok(error_answer(e)),
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
                    Ok(()) => Ok(success_answer(serde_json::json!({
                        "column": column_name,
                    }))),
                    Err(e) => Ok(error_answer(e)),
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
                    Ok(()) => Ok(success_answer(serde_json::json!({
                        "column": column_name,
                    }))),
                    Err(e) => Ok(error_answer(e)),
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
                    Ok(()) => Ok(success_answer(serde_json::json!({
                        "foreignKey": format!(
                            "{}.{} -> {}",
                            table_name, column_name, referenced_table_name
                        ),
                        "nullable": nullable,
                    }))),
                    Err(e) => Ok(error_answer(e)),
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

        // database.query(tableName, filters, limit, orderBy, orderDir, options)
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

                if !user_ctx_lease.has_capability(&Capability::WriteScriptData) {
                    return Ok(error_answer(capability_refusal(
                        "database",
                        &Capability::WriteScriptData,
                        &user_ctx_lease,
                    )));
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
                        Err(e) => Ok(error_answer(format!("Serialization error: {}", e))),
                    },
                    Err(e) => Ok(error_answer(e)),
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
                    Ok(physical_name) => Ok(success_answer(serde_json::json!({
                        "tableName": table_name,
                        "physicalName": physical_name,
                    }))),
                    Err(e) => Ok(error_answer(e)),
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
                    // The parsed columns rather than the caller's raw
                    // `columns_json`: echoing an argument back verbatim puts
                    // whatever it contained into the answer.
                    Ok(()) => Ok(success_answer(serde_json::json!({
                        "tableName": table_name,
                        "columns": columns,
                    }))),
                    Err(e) => Ok(error_answer(e)),
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

                if !user_ctx_graphql.has_capability(&Capability::ManageScriptDatabase) {
                    return Ok(error_answer(capability_refusal(
                        "database",
                        &Capability::ManageScriptDatabase,
                        &user_ctx_graphql,
                    )));
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
                            return Ok(error_answer(format!("Failed to get table schema: {}", e)));
                        }
                    };

                // Get foreign keys
                let foreign_keys =
                    match crate::repository::get_foreign_keys(&script_uri_graphql, &table_name) {
                        Ok(fks) => fks,
                        Err(e) => {
                            return Ok(error_answer(format!("Failed to get foreign keys: {}", e)));
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
                        return Ok(error_answer(format!(
                            "Failed to inject resolver {}: {:?}",
                            query.resolver_function_name, e
                        )));
                    }
                }

                for mutation in &operations.mutations {
                    if let Err(e) = ctx_inner.eval::<(), _>(mutation.resolver_code.as_str()) {
                        return Ok(error_answer(format!(
                            "Failed to inject resolver {}: {:?}",
                            mutation.resolver_function_name, e
                        )));
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
                    return Ok(
                        serde_json::json!({ "dryRun": true, "table": table_name }).to_string()
                    );
                }

                for query in &operations.queries {
                    if let Err(e) = crate::graphql::register_graphql_query(
                        query.name.clone(),
                        query.sdl.clone(),
                        query.resolver_function_name.clone(),
                        script_uri_graphql.clone(),
                        visibility.clone(),
                    ) {
                        return Ok(error_answer(format!(
                            "Failed to register query {}: {}",
                            query.name, e
                        )));
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
                        return Ok(error_answer(format!(
                            "Failed to register mutation {}: {}",
                            mutation.name, e
                        )));
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

                // `{:?}` on a `Vec<String>` happens to look like a JSON array
                // and escapes by Rust's rules, not JSON's.
                Ok(success_answer(serde_json::json!({
                    "table": table_name,
                    "queries": query_names,
                    "mutations": mutation_names,
                })))
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
                    Err(e) => Ok(error_answer(e)),
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
                    Err(e) => Ok(error_answer(e)),
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
                    Err(e) => Ok(error_answer(e)),
                }
            },
        )?;
        database_obj.set("rollbackTransaction", rollback_transaction)?;

        // database.createSavepoint(name?) - Create a named or auto-generated savepoint
        let create_savepoint = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, name: Opt<String>| -> JsResult<String> {
                match crate::database::Database::create_savepoint(name.0.as_deref()) {
                    Ok(savepoint_name) => Ok(success_answer(serde_json::json!({
                        "savepoint": savepoint_name,
                    }))),
                    Err(e) => Ok(error_answer(e)),
                }
            },
        )?;
        database_obj.set("createSavepoint", create_savepoint)?;

        // database.rollbackToSavepoint(name) - Rollback to a named savepoint
        let rollback_to_savepoint = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, name: String| -> JsResult<String> {
                match crate::database::Database::rollback_to_savepoint(&name) {
                    Ok(()) => Ok(success_answer(serde_json::json!({
                        "message": format!("Rolled back to savepoint: {}", name),
                    }))),
                    Err(e) => Ok(error_answer(e)),
                }
            },
        )?;
        database_obj.set("rollbackToSavepoint", rollback_to_savepoint)?;

        // database.releaseSavepoint(name) - Release a named savepoint
        let release_savepoint = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, name: String| -> JsResult<String> {
                match crate::database::Database::release_savepoint(&name) {
                    Ok(()) => Ok(success_answer(serde_json::json!({
                        "message": format!("Released savepoint: {}", name),
                    }))),
                    Err(e) => Ok(error_answer(e)),
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
                    return Ok(Self::task_failure(
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
                    return Ok(Self::task_failure(
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
                        return Ok(Self::task_failure(
                            "TypeError",
                            &format!("scriptTasks.enqueue: options are not valid JSON: {}", e),
                        ));
                    }
                };

                let lane = match Self::lane_from_options(&options) {
                    Ok(lane) => lane,
                    Err(message) => {
                        return Ok(Self::task_failure(
                            "TypeError",
                            &format!("scriptTasks.enqueue: {}", message),
                        ));
                    }
                };

                let run_at = match options.get("runAt").and_then(|v| v.as_str()) {
                    Some(value) => match crate::scheduler::parse_utc_timestamp(value) {
                        Ok(parsed) => Some(parsed),
                        Err(_) => {
                            return Ok(Self::task_failure(
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
                            return Ok(Self::task_failure(
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
                    kind: crate::tasks::TaskKind::Task,
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
                    Ok(task) => Ok(Self::task_ok(crate::tasks::to_json(&task))),
                    Err(e) => Ok(Self::task_failure(
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
                return Ok(Self::task_failure(
                    "DryRunError",
                    "scriptTasks.cancel: nothing was cancelled - this is a dry run",
                ));
            }

            // Cancelling is changing the queue, so it takes the same
            // capability enqueueing does. A narrowed turn that could delete
            // work the script had accepted would be a write by another name.
            if !user_cancel.has_capability(&Capability::EnqueueTasks) {
                return Ok(Self::task_failure(
                    "SecurityError",
                    &capability_refusal(
                        "scriptTasks.cancel",
                        &Capability::EnqueueTasks,
                        &user_cancel,
                    ),
                ));
            }

            let Ok(parsed) = uuid::Uuid::parse_str(task_id.trim()) else {
                return Ok(Self::task_failure(
                    "TypeError",
                    "scriptTasks.cancel: that is not a task id",
                ));
            };

            match crate::tasks::blocking::cancel(parsed) {
                Ok(cancelled) => Ok(Self::task_ok(serde_json::Value::Bool(cancelled))),
                Err(e) => Ok(Self::task_failure(
                    "Error",
                    &format!("scriptTasks.cancel: {}", e),
                )),
            }
        })?;
        host.set("cancel", cancel)?;

        // Scoped to the calling script, so one script cannot read another's
        // queue by holding an id. Everything else here is already per script
        // because the URI comes from the binding rather than the caller.
        let script_uri_get = script_uri.to_string();
        let get = Function::new(ctx.clone(), move |task_id: String| -> JsResult<String> {
            let Ok(parsed) = uuid::Uuid::parse_str(task_id.trim()) else {
                return Ok(Self::task_failure(
                    "TypeError",
                    "scriptTasks.get: that is not a task id",
                ));
            };

            match crate::tasks::blocking::get(parsed) {
                Ok(Some(task)) if task.script_uri == script_uri_get => {
                    Ok(Self::task_ok(crate::tasks::to_json(&task)))
                }
                Ok(_) => Ok(Self::task_ok(serde_json::Value::Null)),
                Err(e) => Ok(Self::task_failure(
                    "Error",
                    &format!("scriptTasks.get: {}", e),
                )),
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
                    return Ok(Self::task_failure(
                        "DryRunError",
                        "personalTasks.enqueue: nothing was enqueued - this is a dry run",
                    ));
                }

                // As `scriptTasks.enqueue`: work queued now runs later under
                // what the grant allows, which is not what this turn was
                // narrowed to.
                if !user_personal_enqueue.has_capability(&Capability::EnqueueTasks) {
                    return Ok(Self::task_failure(
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
                    return Ok(Self::task_failure(
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
                        return Ok(Self::task_failure(
                            "SecurityError",
                            "personalTasks.enqueue: this person's authorisation for this script has expired",
                        ));
                    }
                    Ok(None) => {
                        return Ok(Self::task_failure(
                            "SecurityError",
                            "personalTasks.enqueue: this person has not authorised this script to act for them",
                        ));
                    }
                    Err(e) => {
                        return Ok(Self::task_failure(
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
                        return Ok(Self::task_failure(
                            "TypeError",
                            &format!("personalTasks.enqueue: options are not valid JSON: {}", e),
                        ));
                    }
                };

                let lane = match Self::lane_from_options(&options) {
                    Ok(lane) => lane,
                    Err(message) => {
                        return Ok(Self::task_failure(
                            "TypeError",
                            &format!("personalTasks.enqueue: {}", message),
                        ));
                    }
                };

                let run_at = match options.get("runAt").and_then(|v| v.as_str()) {
                    Some(value) => match crate::scheduler::parse_utc_timestamp(value) {
                        Ok(parsed) => Some(parsed),
                        Err(_) => {
                            return Ok(Self::task_failure(
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
                    kind: crate::tasks::TaskKind::Task,
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
                    Ok(task) => Ok(Self::task_ok(crate::tasks::to_json(&task))),
                    Err(e) => Ok(Self::task_failure(
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
                    return Ok(Self::task_failure(
                        "DryRunError",
                        "personalTasks.enqueueFrom: nothing was enqueued - this is a dry run",
                    ));
                }

                if !user_from.has_capability(&Capability::EnqueueTasks) {
                    return Ok(Self::task_failure(
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
                        return Ok(Self::task_failure(
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
                        return Ok(Self::task_failure("TypeError", &message));
                    }
                };

                // Who that sender is here. Refused before anything else is
                // read, so an unlinked sender costs one indexed lookup.
                let user_id = match crate::database::run_blocking(
                    crate::delegation::resolve_channel(&script_uri_from, &channel, &identity),
                ) {
                    Ok(Some(user_id)) => user_id,
                    Ok(None) => {
                        return Ok(Self::task_failure(
                            "SecurityError",
                            "personalTasks.enqueueFrom: nobody has linked that sender to this \
                             script - `personalTasks.inviteLink()` mints a link to send them",
                        ));
                    }
                    Err(e) => {
                        return Ok(Self::task_failure(
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
                        return Ok(Self::task_failure(
                            "SecurityError",
                            "personalTasks.enqueueFrom: this person's authorisation for this \
                             script has expired",
                        ));
                    }
                    Ok(None) => {
                        return Ok(Self::task_failure(
                            "SecurityError",
                            "personalTasks.enqueueFrom: this person has not authorised this \
                             script to act for them",
                        ));
                    }
                    Err(e) => {
                        return Ok(Self::task_failure(
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
                    return Ok(Self::task_failure("RangeError", &message));
                }

                let lane = match Self::lane_from_options(&options) {
                    Ok(lane) => lane,
                    Err(message) => {
                        return Ok(Self::task_failure(
                            "TypeError",
                            &format!("personalTasks.enqueueFrom: {}", message),
                        ));
                    }
                };

                let run_at = match options.get("runAt").and_then(|v| v.as_str()) {
                    Some(value) => match crate::scheduler::parse_utc_timestamp(value) {
                        Ok(parsed) => Some(parsed),
                        Err(_) => {
                            return Ok(Self::task_failure(
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
                    kind: crate::tasks::TaskKind::Task,
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
                        Ok(Self::task_ok(crate::tasks::to_json(&task)))
                    }
                    Err(e) => Ok(Self::task_failure(
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
                        return Ok(Self::task_failure(
                            "TypeError",
                            &format!("personalTasks.sender: options are not valid JSON: {}", e),
                        ));
                    }
                };

                let (channel, identity) = match Self::sender_from_options(&options) {
                    Ok(pair) => pair,
                    Err(message) => return Ok(Self::task_failure("TypeError", &message)),
                };

                let linked = match crate::database::run_blocking(
                    crate::delegation::resolve_channel(&script_uri_sender, &channel, &identity),
                ) {
                    Ok(linked) => linked,
                    Err(e) => {
                        return Ok(Self::task_failure(
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
                    return Ok(Self::task_ok(serde_json::json!({
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

                Ok(Self::task_ok(serde_json::json!({
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
                    return Ok(Self::task_failure(
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
                    return Ok(Self::task_failure(
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
                        return Ok(Self::task_failure(
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
                    Err(message) => return Ok(Self::task_failure("TypeError", &message)),
                };

                // Minting is inbound-triggered and writes, so it spends the
                // same budget a trigger does. Without it a stranger could
                // make the engine write a row per message.
                if let Err(message) =
                    Self::spend_channel_budget(&script_uri_invite, &channel, &identity)
                {
                    return Ok(Self::task_failure("RangeError", &message));
                }

                match crate::database::run_blocking(crate::delegation::invite_link(
                    &script_uri_invite,
                    &channel,
                    &identity,
                )) {
                    Ok(url) => Ok(Self::task_ok(serde_json::json!({
                        "linkUrl": url,
                        "channel": channel,
                        "identity": identity,
                        "expiresInMinutes": crate::delegation::LINK_TOKEN_MINUTES,
                    }))),
                    Err(refusal) => Ok(Self::task_failure(
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
                    return Ok(Self::task_ok(serde_json::json!({
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
                        Ok(Self::task_ok(serde_json::json!({
                            "authenticated": true,
                            "granted": live,
                            "expired": !live,
                            "expiresAt": grant.expires_at.to_rfc3339(),
                            "scopes": grant.scopes.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                            "consentUrl": crate::delegation::consent_url(&script_uri_grant),
                        })))
                    }
                    Ok(None) => Ok(Self::task_ok(serde_json::json!({
                        "authenticated": true,
                        "granted": false,
                        "expired": false,
                        "scopes": [],
                        "consentUrl": crate::delegation::consent_url(&script_uri_grant),
                    }))),
                    Err(e) => Ok(Self::task_failure(
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

    /// The envelope shape `tasks_prelude.js` unwraps.
    fn task_ok(value: serde_json::Value) -> String {
        serde_json::json!({ "ok": value }).to_string()
    }

    fn task_failure(name: &str, message: &str) -> String {
        serde_json::json!({ "error": { "name": name, "message": message } }).to_string()
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
    /// caller-chosen `UserContext`, and `dispatcher.sendMessage` already
    /// builds a nested runtime from inside a running host call. This is those
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
                    // deploys nothing must not set that in motion, for the
                    // reason `dispatcher.sendMessage` must not.
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

                let narrowed = match crate::sandbox::narrow(&user_run, &requested) {
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
        // `messageData` is whatever the script has: an object is serialized
        // here, a JSON string is passed through, and an omitted argument is
        // the empty object the listeners used to be handed.
        let config_send = self.config.clone();
        // Whoever is dispatching. A listener is part of serving that caller's
        // invocation, so it runs as they do — see `execute_message_handler`.
        let user_ctx_send = self.user_context.clone();
        let send_message = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  message_type: String,
                  message_data: Opt<rquickjs::Value<'_>>|
                  -> JsResult<String> {
                let message_data_json = match message_data.0 {
                    Some(value) if !value.is_undefined() && !value.is_null() => {
                        Some(json_arg(value, "messageData")?)
                    }
                    _ => None,
                };
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

                // A listener runs under the sending caller's context, so a
                // narrowed execution that dispatches hands the listener
                // exactly what it holds itself — the narrowing follows the
                // message rather than being escaped by it. This gate is the
                // other half: whether the narrowed turn may set anything in
                // motion at all.
                if !user_ctx_send.has_capability(&Capability::SendMessages) {
                    return Ok(capability_refusal(
                        "dispatcher.sendMessage",
                        &Capability::SendMessages,
                        &user_ctx_send,
                    ));
                }

                // Get message data as JSON string
                let message_data_json = message_data_json.unwrap_or_else(|| "{}".to_string());

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
                        user_ctx_send.clone(),
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

        // post(messageType, data) — the same fan-out, queued rather than run.
        //
        // `sendMessage` runs every listener inline: on the sender's budget, in
        // the sender's execution, under the sender's context. That is right
        // for a message whose result the sender needs, and wrong for anything
        // slow, since one listener's work is charged to whoever set it off.
        //
        // Posting resolves the listeners now and enqueues one task each, so
        // the sender returns immediately and each listener gets a budget, a
        // retry and a visible state of its own. The listeners are resolved at
        // post time, because the fan-out is to whoever was listening when the
        // message was sent — not to whoever happens to be listening by the
        // time the queue gets to it.
        //
        // The queued listener runs in *script context*, not the sender's.
        // Inline it holds what the sender held, which is exactly as much as
        // the sender could have done itself; queued, there is no sender left
        // to borrow from, and the alternative — keeping a caller's authority
        // alive in a row — is the delegation question, which is not answered
        // by a convenience method on the dispatcher.
        let config_post = self.config.clone();
        let user_post = self.user_context.clone();
        let post_message = Function::new(
            ctx.clone(),
            move |message_type: String, message_data_json: Opt<String>| -> JsResult<String> {
                let message_type = message_type.trim().to_string();
                if message_type.is_empty() {
                    return Ok(Self::task_failure(
                        "TypeError",
                        "dispatcher.post: message type cannot be empty",
                    ));
                }

                if config_post.is_dry_run() {
                    return Ok(Self::task_failure(
                        "DryRunError",
                        &format!(
                            "dispatcher.post: '{}' not queued - this is a dry run",
                            message_type
                        ),
                    ));
                }

                let message_data: serde_json::Value = match message_data_json.0 {
                    Some(raw) => match serde_json::from_str(&raw) {
                        Ok(value) => value,
                        Err(e) => {
                            return Ok(Self::task_failure(
                                "TypeError",
                                &format!("dispatcher.post: message data is not valid JSON: {}", e),
                            ));
                        }
                    },
                    None => serde_json::json!({}),
                };

                let listeners =
                    match crate::dispatcher::GLOBAL_DISPATCHER.get_listeners(&message_type) {
                        Ok(listeners) => listeners,
                        Err(e) => {
                            return Ok(Self::task_failure(
                                "Error",
                                &format!("dispatcher.post: failed to read listeners: {}", e),
                            ));
                        }
                    };

                let mut queued = Vec::new();
                for listener in listeners.iter() {
                    let new_task = crate::tasks::NewTask {
                        script_uri: listener.script_uri.clone(),
                        handler_name: listener.handler_name.clone(),
                        payload: serde_json::json!({
                            "messageType": message_type,
                            "messageData": message_data,
                        }),
                        run_at: None,
                        max_attempts: None,
                        enqueued_by: user_post.user_id.clone(),
                        kind: crate::tasks::TaskKind::Message,
                        // A queued listener runs in its own script's context:
                        // there is no sender left to borrow authority from.
                        run_as: None,
                        // Posting is a fan-out: one message reaches every
                        // listener, and the whole point is that they do not
                        // wait on each other. A lane here would serialise
                        // unrelated scripts because one message named them
                        // together. A listener that must not run beside
                        // itself is a different question, and one the
                        // dispatcher has no vocabulary for yet.
                        lane: None,
                    };

                    match crate::tasks::blocking::enqueue(new_task) {
                        Ok(task) => queued.push(task.task_id.to_string()),
                        Err(e) => {
                            // Partially queued is reported rather than hidden:
                            // some listeners will run and the caller has to
                            // know which, since there is no transaction across
                            // a fan-out.
                            return Ok(Self::task_failure(
                                "Error",
                                &format!(
                                    "dispatcher.post: queued {} of {} listeners for '{}', then: {}",
                                    queued.len(),
                                    listeners.len(),
                                    message_type,
                                    e
                                ),
                            ));
                        }
                    }
                }

                Ok(Self::task_ok(serde_json::json!({
                    "messageType": message_type,
                    "queued": queued.len(),
                    "taskIds": queued,
                })))
            },
        )?;

        dispatcher_obj.set("registerListener", register_listener)?;
        dispatcher_obj.set("sendMessage", send_message)?;
        dispatcher_obj.set("__post", post_message)?;
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
    user_context: UserContext,
) -> Result<(), String> {
    use rquickjs::Context;

    // The same runtime every other entry point gets. Built bare, a listener ran
    // with no memory limit, no stack limit and no interrupt handler — so the
    // one execution path a script reaches by dispatching a message was the one
    // path where `javascript.max_memory_bytes`, `stack_size_bytes` and
    // `execution_timeout_ms` did not apply, and a listener that looped never
    // stopped. The budget guard has to outlive the runtime; dropping it early
    // would leave the handler's host calls unbounded again.
    let (rt, _budget) =
        crate::js_engine::create_sandboxed_runtime(&crate::js_engine::current_execution_limits())?;
    let ctx = Context::full(&rt).map_err(|e| format!("Failed to create context: {}", e))?;

    let setup = ctx.with(|ctx| -> Result<(), String> {
        // The dispatching caller's own context, not one of the engine's.
        //
        // A listener used to run as `UserContext::admin("dispatcher")`, which
        // made `dispatcher.sendMessage` a way to escalate: it is reachable from
        // inside any script serving any request, so an anonymous visitor could
        // set off a handler holding `ManageScriptDatabase`, `WriteAssets` and
        // `AdministerEngine` — powers they do not have and the sending script
        // did not have either. A listener is part of serving the invocation
        // that dispatched to it, and a script serving a request runs under the
        // requesting user's context, so it holds exactly what the sender held:
        // no more, and no less.

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
                &script_uri,
                crate::middleware::generate_request_id(),
                Some(message_type.to_string()),
            ),
            // Not acting for anybody: nothing to narrow.
            delegated_scopes: None,
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
            "assetStorage",
            "scriptStorage",
            "personalStorage",
            "secretStorage",
            "schedulerService",
            "scriptTasks",
            "personalTasks",
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
    // The stream and dispatch bindings file an audit event on a spawned task,
    // so this one needs a runtime where the others do not.
    #[tokio::test]
    async fn the_data_arguments_take_the_object_their_examples_pass() {
        for call in [
            "routeRegistry.sendStreamMessage('/events/x', { type: 'alert', n: 1 })",
            "routeRegistry.sendStreamMessageFiltered('/events/x', { type: 'alert' }, \
             JSON.stringify({ role: 'admin' }))",
            "dispatcher.sendMessage('marshalling.test.type', { userId: '123' })",
        ] {
            // The call is wrapped in JavaScript so a refusal comes back as
            // text: what is being asserted is which refusal it is, and an
            // exception out of `eval` would take that with it.
            let result = eval_outside_registration_phase(&format!(
                "(function () {{ try {{ return String({}); }}                  catch (e) {{ return 'threw: ' + e; }} }})()",
                call
            ));
            assert!(
                !result.contains("converting from js"),
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
