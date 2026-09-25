//! Every limit a solution developer can meet, in one place.
//!
//! A limit is only useful to somebody writing against the engine if they can
//! find it, and the two documents they read — `aiwebengine.d.ts` and
//! `/engine/openapi.json` — cannot describe what they cannot see. Both read
//! them from here: [`snapshot`] is what `/engine/openapi.json` publishes as
//! `x-aiwebengine-limits`, and [`render_placeholders`] is what fills the
//! `{{limits...}}` markers in the type definitions as they are served.
//!
//! The prose half used to be typed out by hand, and it drifted the way a
//! second copy of a number always does: `init()`'s budget has been documented
//! as 5 s, as 10 s and as 30 s, each of them true of some engine at some
//! point. There is now one number per limit, in the code that enforces it.
//!
//! Nothing here is a limit of its own. Every field either reads the constant
//! that enforces it or the configuration captured at startup, because a second
//! copy of a number is a number that will disagree with the first one — which
//! is exactly how a document stops being worth reading.

use serde::Serialize;
use std::sync::OnceLock;

use crate::config::AppConfig;

/// The configured half: values that differ between deployments, captured once
/// at startup because the configuration itself is not held anywhere global.
#[derive(Debug, Clone, Copy)]
struct ConfiguredLimits {
    max_concurrent_executions: usize,
    max_request_body_bytes: usize,
    max_upload_bytes: usize,
    db_max_connections: u32,
    db_statement_timeout_ms: u64,
    db_lock_timeout_ms: u64,
    db_idle_in_transaction_timeout_ms: u64,
    revisions_prune_enabled: bool,
    revisions_retention_days: u32,
    revisions_keep_per_script: u32,
    logs_prune_enabled: bool,
}

impl ConfiguredLimits {
    fn from_config(config: &AppConfig) -> Self {
        Self {
            max_concurrent_executions: config.javascript.max_concurrent_executions,
            max_request_body_bytes: config.security.max_request_body_bytes,
            max_upload_bytes: config.repository.max_upload_size_bytes,
            db_max_connections: config.repository.max_connections,
            db_statement_timeout_ms: config.repository.statement_timeout_ms,
            db_lock_timeout_ms: config.repository.lock_timeout_ms,
            db_idle_in_transaction_timeout_ms: config.repository.idle_in_transaction_timeout_ms,
            revisions_prune_enabled: config.revisions.prune_enabled,
            revisions_retention_days: config.revisions.retention_days,
            revisions_keep_per_script: config.revisions.keep_per_script,
            logs_prune_enabled: config.logs.prune_enabled,
        }
    }
}

impl Default for ConfiguredLimits {
    fn default() -> Self {
        Self::from_config(&AppConfig::default())
    }
}

static CONFIGURED: OnceLock<ConfiguredLimits> = OnceLock::new();

/// Records the configuration-derived limits, so the published document
/// describes the engine that is running rather than the defaults it shipped
/// with. Returns false if they were already recorded.
pub fn configure(config: &AppConfig) -> bool {
    CONFIGURED
        .set(ConfiguredLimits::from_config(config))
        .is_ok()
}

/// What a script may spend and how large anything it handles may be.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    /// Facts about the execution model that are not numbers, spelled out
    /// because they surprise people who expect a browser or Node.
    pub notes: Vec<&'static str>,
    pub execution: ExecutionLimits,
    pub size: SizeLimits,
    pub database: DatabaseLimits,
    pub fetch: FetchLimits,
    pub graphql: GraphQlLimits,
    pub scheduler: SchedulerLimits,
    pub search: SearchLimits,
    pub retention: RetentionLimits,
    pub rate_limits: Vec<RateLimitBudget>,
}

/// Time, memory and stack a single invocation may spend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionLimits {
    /// Wall clock for one invocation (`javascript.execution_timeout_ms`).
    pub timeout_ms: u64,
    /// Wall clock for `init()` (`javascript.init_timeout_ms`, defaulting to
    /// `timeout_ms`).
    pub init_timeout_ms: u64,
    /// Wall clock for one scheduled handler (`javascript.job_timeout_ms`,
    /// defaulting to `timeout_ms`).
    pub job_timeout_ms: u64,
    pub max_memory_bytes: usize,
    pub stack_size_bytes: usize,
    /// Scripts that may run at once. Past this a caller waits for a slot
    /// inside the timeout it already had, rather than being refused.
    pub max_concurrent_executions: usize,
    /// Budget for one test module, and for a whole `run_tests` run.
    pub test_module_timeout_ms: u64,
    pub test_run_timeout_ms: u64,
    /// Ceiling a `/engine/check` caller's own `timeout_ms` is clamped to.
    pub check_max_timeout_ms: u64,
}

/// How large anything a script stores or is handed may be, in bytes.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SizeLimits {
    /// A script's root source (`repository.max_script_size_bytes`).
    pub max_script_source_bytes: usize,
    /// One asset's stored content.
    pub max_asset_bytes: usize,
    pub max_asset_uri_chars: usize,
    /// One `scriptStorage` / `personalStorage` value, and one secret.
    pub max_storage_value_bytes: usize,
    pub max_secret_value_bytes: usize,
    /// An incoming request body, on an engine-owned path and on a script's
    /// route alike (`security.max_request_body_bytes`). Past it the caller
    /// gets a 413 and the handler is never entered.
    pub max_request_body_bytes: usize,
    /// A form-encoded or multipart request body, plus 64 KB of framing
    /// (`repository.max_upload_size_bytes`). Everything else is bounded by
    /// `max_request_body_bytes`.
    pub max_upload_bytes: usize,
    /// Files in one `/engine/assets/batch` write, their combined size, and the
    /// request body carrying them — a little above the content bound, since a
    /// file that is not text still travels base64 and costs a third more.
    pub max_batch_files: usize,
    pub max_batch_bytes: usize,
    pub max_batch_body_bytes: usize,
    /// Edits in one `PATCH /engine/assets` or `/engine/edit_script` call.
    pub max_patch_edits: usize,
    pub max_markdown_bytes: usize,
    pub max_template_bytes: usize,
    /// Bytes of diff `/engine/revisions/diff` will render before truncating.
    pub max_revision_diff_bytes: usize,
    /// Characters in an `import` specifier.
    pub max_module_specifier_chars: usize,
}

/// The script-scoped database: what a script may create, and what bounds a
/// call into it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseLimits {
    pub max_tables_per_script: usize,
    pub max_columns_per_table: usize,
    pub max_identifier_chars: usize,
    /// Rows `database.query` returns when no limit is named, and the ceiling a
    /// named one is silently clamped to.
    pub default_query_limit: i64,
    pub max_query_limit: i64,
    /// Pool connections for the whole engine instance. Every JavaScript
    /// database call holds one for its round trip.
    pub max_connections: u32,
    /// Postgres-side guards, in milliseconds; `0` means the guard is off.
    /// These are the only limits that can end a statement already running: a
    /// script's own budget is enforced between JavaScript operations and
    /// cannot reach inside a host call.
    pub statement_timeout_ms: u64,
    pub lock_timeout_ms: u64,
    pub idle_in_transaction_timeout_ms: u64,
}

/// What `fetch` will and will not do.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FetchLimits {
    pub max_response_bytes: usize,
    /// Default per-request timeout, shortened to whatever is left of the
    /// handler's execution budget.
    pub default_timeout_ms: u64,
    pub max_redirects: usize,
    pub allowed_schemes: Vec<&'static str>,
    /// Whether localhost and private, loopback and link-local addresses are
    /// refused — including a public host that resolves to one, and every hop
    /// of a redirect chain.
    pub private_addresses_blocked: bool,
    /// Content codings the engine asks for and can undo. A server answering
    /// in anything else is an error rather than an unreadable body.
    pub supported_encodings: &'static str,
}

/// GraphQL, as a script reaches it and as a client connects to it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GraphQlLimits {
    pub max_query_chars: usize,
    pub max_variables_chars: usize,
    pub max_subscriptions_per_connection: usize,
}

/// Scheduled jobs.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SchedulerLimits {
    pub min_recurring_interval_ms: i64,
    pub max_job_name_chars: usize,
}

/// What one `/engine/search` answers with before it stops looking.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchLimits {
    pub max_files: usize,
    pub max_matches_per_file: usize,
    /// Matches one `grep` of a single file — `/engine/read_script?grep=` or
    /// `/engine/assets?grep=` — reports before it stops looking.
    pub max_grep_matches_per_read: usize,
    pub max_pattern_chars: usize,
    /// A matching line longer than this comes back truncated, and says so.
    pub max_line_chars: usize,
}

/// What survives, and for how long.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RetentionLimits {
    pub logs: LogRetentionLimits,
    pub revisions: RevisionRetentionLimits,
    /// Console lines captured and returned by `/engine/eval` and
    /// `/engine/check` before the rest are dropped and counted.
    pub max_captured_console_lines: usize,
}

/// Log retention. Either clause alone removes a line: a count leaves a
/// thousand dormant scripts sitting on their full quota, an age window leaves
/// one script logging in a loop to fill it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LogRetentionLimits {
    pub prune_enabled: bool,
    pub keep_per_script: i64,
    pub retention_hours: i32,
}

/// Revision retention. Unlike logs, a revision has to fall outside *both*
/// clauses before it goes, and a labelled revision, the newest that
/// initialised cleanly and the newest of each script are kept regardless.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RevisionRetentionLimits {
    pub prune_enabled: bool,
    pub retention_days: u32,
    pub keep_per_script: u32,
}

/// One throttled surface. These bound the engine's own endpoints, not script
/// routes — a solution that wants its own traffic throttled writes that
/// itself.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitBudget {
    /// The bucket's name in the limiter, and what it is keyed by.
    pub key: &'static str,
    pub scope: &'static str,
    pub description: &'static str,
    /// Requests available at once, and how fast the budget refills.
    pub max_tokens: u32,
    pub refill_per_second: f64,
}

/// The limits in effect right now: the constants that enforce them, plus the
/// configuration this engine started with.
pub fn snapshot() -> Limits {
    let configured = CONFIGURED.get().copied().unwrap_or_default();
    let js = crate::js_engine::current_execution_limits();
    let (test_module_timeout_ms, test_run_timeout_ms) =
        crate::script_test::configured_test_timeouts();
    let logs = crate::log_retention::configured();

    Limits {
        notes: vec![
            "Every invocation gets a fresh runtime: no value assigned at module scope survives \
             one request into the next. Persist through scriptStorage, personalStorage or the \
             script database.",
            "There are no timers. setTimeout and setInterval do not exist, and a promise still \
             pending when a handler returns can never settle.",
            "There is no concurrency. Every host call — fetch, database, a script's tables — blocks \
             until it has an answer, so `await` sequences work rather than overlapping it, and \
             Promise.all over several fetches runs them one after another.",
            "Imports resolve to this script's own assets. A specifier is relative ('./x.ts', \
             '../y.ts') or an asset-root path ('server/x.ts'); there is no dynamic import(), no \
             npm and no node built-ins.",
            "A script's execution budget is enforced between JavaScript operations and as a \
             ceiling on each host call, so a slow database statement cannot outlive it — but \
             only Postgres can end a statement already running.",
            "Engine administration (scripts, assets, users, secrets, logs) is not reachable from \
             JavaScript at all; it lives on these /engine endpoints and the engine MCP tools.",
        ],
        execution: ExecutionLimits {
            timeout_ms: js.timeout_ms,
            init_timeout_ms: crate::script_init::configured_init_timeout_ms(),
            job_timeout_ms: crate::scheduler::configured_job_timeout_ms(),
            max_memory_bytes: js.max_memory_mb * 1024 * 1024,
            stack_size_bytes: js.stack_size_bytes,
            max_concurrent_executions: configured.max_concurrent_executions,
            test_module_timeout_ms,
            test_run_timeout_ms,
            check_max_timeout_ms: crate::script_check::MAX_CHECK_TIMEOUT_MS,
        },
        size: SizeLimits {
            max_script_source_bytes: js.max_script_size_bytes,
            // The endpoints bound a request body a little above what storage
            // accepts, so the smaller of the two is what a caller actually
            // gets.
            max_asset_bytes: crate::engine_api::MAX_ASSET_BYTES
                .min(crate::repository::MAX_ASSET_CONTENT_BYTES),
            max_asset_uri_chars: crate::repository::MAX_ASSET_URI_CHARS,
            max_storage_value_bytes: crate::repository::MAX_STORAGE_VALUE_BYTES,
            max_secret_value_bytes: crate::repository::MAX_STORAGE_VALUE_BYTES,
            max_request_body_bytes: configured.max_request_body_bytes,
            max_upload_bytes: configured.max_upload_bytes,
            max_batch_files: crate::engine_api::MAX_BATCH_FILES,
            max_batch_bytes: crate::engine_api::MAX_BATCH_BYTES,
            max_batch_body_bytes: crate::engine_api::MAX_BATCH_BODY_BYTES,
            max_patch_edits: crate::engine_api::MAX_PATCH_EDITS,
            max_markdown_bytes: crate::conversion::MAX_MARKDOWN_SIZE,
            max_template_bytes: crate::conversion::MAX_TEMPLATE_SIZE,
            max_revision_diff_bytes: crate::revisions::MAX_DIFF_BYTES,
            max_module_specifier_chars: crate::module_loader::MAX_MODULE_SPECIFIER_LENGTH,
        },
        database: DatabaseLimits {
            max_tables_per_script: crate::db_schema_utils::MAX_TABLES_PER_SCRIPT,
            max_columns_per_table: crate::db_schema_utils::MAX_COLUMNS_PER_TABLE,
            max_identifier_chars: crate::db_schema_utils::MAX_IDENTIFIER_LENGTH,
            default_query_limit: crate::repository::DEFAULT_QUERY_LIMIT,
            max_query_limit: crate::repository::MAX_QUERY_LIMIT,
            max_connections: configured.db_max_connections,
            statement_timeout_ms: configured.db_statement_timeout_ms,
            lock_timeout_ms: configured.db_lock_timeout_ms,
            idle_in_transaction_timeout_ms: configured.db_idle_in_transaction_timeout_ms,
        },
        fetch: FetchLimits {
            max_response_bytes: crate::http_client::MAX_RESPONSE_SIZE,
            default_timeout_ms: crate::http_client::DEFAULT_TIMEOUT.as_millis() as u64,
            max_redirects: crate::http_client::MAX_REDIRECTS,
            allowed_schemes: vec!["http", "https"],
            private_addresses_blocked: true,
            supported_encodings: crate::http_client::SUPPORTED_ENCODINGS,
        },
        graphql: GraphQlLimits {
            max_query_chars: crate::security::secure_globals::MAX_GRAPHQL_QUERY_CHARS,
            max_variables_chars: crate::security::secure_globals::MAX_GRAPHQL_VARIABLES_CHARS,
            max_subscriptions_per_connection: crate::graphql_ws::MAX_SUBSCRIPTIONS_PER_CONNECTION,
        },
        scheduler: SchedulerLimits {
            min_recurring_interval_ms: crate::scheduler::MIN_RECURRING_INTERVAL_MS,
            max_job_name_chars: crate::scheduler::MAX_JOB_NAME_CHARS,
        },
        search: SearchLimits {
            max_files: crate::engine_api::MAX_SEARCH_FILES,
            max_matches_per_file: crate::engine_api::MAX_SEARCH_MATCHES_PER_FILE,
            max_grep_matches_per_read: crate::engine_api::MAX_GREP_MATCHES,
            max_pattern_chars: crate::engine_api::MAX_GREP_PATTERN_CHARS,
            max_line_chars: crate::engine_api::MAX_GREP_LINE_CHARS,
        },
        retention: RetentionLimits {
            logs: LogRetentionLimits {
                prune_enabled: configured.logs_prune_enabled,
                keep_per_script: logs.keep_per_script,
                retention_hours: logs.keep_hours,
            },
            revisions: RevisionRetentionLimits {
                prune_enabled: configured.revisions_prune_enabled,
                retention_days: configured.revisions_retention_days,
                keep_per_script: configured.revisions_keep_per_script,
            },
            max_captured_console_lines: crate::security::secure_globals::MAX_CAPTURED_CONSOLE_LINES,
        },
        rate_limits: rate_limit_budgets(),
    }
}

/// The throttled surfaces, read from the limiter this engine is running so the
/// document cannot describe a budget nothing enforces. Before startup has set
/// one — a unit test — the list is empty rather than invented.
fn rate_limit_budgets() -> Vec<RateLimitBudget> {
    const SURFACES: [(&str, &str, &str); 4] = [
        (
            "ip",
            "client address",
            "Authentication endpoints: sign-in, registration, guest and recovery.",
        ),
        (
            "login_failure",
            "account",
            "Failed sign-ins for one account, so guessing a password from many addresses meets a \
             wall the per-address budget cannot give. Only failures spend it.",
        ),
        (
            "client_registration",
            "client address",
            "OAuth2 dynamic client registration, which is unauthenticated by design.",
        ),
        (
            "git_sync",
            "account",
            "Git pull and push: each is an archive transfer and a tree rewrite.",
        ),
    ];

    let Some(limiter) = crate::security::rate_limiting::shared() else {
        return Vec::new();
    };

    SURFACES
        .iter()
        .filter_map(|(key, scope, description)| {
            let config = limiter.config_named(key)?;
            Some(RateLimitBudget {
                key,
                scope,
                description,
                max_tokens: config.max_tokens,
                refill_per_second: config.refill_rate,
            })
        })
        .collect()
}

/// Render `text` with every `{{limits...}}` placeholder replaced by the value
/// this engine is running.
///
/// The type declarations are the other document a solution developer reads,
/// and they used to carry the numbers as prose typed out by hand — which is
/// how `init()`'s budget came to be documented as 5 s, as 10 s and as 30 s,
/// each of them true of some engine at some point. A placeholder cannot drift:
/// there is one number, it lives in the code that enforces it, and both
/// documents read it from here.
///
/// An unrecognised placeholder is left as it stands rather than removed, so a
/// typo shows up in the served file — and in the test below — instead of
/// quietly deleting the sentence it was part of.
pub fn render_placeholders(text: &str) -> String {
    let limits = snapshot();
    let mut rendered = text.to_string();

    // The bullets are already prose in [`Limits::notes`], so the declarations
    // hold the marker rather than a second copy of the sentences.
    if let Some(position) = rendered.find(NOTES_PLACEHOLDER) {
        let indent = line_prefix(&rendered, position);
        let bullets = limits
            .notes
            .iter()
            .map(|note| wrap_bullet(note, &indent))
            .collect::<Vec<_>>()
            .join(&format!("\n{}", indent));
        rendered = rendered.replace(NOTES_PLACEHOLDER, bullets.trim_start());
    }

    for (placeholder, value) in placeholder_values(&limits) {
        rendered = rendered.replace(&placeholder, &value);
    }
    rendered
}

/// The marker the notes are rendered over.
const NOTES_PLACEHOLDER: &str = "{{limits.notes}}";

/// The text opening the line a placeholder sits on — ` * ` inside a JSDoc
/// comment — so every line the marker expands into continues that comment
/// rather than ending it.
///
/// The marker is expected to sit alone on its line after whatever opens it,
/// which is how both documents use it.
fn line_prefix(text: &str, position: usize) -> String {
    let line_start = text[..position].rfind('\n').map_or(0, |i| i + 1);
    text[line_start..position].to_string()
}

/// One note as a `-` bullet, wrapped to the width the file is written at.
fn wrap_bullet(note: &str, indent: &str) -> String {
    const WIDTH: usize = 76;
    let mut lines = Vec::new();
    let mut current = String::from("- ");
    for word in note.split_whitespace() {
        if current.trim_end() != "-" && current.len() + 1 + word.len() + indent.len() > WIDTH {
            lines.push(std::mem::take(&mut current).trim_end().to_string());
            current = String::from("  ");
        }
        if !current.ends_with(' ') {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.trim().is_empty() {
        lines.push(current.trim_end().to_string());
    }
    lines.join(&format!("\n{}", indent))
}

/// Every placeholder the documents may use, with the value it stands for.
///
/// The formatter is chosen per entry rather than inferred from the name: a
/// number of bytes reads as `10 MB` and a number of files as `256`, and only
/// the list knows which is which.
fn placeholder_values(limits: &Limits) -> Vec<(String, String)> {
    let execution = &limits.execution;
    let size = &limits.size;
    let database = &limits.database;
    let fetch = &limits.fetch;
    let graphql = &limits.graphql;
    let scheduler = &limits.scheduler;
    let logs = &limits.retention.logs;
    let revisions = &limits.retention.revisions;

    let entries: Vec<(&str, String)> = vec![
        ("execution.timeout", duration(execution.timeout_ms)),
        ("execution.initTimeout", duration(execution.init_timeout_ms)),
        ("execution.jobTimeout", duration(execution.job_timeout_ms)),
        (
            "execution.testModuleTimeout",
            duration(execution.test_module_timeout_ms),
        ),
        (
            "execution.testRunTimeout",
            duration(execution.test_run_timeout_ms),
        ),
        ("execution.maxMemory", bytes(execution.max_memory_bytes)),
        ("execution.stackSize", bytes(execution.stack_size_bytes)),
        (
            "execution.stackFrames",
            // A JavaScript frame costs about a kilobyte, which is the same
            // reasoning `config.toml` gives for the setting itself.
            count(execution.stack_size_bytes / 1024),
        ),
        (
            "execution.maxConcurrent",
            count(execution.max_concurrent_executions),
        ),
        ("size.maxScriptSource", bytes(size.max_script_source_bytes)),
        ("size.maxAsset", bytes(size.max_asset_bytes)),
        ("size.maxAssetUriChars", count(size.max_asset_uri_chars)),
        ("size.maxStorageValue", bytes(size.max_storage_value_bytes)),
        ("size.maxSecretValue", bytes(size.max_secret_value_bytes)),
        ("size.maxRequestBody", bytes(size.max_request_body_bytes)),
        ("size.maxUpload", bytes(size.max_upload_bytes)),
        ("size.maxBatchFiles", count(size.max_batch_files)),
        ("size.maxBatchBytes", bytes(size.max_batch_bytes)),
        ("size.maxPatchEdits", count(size.max_patch_edits)),
        ("size.maxMarkdown", bytes(size.max_markdown_bytes)),
        ("size.maxTemplate", bytes(size.max_template_bytes)),
        (
            "size.maxModuleSpecifierChars",
            count(size.max_module_specifier_chars),
        ),
        (
            "database.maxTablesPerScript",
            count(database.max_tables_per_script),
        ),
        (
            "database.maxColumnsPerTable",
            count(database.max_columns_per_table),
        ),
        (
            "database.maxIdentifierChars",
            count(database.max_identifier_chars),
        ),
        (
            "database.defaultQueryLimit",
            count(database.default_query_limit as usize),
        ),
        (
            "database.maxQueryLimit",
            count(database.max_query_limit as usize),
        ),
        (
            "database.maxConnections",
            count(database.max_connections as usize),
        ),
        (
            "database.statementTimeout",
            duration(database.statement_timeout_ms),
        ),
        ("database.lockTimeout", duration(database.lock_timeout_ms)),
        (
            "database.idleInTransactionTimeout",
            duration(database.idle_in_transaction_timeout_ms),
        ),
        ("fetch.maxResponse", bytes(fetch.max_response_bytes)),
        ("fetch.defaultTimeout", duration(fetch.default_timeout_ms)),
        ("fetch.maxRedirects", count(fetch.max_redirects)),
        ("fetch.schemes", fetch.allowed_schemes.join(" and ")),
        ("fetch.encodings", fetch.supported_encodings.to_string()),
        ("graphql.maxQueryChars", count(graphql.max_query_chars)),
        (
            "graphql.maxVariablesChars",
            count(graphql.max_variables_chars),
        ),
        (
            "scheduler.minRecurringInterval",
            duration(scheduler.min_recurring_interval_ms.max(0) as u64),
        ),
        (
            "scheduler.maxJobNameChars",
            count(scheduler.max_job_name_chars),
        ),
        (
            "retention.logsKeepPerScript",
            count(logs.keep_per_script.max(0) as usize),
        ),
        (
            "retention.logsRetentionHours",
            count(logs.retention_hours.max(0) as usize),
        ),
        (
            "retention.revisionsRetentionDays",
            count(revisions.retention_days as usize),
        ),
        (
            "retention.revisionsKeepPerScript",
            count(revisions.keep_per_script as usize),
        ),
    ];

    entries
        .into_iter()
        .map(|(name, value)| (format!("{{{{limits.{}}}}}", name), value))
        .collect()
}

/// A size as a reader states it: whole binary megabytes as `128 MB`, anything
/// else as its exact count, because `10,000,000 bytes` is a limit somebody
/// chose and `9.54 MB` is not.
fn bytes(value: usize) -> String {
    const MB: usize = 1024 * 1024;
    if value >= MB && value.is_multiple_of(MB) {
        format!("{} MB", value / MB)
    } else if value >= 1024 && value.is_multiple_of(1024) {
        format!("{} KB", value / 1024)
    } else {
        format!("{} bytes", count(value))
    }
}

/// A budget in the unit it was chosen in: `5 min`, `30 s`, `100 ms`.
fn duration(millis: u64) -> String {
    if millis >= 60_000 && millis.is_multiple_of(60_000) {
        format!("{} min", millis / 60_000)
    } else if millis >= 1_000 && millis.is_multiple_of(1_000) {
        format!("{} s", millis / 1_000)
    } else {
        format!("{} ms", millis)
    }
}

/// A count with thousands separators, so `10000` reads as `10,000`.
fn count(value: usize) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The snapshot has to answer before startup as well as after it: the
    /// OpenAPI document is generated in contexts — a test binary, a
    /// `--validate-config` run — where nothing has called [`configure`].
    #[test]
    fn snapshot_answers_without_configuration() {
        let limits = snapshot();
        assert!(limits.execution.timeout_ms > 0);
        assert!(limits.size.max_script_source_bytes > 0);
        assert!(limits.database.max_query_limit >= limits.database.default_query_limit);
        assert!(!limits.notes.is_empty());
    }

    /// Every published number must come from the code that enforces it. These
    /// are the ones a document would otherwise be tempted to retype.
    #[test]
    fn published_values_match_the_enforcing_constants() {
        let limits = snapshot();
        assert_eq!(
            limits.database.max_tables_per_script,
            crate::db_schema_utils::MAX_TABLES_PER_SCRIPT
        );
        assert_eq!(
            limits.fetch.max_response_bytes,
            crate::http_client::MAX_RESPONSE_SIZE
        );
        assert_eq!(
            limits.size.max_storage_value_bytes,
            crate::repository::MAX_STORAGE_VALUE_BYTES
        );
        assert_eq!(
            limits.scheduler.min_recurring_interval_ms,
            crate::scheduler::MIN_RECURRING_INTERVAL_MS
        );
        // An asset is bounded by storage rather than by the endpoint, which
        // allows a slightly larger body than storage will keep.
        assert!(limits.size.max_asset_bytes <= crate::repository::MAX_ASSET_CONTENT_BYTES);
    }

    /// The declarations the engine serves.
    const TYPE_DEFINITIONS: &str = include_str!("../assets/aiwebengine.d.ts");

    /// The invariant that makes generated prose worth having: a marker that
    /// names nothing survives rendering, so a typo shows up as itself rather
    /// than as a sentence with a hole in it — and this test fails before the
    /// file reaches anyone.
    #[test]
    fn every_placeholder_in_the_declarations_resolves() {
        let rendered = render_placeholders(TYPE_DEFINITIONS);
        let leftovers: Vec<&str> = rendered
            .match_indices("{{limits")
            .map(|(index, _)| {
                let end = rendered[index..]
                    .find("}}")
                    .map_or(rendered.len(), |offset| index + offset + 2);
                &rendered[index..end]
            })
            .collect();
        assert!(
            leftovers.is_empty(),
            "unresolved placeholders in aiwebengine.d.ts: {:?}",
            leftovers
        );
    }

    /// What the reviewer actually asked for: the prose and the published
    /// document cannot disagree, because they are the same numbers.
    #[test]
    fn the_declarations_state_the_limits_the_engine_enforces() {
        let limits = snapshot();
        let rendered = render_placeholders(TYPE_DEFINITIONS);

        for expected in [
            duration(limits.execution.init_timeout_ms),
            duration(limits.execution.timeout_ms),
            bytes(limits.execution.max_memory_bytes),
            bytes(limits.size.max_script_source_bytes),
            count(limits.database.max_tables_per_script),
            count(limits.fetch.max_redirects),
        ] {
            assert!(
                rendered.contains(&expected),
                "the declarations should state `{}`",
                expected
            );
        }
    }

    /// The notes are prose in one place too: the declarations hold the marker
    /// rather than a second copy of the sentences.
    #[test]
    fn the_execution_model_notes_are_rendered_rather_than_repeated() {
        let rendered = render_placeholders(TYPE_DEFINITIONS);
        assert!(!rendered.contains(NOTES_PLACEHOLDER));
        for note in snapshot().notes {
            // The rendered bullet is wrapped, so the opening clause is what
            // survives a line break intact.
            let opening: String = note
                .split_whitespace()
                .take(5)
                .collect::<Vec<_>>()
                .join(" ");
            assert!(
                rendered.contains(&opening),
                "note missing from the declarations: {}",
                opening
            );
        }
        // Each one is a bullet continuing the JSDoc comment it sits in, built
        // from the note rather than from a second copy of its wording.
        let first = snapshot().notes[0];
        let opening: String = first
            .split_whitespace()
            .take(6)
            .collect::<Vec<_>>()
            .join(" ");
        assert!(
            rendered.contains(&format!(" * - {}", opening)),
            "the first note should open a bullet inside the comment"
        );
    }

    /// The rendered block has to stay a comment, and stay readable: every line
    /// of it continues the JSDoc it was spliced into, and none of it runs past
    /// the width the rest of the file is written at.
    #[test]
    fn the_rendered_block_stays_a_readable_comment() {
        let rendered = render_placeholders(TYPE_DEFINITIONS);
        let block: Vec<&str> = rendered
            .lines()
            .skip_while(|line| !line.contains("What a script may spend"))
            .take_while(|line| !line.trim_start().starts_with("*/"))
            .collect();
        assert!(block.len() > 40, "the limits block should be substantial");
        for line in &block {
            assert!(
                line.starts_with(" *"),
                "line left the comment it was rendered into: {:?}",
                line
            );
            assert!(
                line.chars().count() <= 80,
                "line runs past the width the file is written at: {:?}",
                line
            );
        }
    }

    #[test]
    fn values_read_in_the_unit_they_were_chosen_in() {
        assert_eq!(duration(30_000), "30 s");
        assert_eq!(duration(300_000), "5 min");
        assert_eq!(duration(100), "100 ms");
        assert_eq!(bytes(128 * 1024 * 1024), "128 MB");
        assert_eq!(bytes(10_000_000), "10,000,000 bytes");
        assert_eq!(count(10_000), "10,000");
        assert_eq!(count(64), "64");
    }

    /// A deployment's own numbers reach the declarations, which is the whole
    /// reason this is rendered when the file is served rather than when it is
    /// written.
    #[test]
    fn an_unknown_placeholder_is_left_where_a_reader_will_see_it() {
        assert_eq!(
            render_placeholders("a {{limits.nothing.here}} b"),
            "a {{limits.nothing.here}} b"
        );
    }

    #[test]
    fn configuration_reaches_the_snapshot() {
        let mut config = AppConfig::default();
        config.repository.max_connections = 7;
        let configured = ConfiguredLimits::from_config(&config);
        assert_eq!(configured.db_max_connections, 7);
    }
}
