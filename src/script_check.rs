//! What a script would do if it were deployed, reported as diagnostics.
//!
//! This is the server's half of a solution developer's checking loop. Their own
//! toolchain — `tsc`, a formatter, a linter — sees the source and nothing else.
//! The engine sees the rest: how *it* resolves asset-backed imports, which
//! delegates a registration actually names, what `init()` costs against the
//! budget a deploy enforces, and what is already registered on the same hosts.
//! Those are the findings collected here.
//!
//! The whole check is a dry run: the script's `init()` really executes, but
//! every registry write is withheld
//! ([`crate::security::secure_globals::GlobalSecurityConfig::dry_run_sink`]),
//! message dispatch is suppressed, and database writes roll back. What it
//! cannot undo is anything the engine does not mediate — an outbound `fetch`,
//! a write to a third-party system. Checking runs the script's own code, and
//! that is also the point: a static pass could not tell you which handler names
//! resolve.

use serde::Serialize;
use serde_json::{Value, json};

use crate::js_engine::{DryRunParams, MissingHandler, RegistrationPassOutcome};
use crate::module_loader::ModuleLoaderError;
use crate::repository;
use crate::security::secure_globals::{CollectedRegistration, RegistrationKind};

/// Fraction of the `init()` budget a script may spend before the check says so.
///
/// Deliberately well below 1.0: a run that fits today at 95% of budget is one
/// cold cache or one slow query from failing the next deploy, and the failure
/// mode is a script that comes up with no routes. The margin is what makes the
/// warning actionable while the script still works.
const INIT_BUDGET_WARN_RATIO: f64 = 0.7;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// The script will not work as deployed.
    Error,
    /// The script works, but something about it is likely to bite later.
    Warning,
}

/// One finding, in the shape an editor or an agent can act on directly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    /// The script URI, or the logical path of the asset module the finding is
    /// in when the engine can attribute it to one.
    pub file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    pub severity: Severity,
    /// Stable identifier for the kind of finding, for callers that route on it
    /// rather than on prose.
    pub code: &'static str,
    pub message: String,
    /// Which checker produced this. Always `engine` today; the field exists so
    /// that diagnostics from a type checker or linter can be merged into the
    /// same list without the caller having to guess which layer complained.
    pub source: &'static str,
}

impl Diagnostic {
    fn error(file: impl Into<String>, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            file: file.into(),
            line: None,
            column: None,
            severity: Severity::Error,
            code,
            message: message.into(),
            source: "engine",
        }
    }

    fn warning(file: impl Into<String>, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Warning,
            ..Self::error(file, code, message)
        }
    }

    fn at(mut self, location: Option<SourceLocation>) -> Self {
        if let Some(location) = location {
            self.file = location.file;
            self.line = Some(location.line);
            self.column = location.column;
        }
        self
    }
}

/// How `init()` went, whether or not it produced diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InitReport {
    /// False when the script defines no `init()` at all.
    pub ran: bool,
    /// Wall-clock cost of the bundle, the program's top level and `init()` —
    /// the same three steps a deploy pays for.
    ///
    /// The bundle is rebuilt for this measurement rather than served from cache,
    /// so it is included. Module *sources* are read from cache when this
    /// instance has them, which is what a redeploy of unchanged assets gets;
    /// a first deploy on a cold instance pays more.
    pub duration_ms: u64,
    /// The budget a real deploy enforces (`javascript.init_timeout_ms`).
    pub budget_ms: u64,
}

/// Everything one check produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckReport {
    pub script_uri: String,
    /// False when any diagnostic is an error.
    pub ok: bool,
    pub diagnostics: Vec<Diagnostic>,
    /// Absent when the script never got as far as running: a bundle that does
    /// not build has no `init()` to time.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub init: Option<InitReport>,
    /// What the script registered, in the order it registered it. This is the
    /// deployed shape of the script — worth reading even when there are no
    /// diagnostics, because a route the author expected and does not see here
    /// is a finding no checker can raise for them.
    pub registrations: Vec<CollectedRegistration>,
}

impl CheckReport {
    fn new(script_uri: String) -> Self {
        Self {
            script_uri,
            ok: true,
            diagnostics: Vec::new(),
            init: None,
            registrations: Vec::new(),
        }
    }

    fn finish(mut self) -> Self {
        self.ok = !self
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == Severity::Error);
        self
    }

    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|e| {
            json!({
                "scriptUri": self.script_uri,
                "ok": false,
                "error": format!("Failed to serialize check report: {}", e),
            })
        })
    }

    pub fn error_count(&self) -> usize {
        self.diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.severity == Severity::Error)
            .count()
    }
}

/// What to check.
pub struct CheckRequest {
    pub script_uri: String,
    /// Candidate source to check instead of what is deployed.
    ///
    /// The point of the whole endpoint: an agent can check the code it is about
    /// to write, rather than deploying it and reading the FATAL afterwards.
    pub content: Option<String>,
    /// Roll back the database writes `init()` makes. On by default.
    pub rollback: bool,
}

/// Runs checks under the engine's configured `init()` budget.
pub struct ScriptChecker {
    init_timeout_ms: u64,
}

impl ScriptChecker {
    pub fn new(init_timeout_ms: u64) -> Self {
        Self { init_timeout_ms }
    }

    /// A checker using the budget configured at startup, so what it reports
    /// about the `init()` cost is measured against the number a deploy on this
    /// instance would actually enforce.
    pub fn with_configured_timeout() -> Self {
        Self::new(crate::script_init::configured_init_timeout_ms())
    }

    pub async fn run(&self, request: CheckRequest) -> CheckReport {
        let script_uri = request.script_uri.clone();
        let init_timeout_ms = self.init_timeout_ms;

        let checked =
            tokio::task::spawn_blocking(move || check_blocking(request, init_timeout_ms)).await;

        match checked {
            Ok(report) => report,
            Err(join_error) => {
                let mut report = CheckReport::new(script_uri.clone());
                report.diagnostics.push(Diagnostic::error(
                    script_uri,
                    "check-failed",
                    format!("The check could not be completed: {}", join_error),
                ));
                report.finish()
            }
        }
    }
}

/// Run a check on the current thread.
///
/// One function on one thread because its steps share state bound to it — the
/// transaction isolating the run is thread-local, and the QuickJS context
/// holding the answers about delegates lives only as long as the run. Callers
/// on an async task go through [`ScriptChecker::run`], which moves this to the
/// blocking pool; the MCP dispatcher is already there and calls it directly.
pub fn check_blocking(request: CheckRequest, init_timeout_ms: u64) -> CheckReport {
    let CheckRequest {
        script_uri,
        content,
        rollback,
    } = request;

    let mut report = CheckReport::new(script_uri.clone());

    let checking_candidate = content.is_some();
    let content = match content.or_else(|| repository::fetch_script(&script_uri)) {
        Some(content) => content,
        None => {
            report.diagnostics.push(Diagnostic::error(
                &script_uri,
                "script-not-found",
                format!(
                    "No script is deployed at '{}' and no content was supplied to check",
                    script_uri
                ),
            ));
            return report.finish();
        }
    };

    // Bundle first and on its own, rather than letting the dry run's own
    // bundling report it. The linker's error type distinguishes a cycle from an
    // unresolvable specifier from a syntax error, and that distinction is lost
    // once the run flattens it into one "Transpilation failed" string.
    if let Err(error) = crate::module_loader::prepare_executable_program(&script_uri, &content) {
        report
            .diagnostics
            .push(bundle_diagnostic(&script_uri, &error));
        if checking_candidate {
            crate::module_loader::invalidate_program(&script_uri);
        }
        // Nothing further is knowable: without a program there is no `init()`
        // to run and no globals to resolve delegates against.
        return report.finish();
    }

    // The probe above left its bundle in the prepared-program cache, and a
    // deploy does not get that head start — leaving it there would hide the
    // bundling cost from the measurement the budget warning is based on. Module
    // *sources* stay cached, matching a redeploy whose assets did not change.
    crate::module_loader::invalidate_program(&script_uri);

    let outcome = crate::js_engine::dry_run_registration_pass(&DryRunParams {
        script_uri: script_uri.clone(),
        script_content: content,
        timeout_ms: init_timeout_ms,
        rollback,
    });

    if checking_candidate {
        // The candidate's bundle is now cached under this script's key. It is
        // validated by a hash of the root content, so a live request would
        // rebuild rather than serve it — but leaving an entry that can only ever
        // miss is worse than leaving none.
        crate::module_loader::invalidate_program(&script_uri);
    }

    collect_diagnostics(&script_uri, &outcome, init_timeout_ms, &mut report);
    report.registrations = outcome.pass.collected;
    report
        .diagnostics
        .extend(route_conflicts(&script_uri, &report.registrations));

    report.finish()
}

/// Turn one registration pass into findings.
fn collect_diagnostics(
    script_uri: &str,
    outcome: &RegistrationPassOutcome,
    budget_ms: u64,
    report: &mut CheckReport,
) {
    let pass = &outcome.pass;

    report.init = Some(InitReport {
        ran: pass.had_init,
        duration_ms: pass.duration_ms,
        budget_ms,
    });

    if let Some(error) = &outcome.error {
        report.diagnostics.push(
            Diagnostic::error(script_uri, "init-failed", error.clone())
                .at(source_location(script_uri, error)),
        );
    }

    for missing in &pass.missing_handlers {
        report
            .diagnostics
            .push(missing_handler_diagnostic(script_uri, missing));
    }

    if !pass.had_init {
        report.diagnostics.push(Diagnostic::warning(
            script_uri,
            "no-init",
            "This script defines no init() function, so it registers nothing: no routes, \
             resolvers, streams, jobs or tools. Export a function named 'init'.",
        ));
    } else if outcome.error.is_none() && pass.collected.is_empty() {
        report.diagnostics.push(Diagnostic::warning(
            script_uri,
            "no-registrations",
            "init() ran but registered nothing, so this script answers no requests. \
             Registration calls only take effect during startup and init().",
        ));
    }

    // Only worth saying when init() actually ran to completion: a run cut short
    // by an error is not a measurement of what the script costs.
    if pass.had_init && outcome.error.is_none() {
        let spent = pass.duration_ms as f64 / budget_ms.max(1) as f64;
        if spent >= INIT_BUDGET_WARN_RATIO {
            report.diagnostics.push(Diagnostic::warning(
                script_uri,
                "init-budget",
                format!(
                    "init() took {}ms of the {}ms budget ({:.0}%). A deploy that exceeds the \
                     budget brings the script up with no routes registered. Move slow work out \
                     of init() — into a scheduled job, or behind the first request that needs it.",
                    pass.duration_ms,
                    budget_ms,
                    spent * 100.0
                ),
            ));
        }
    }
}

fn missing_handler_diagnostic(script_uri: &str, missing: &MissingHandler) -> Diagnostic {
    let what = match missing.kind {
        RegistrationKind::Route => "route",
        RegistrationKind::Stream => "stream",
        RegistrationKind::AssetRoute => "asset route",
        RegistrationKind::GraphqlQuery => "GraphQL query",
        RegistrationKind::GraphqlMutation => "GraphQL mutation",
        RegistrationKind::GraphqlSubscription => "GraphQL subscription",
        RegistrationKind::McpTool => "MCP tool",
        RegistrationKind::McpPrompt => "MCP prompt",
        RegistrationKind::ScheduledJob => "scheduled job",
        RegistrationKind::MessageListener => "listener",
    };

    let detail = match &missing.found_type {
        Some(found) => format!("'{}' is defined, but it is a {}", missing.handler, found),
        None => format!("nothing named '{}' is defined", missing.handler),
    };

    Diagnostic::error(
        script_uri,
        "missing-handler",
        format!(
            "The {} '{}' delegates to '{}', but {}. Handlers are resolved by name against the \
             program's globals when a request arrives, so this fails at call time, not at deploy \
             time. A function defined in an imported module is not a global unless the entrypoint \
             assigns it — `globalThis.{} = {};`.",
            what, missing.name, missing.handler, detail, missing.handler, missing.handler
        ),
    )
}

fn bundle_diagnostic(script_uri: &str, error: &ModuleLoaderError) -> Diagnostic {
    match error {
        ModuleLoaderError::CircularImport(message) => Diagnostic::error(
            script_uri,
            "circular-import",
            format!(
                "{}. The engine's bundler refuses import cycles, so this script cannot be \
                 deployed at all — note that `tsc` accepts them, which is why this only shows \
                 up here. Break the cycle by moving the shared declarations into a module both \
                 sides import.",
                message
            ),
        ),
        ModuleLoaderError::UnsupportedImport(message) => {
            Diagnostic::error(script_uri, "unsupported-import", message.clone())
        }
        ModuleLoaderError::InvalidSpecifier(message) => {
            Diagnostic::error(script_uri, "invalid-import", message.clone())
        }
        ModuleLoaderError::Transpilation(message) => {
            Diagnostic::error(script_uri, "transpile-error", message.clone())
                .at(source_location(script_uri, message))
        }
    }
}

/// Registrations that another script already claims on a host both serve.
///
/// The route index is keyed by `(host, path, method)`, so this is only a
/// conflict where the two scripts' host sets overlap — publishing the same path
/// on two different hosts is the multi-host feature working as intended, not a
/// mistake. A conflict does not stop either script from deploying: one of them
/// simply stops answering, which is exactly why it is worth reporting.
fn route_conflicts(script_uri: &str, registrations: &[CollectedRegistration]) -> Vec<Diagnostic> {
    let claimed: Vec<(&str, &str)> = registrations
        .iter()
        .filter(|registration| registration.kind == RegistrationKind::Route)
        .filter_map(|registration| {
            Some((registration.name.as_str(), registration.method.as_deref()?))
        })
        .collect();

    if claimed.is_empty() {
        return Vec::new();
    }

    let Ok(all_metadata) = repository::get_all_script_metadata() else {
        return Vec::new();
    };

    let mine = all_metadata
        .iter()
        .find(|metadata| metadata.uri == script_uri)
        .map(|metadata| metadata.hosts.as_slice())
        // A candidate for a URI nothing is deployed at yet has no binding
        // stored, and an unbound script publishes on the default host — so
        // resolve the empty binding rather than skipping the check.
        .unwrap_or(&[]);
    let mine = crate::hosts::effective_hosts(mine);

    let mut conflicts = Vec::new();
    for other in &all_metadata {
        if other.uri == script_uri || !other.initialized {
            continue;
        }
        let theirs = crate::hosts::effective_hosts(&other.hosts);
        let Some(shared) = shared_hosts(&mine, &theirs) else {
            continue;
        };

        for (path, method) in other.registrations.keys() {
            if !claimed.iter().any(|(claimed_path, claimed_method)| {
                claimed_path == path && claimed_method == method
            }) {
                continue;
            }
            conflicts.push(Diagnostic::warning(
                script_uri,
                "route-conflict",
                format!(
                    "'{} {}' is already registered by script '{}' on {}. Both cannot answer it; \
                     one of the two will stop serving that path.",
                    method, path, other.uri, shared
                ),
            ));
        }
    }

    conflicts.sort_by(|a, b| a.message.cmp(&b.message));
    conflicts
}

/// Where two scripts' registrations would collide, or `None` when they cannot.
///
/// Mirrors [`crate::hosts::serves_host`]: with host binding unconfigured the
/// engine publishes every script on every host, so any two claims on the same
/// path collide. Reading the empty host list literally instead would report no
/// conflicts at all on exactly the deployments where every conflict is real.
fn shared_hosts(mine: &[String], theirs: &[String]) -> Option<String> {
    if !crate::hosts::is_configured() {
        return Some("every host".to_string());
    }
    let shared: Vec<&str> = mine
        .iter()
        .filter(|host| theirs.contains(host))
        .map(String::as_str)
        .collect();
    if shared.is_empty() {
        None
    } else {
        Some(shared.join(", "))
    }
}

/// A file and line recovered from an engine error message.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceLocation {
    file: String,
    line: u32,
    column: Option<u32>,
}

/// Pull the first `file:line` out of a QuickJS error or stack trace.
///
/// QuickJS reports stack frames as `    at handler (server/routes.ts:42)`, and
/// the transpiler's errors carry `path:line:column`. Both are worth recovering:
/// a diagnostic an editor can jump to is worth more than the same text without
/// a line number. Anything else yields `None` and the caller keeps the script
/// URI, rather than guessing.
fn source_location(script_uri: &str, message: &str) -> Option<SourceLocation> {
    message
        .lines()
        .find_map(|line| parse_location(script_uri, unwrap_frame(line)))
}

/// Reduce one line of an error message to the bare `path:line[:column]` it may
/// contain, stripping the stack-frame decoration around it.
fn unwrap_frame(raw_line: &str) -> &str {
    let candidate = raw_line.trim();
    let candidate = match candidate.strip_prefix("at ") {
        Some(frame) => frame.rsplit_once('(').map_or(frame, |(_, inner)| inner),
        None => candidate,
    };
    candidate.trim_end_matches([')', ',', ';'])
}

fn parse_location(script_uri: &str, candidate: &str) -> Option<SourceLocation> {
    // Split from the right so a `https://` scheme in the script URI is never
    // read as a line number: `https` is followed by `//example.com/...`, which
    // does not parse, and the whole candidate is rejected.
    let (head, last) = candidate.rsplit_once(':')?;
    let last = last.parse::<u32>().ok()?;

    // `path:line:column`
    if let Some((file, line)) = head.rsplit_once(':')
        && let Ok(line) = line.parse::<u32>()
    {
        return Some(SourceLocation {
            file: pick_file(script_uri, file),
            line,
            column: Some(last),
        });
    }

    Some(SourceLocation {
        file: pick_file(script_uri, head),
        line: last,
        column: None,
    })
}

/// Keep a recovered path only when it names something other than the root.
fn pick_file(script_uri: &str, recovered: &str) -> String {
    let recovered = recovered.trim();
    if recovered.is_empty() || recovered == "<eval>" || recovered == "<input>" {
        script_uri.to_string()
    } else {
        recovered.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(path: &str, method: &str, handler: &str) -> CollectedRegistration {
        CollectedRegistration::new(RegistrationKind::Route, path)
            .with_method(method)
            .with_handler(handler)
    }

    #[test]
    fn a_report_is_not_ok_when_any_diagnostic_is_an_error() {
        let mut report = CheckReport::new("s".to_string());
        report
            .diagnostics
            .push(Diagnostic::warning("s", "no-init", "m"));
        assert!(report.clone().finish().ok);

        report
            .diagnostics
            .push(Diagnostic::error("s", "init-failed", "m"));
        let report = report.finish();
        assert!(!report.ok);
        assert_eq!(report.error_count(), 1);
    }

    #[test]
    fn a_missing_handler_names_the_registration_and_the_delegate() {
        let diagnostic = missing_handler_diagnostic(
            "s",
            &MissingHandler {
                kind: RegistrationKind::Route,
                name: "/api/users".to_string(),
                handler: "listUsers".to_string(),
                found_type: None,
            },
        );
        assert_eq!(diagnostic.code, "missing-handler");
        assert!(diagnostic.message.contains("/api/users"));
        assert!(diagnostic.message.contains("listUsers"));
    }

    #[test]
    fn a_handler_that_is_the_wrong_type_says_what_it_is() {
        let diagnostic = missing_handler_diagnostic(
            "s",
            &MissingHandler {
                kind: RegistrationKind::McpTool,
                name: "search".to_string(),
                handler: "config".to_string(),
                found_type: Some("object".to_string()),
            },
        );
        assert!(diagnostic.message.contains("it is a object"));
    }

    #[test]
    fn a_circular_import_says_that_tsc_would_have_accepted_it() {
        let diagnostic = bundle_diagnostic(
            "s",
            &ModuleLoaderError::CircularImport(
                "Circular asset-backed module import: a.ts -> b.ts -> a.ts".to_string(),
            ),
        );
        assert_eq!(diagnostic.code, "circular-import");
        assert!(diagnostic.message.contains("a.ts -> b.ts -> a.ts"));
        assert!(diagnostic.message.contains("tsc"));
    }

    #[test]
    fn a_stack_frame_yields_a_file_and_line() {
        let location = source_location(
            "https://example.com/app",
            "    at init (server/routes.ts:42)",
        )
        .expect("a frame with a line number should resolve");
        assert_eq!(location.file, "server/routes.ts");
        assert_eq!(location.line, 42);
    }

    #[test]
    fn a_message_without_a_line_number_keeps_the_script_uri() {
        assert_eq!(
            source_location(
                "https://example.com/app",
                "init() threw: something went wrong"
            ),
            None
        );
    }

    #[test]
    fn a_scheme_in_the_script_uri_is_not_read_as_a_line_number() {
        assert_eq!(
            source_location("https://example.com/app", "https://example.com/app"),
            None
        );
    }

    #[test]
    fn an_init_that_fits_the_budget_raises_nothing() {
        let mut report = CheckReport::new("s".to_string());
        collect_diagnostics(
            "s",
            &RegistrationPassOutcome {
                pass: crate::js_engine::RegistrationPass {
                    had_init: true,
                    duration_ms: 100,
                    collected: vec![route("/a", "GET", "h")],
                    ..Default::default()
                },
                error: None,
            },
            5_000,
            &mut report,
        );
        assert!(report.diagnostics.is_empty(), "{:?}", report.diagnostics);
    }

    #[test]
    fn an_init_close_to_the_budget_is_reported_before_it_fails() {
        let mut report = CheckReport::new("s".to_string());
        collect_diagnostics(
            "s",
            &RegistrationPassOutcome {
                pass: crate::js_engine::RegistrationPass {
                    had_init: true,
                    duration_ms: 4_000,
                    collected: vec![route("/a", "GET", "h")],
                    ..Default::default()
                },
                error: None,
            },
            5_000,
            &mut report,
        );
        let budget = report
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "init-budget")
            .expect("a run at 80% of budget should be reported");
        assert_eq!(budget.severity, Severity::Warning);
        assert!(budget.message.contains("4000ms"));
    }

    #[test]
    fn a_script_without_an_init_is_reported_as_registering_nothing() {
        let mut report = CheckReport::new("s".to_string());
        collect_diagnostics(
            "s",
            &RegistrationPassOutcome {
                pass: crate::js_engine::RegistrationPass::default(),
                error: None,
            },
            5_000,
            &mut report,
        );
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(report.diagnostics[0].code, "no-init");
    }

    #[test]
    fn an_init_that_registers_nothing_is_distinguished_from_having_no_init() {
        let mut report = CheckReport::new("s".to_string());
        collect_diagnostics(
            "s",
            &RegistrationPassOutcome {
                pass: crate::js_engine::RegistrationPass {
                    had_init: true,
                    duration_ms: 5,
                    ..Default::default()
                },
                error: None,
            },
            5_000,
            &mut report,
        );
        assert_eq!(report.diagnostics.len(), 1);
        assert_eq!(report.diagnostics[0].code, "no-registrations");
    }

    #[test]
    fn a_failed_init_is_not_also_reported_as_registering_nothing() {
        let mut report = CheckReport::new("s".to_string());
        collect_diagnostics(
            "s",
            &RegistrationPassOutcome {
                pass: crate::js_engine::RegistrationPass {
                    had_init: true,
                    duration_ms: 5,
                    ..Default::default()
                },
                error: Some("Init function error: boom".to_string()),
            },
            5_000,
            &mut report,
        );
        let codes: Vec<&str> = report
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect();
        assert_eq!(codes, vec!["init-failed"]);
    }
}
