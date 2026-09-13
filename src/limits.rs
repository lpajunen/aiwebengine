//! Every limit a solution developer can meet, in one place.
//!
//! A limit is only useful to somebody writing against the engine if they can
//! find it, and the two documents they read — `aiwebengine.d.ts` and
//! `/engine/openapi.json` — cannot describe what they cannot see. The prose
//! half lives in the type definitions, which are compiled into the binary and
//! therefore cannot name a number a deployment has changed; this module is the
//! other half, and it answers with the values *this* engine is running:
//! [`snapshot`] is what `/engine/openapi.json` publishes as
//! `x-aiwebengine-limits`.
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
    /// Files in one `/engine/assets/batch` write, their combined decoded size,
    /// and the request body that carries them base64-encoded.
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
            "There is no concurrency. Every host call — fetch, database, dispatcher — blocks \
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

    #[test]
    fn configuration_reaches_the_snapshot() {
        let mut config = AppConfig::default();
        config.repository.max_connections = 7;
        let configured = ConfiguredLimits::from_config(&config);
        assert_eq!(configured.db_max_connections, 7);
    }
}
