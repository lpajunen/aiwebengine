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
use std::time::Duration;

use crate::js_engine::{DryRunParams, MissingHandler, RegistrationPassOutcome};
use crate::module_loader::ModuleLoaderError;
use crate::repository;
use crate::security::secure_globals::{CollectedRegistration, RegistrationKind, RegistrationSink};

/// Fraction of the `init()` budget a script may spend before the check says so.
///
/// Deliberately well below 1.0: a run that fits today at 95% of budget is one
/// cold cache or one slow query from failing the next deploy, and the failure
/// mode is a script that comes up with no routes. The margin is what makes the
/// warning actionable while the script still works.
const INIT_BUDGET_WARN_RATIO: f64 = 0.7;

/// How much longer than the deploy budget a check lets `init()` run.
///
/// A check *measures* the init cost; it does not enforce it. Capping the run at
/// the deploy budget makes the one script that most needs an answer — the one
/// that is over budget — the one that cannot get it: the run is interrupted at
/// the ceiling, and all the report can say is "interrupted", never "took 12s,
/// which is 2.5x the budget". Headroom buys the measurement, and the deploy
/// budget is still what the verdict is stated against.
const INIT_HEADROOM_MULTIPLE: u32 = 4;

/// Hard ceiling on one check, however large the budget or the caller's request.
///
/// A check holds a blocking thread for its whole duration, so this bounds what
/// one caller can occupy. Reached only by an `init()` that is already far past
/// anything deployable.
pub const MAX_CHECK_TIMEOUT_MS: u64 = 60_000;

/// The ceiling to run `init()` under, given the deploy budget and whatever the
/// caller asked for.
pub fn check_timeout_ms(budget_ms: u64, requested_ms: Option<u64>) -> u64 {
    let default = budget_ms.saturating_mul(INIT_HEADROOM_MULTIPLE as u64);
    requested_ms
        .unwrap_or(default)
        // Never below the budget itself: a ceiling under the budget would
        // report a script as over-budget that a deploy would have accepted.
        .clamp(budget_ms.max(1), MAX_CHECK_TIMEOUT_MS.max(budget_ms))
}

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
    /// The budget a real deploy enforces (`javascript.init_timeout_ms`). The
    /// verdict is stated against this, whatever ceiling the run itself used.
    pub budget_ms: u64,
    /// The ceiling this run was given — headroom above the budget, so that a
    /// script over budget still produces a measurement instead of a timeout.
    pub ceiling_ms: u64,
    /// True when even the ceiling was not enough and the run was stopped, in
    /// which case `duration_ms` is where it was cut off rather than what it
    /// costs.
    pub timed_out: bool,
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
    /// Ceiling for the `init()` run, clamped by [`check_timeout_ms`]. Raise it
    /// when a script's `init()` is slow enough that even the default headroom
    /// cannot measure it.
    pub timeout_ms: Option<u64>,
    /// Candidate source to check instead of what is deployed.
    ///
    /// The point of the whole endpoint: an agent can check the code it is about
    /// to write, rather than deploying it and reading the FATAL afterwards.
    pub content: Option<String>,
    /// Roll back the database writes `init()` makes. On by default.
    pub rollback: bool,
    /// Which version of the script's files the imports resolve against.
    ///
    /// Defaults to what is deployed. Pointed at a revision, it checks that
    /// revision — including its own modules, so a candidate root is checked
    /// against the tree it was written for rather than against head.
    pub view: crate::source_view::SourceView,
}

/// Extra wall-clock time the outer timeout allows beyond the run's own ceiling.
///
/// The in-run interrupt is the better of the two stops — it leaves the pass to
/// finish reporting — so it gets first refusal, and this backstop covers only
/// what it cannot: JavaScript blocked in a *host* call, a `fetch` or a query,
/// where no bytecode executes for the interrupt handler to run between.
const CHECK_BACKSTOP_GRACE_MS: u64 = 5_000;

fn new_sink() -> RegistrationSink {
    std::sync::Arc::new(std::sync::Mutex::new(Vec::new()))
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
        let budget_ms = self.init_timeout_ms;
        let ceiling_ms = check_timeout_ms(budget_ms, request.timeout_ms);

        // The sink is made here and shared with the run, so that a run this
        // timeout gives up on can still be reported from what it collected.
        let sink = new_sink();
        let sink_for_run = std::sync::Arc::clone(&sink);

        let started = std::time::Instant::now();
        let backstop = Duration::from_millis(ceiling_ms.saturating_add(CHECK_BACKSTOP_GRACE_MS));
        let checked = tokio::time::timeout(
            backstop,
            tokio::task::spawn_blocking(move || {
                check_blocking_into(request, budget_ms, sink_for_run)
            }),
        )
        .await;

        match checked {
            Ok(Ok(report)) => report,
            Ok(Err(join_error)) => {
                let mut report = CheckReport::new(script_uri.clone());
                report.diagnostics.push(Diagnostic::error(
                    script_uri,
                    "check-failed",
                    format!("The check could not be completed: {}", join_error),
                ));
                report.finish()
            }
            Err(_) => abandoned_report(
                script_uri,
                budget_ms,
                ceiling_ms,
                started.elapsed().as_millis() as u64,
                &sink,
            ),
        }
    }
}

/// What to report when the backstop fired and the run was left behind.
///
/// The blocking task is still out there — nothing can cancel a thread parked in
/// a host call — but its registrations were being written to a sink this side
/// still holds, so the answer is the partial one rather than no answer at all.
/// Reporting that is the whole point: a script whose `init()` blocks is
/// precisely the script whose author needs to know which registrations it got
/// through before it stalled.
fn abandoned_report(
    script_uri: String,
    budget_ms: u64,
    ceiling_ms: u64,
    // How long the run actually got before being abandoned: the ceiling plus
    // the backstop's grace, not the ceiling alone — the ceiling is the point it
    // *failed* to stop at.
    waited_ms: u64,
    sink: &RegistrationSink,
) -> CheckReport {
    let collected = sink
        .lock()
        .ok()
        .map(|collected| collected.clone())
        .unwrap_or_default();

    let mut report = CheckReport::new(script_uri.clone());
    report.diagnostics.push(Diagnostic::error(
        script_uri,
        "init-blocked",
        format!(
            "init() was still running after {}ms and did not respond to being stopped, which \
            means it is blocked in a host call — a fetch, a database query, an MCP call — rather \
            than in JavaScript. The engine can only interrupt JavaScript, so a deploy would hit \
            the same wall at its {}ms budget and bring the script up with no route table. The {} \
            registration(s) listed here are the ones it made before it stalled. Look for an \
            unbounded call in init(): a request with no timeout, or a query without a limit.",
            ceiling_ms,
            budget_ms,
            collected.len()
        ),
    ));
    report.init = Some(InitReport {
        ran: true,
        duration_ms: waited_ms,
        budget_ms,
        ceiling_ms,
        timed_out: true,
    });
    report.registrations = collected;
    report.finish()
}

/// Run a check on the current thread.
///
/// One function on one thread because its steps share state bound to it — the
/// transaction isolating the run is thread-local, and the QuickJS context
/// holding the answers about delegates lives only as long as the run. Callers
/// on an async task go through [`ScriptChecker::run`], which moves this to the
/// blocking pool; the MCP dispatcher is already there and calls it directly.
pub fn check_blocking(request: CheckRequest, init_budget_ms: u64) -> CheckReport {
    let sink = new_sink();
    check_blocking_into(request, init_budget_ms, sink)
}

/// [`check_blocking`], recording registrations into a sink the caller keeps.
///
/// Only [`ScriptChecker::run`] needs this: it holds the other end so that a run
/// its outer timeout gives up on can still be reported from what the abandoned
/// thread collected before it stopped.
fn check_blocking_into(
    request: CheckRequest,
    init_budget_ms: u64,
    sink: RegistrationSink,
) -> CheckReport {
    let CheckRequest {
        script_uri,
        content,
        rollback,
        timeout_ms,
        view,
    } = request;

    let ceiling_ms = check_timeout_ms(init_budget_ms, timeout_ms);
    let mut report = CheckReport::new(script_uri.clone());

    let checking_candidate = content.is_some();
    // The root comes from the same view as the modules unless the caller
    // supplied one: checking a revision means checking what it held, and
    // reading its root from the live rows would splice two versions together.
    let content = match content.or_else(|| view.root_content(&script_uri)) {
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
    if let Err(error) =
        crate::module_loader::prepare_executable_program_in(&script_uri, &content, &view)
    {
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
        timeout_ms: ceiling_ms,
        rollback,
        sink,
    });

    if checking_candidate {
        // The candidate's bundle is now cached under this script's key. It is
        // validated by a hash of the root content, so a live request would
        // rebuild rather than serve it — but leaving an entry that can only ever
        // miss is worse than leaving none.
        crate::module_loader::invalidate_program(&script_uri);
    }

    collect_diagnostics(
        &script_uri,
        &outcome,
        init_budget_ms,
        ceiling_ms,
        &mut report,
    );
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
    ceiling_ms: u64,
    report: &mut CheckReport,
) {
    let pass = &outcome.pass;

    report.init = Some(InitReport {
        ran: pass.had_init,
        duration_ms: pass.duration_ms,
        budget_ms,
        ceiling_ms,
        timed_out: pass.timed_out,
    });

    if pass.timed_out {
        // The run hit the check's own ceiling, which is already headroom above
        // the deploy budget — so there is no measurement to report, only the
        // fact that there isn't one, and what was salvaged on the way.
        report.diagnostics.push(Diagnostic::error(
            script_uri,
            "init-timeout",
            format!(
                "init() was still running after {}ms and was stopped, so its cost could not be \
                measured. The deploy budget is {}ms, so this script would come up with only the \
                {} registration(s) listed here — the ones it made before it stopped. Move the \
                slow work out of init(), or raise the check's ceiling with timeout_ms (max {}ms) \
                to find out how long it really takes.",
                ceiling_ms,
                budget_ms,
                pass.collected.len(),
                MAX_CHECK_TIMEOUT_MS
            ),
        ));
    } else if let Some(error) = &outcome.error {
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

    // A run cut short measured nothing, so there is no cost to report against
    // the budget — `init-timeout` above has already said so.
    if pass.had_init && outcome.error.is_none() {
        report
            .diagnostics
            .extend(budget_diagnostic(script_uri, pass.duration_ms, budget_ms));
    }
}

/// What the measured `init()` cost means for a deploy.
///
/// Over the budget is an error, not a warning: the deploy will interrupt
/// `init()` there and the script comes up with whatever subset of its routes it
/// had registered by then. The check can say this precisely *because* it runs
/// with headroom — under the budget as a ceiling, this case is unreachable and
/// shows up as a timeout instead.
fn budget_diagnostic(script_uri: &str, duration_ms: u64, budget_ms: u64) -> Option<Diagnostic> {
    let spent = duration_ms as f64 / budget_ms.max(1) as f64;

    if duration_ms >= budget_ms {
        return Some(Diagnostic::error(
            script_uri,
            "init-budget",
            format!(
                "init() took {}ms, over the {}ms deploy budget ({:.1}x). A deploy interrupts \
                 init() at the budget, so this script would come up with only the registrations \
                 it had made by then. Move the slow work out of init() — into a scheduled job, or \
                 behind the first request that needs it.",
                duration_ms, budget_ms, spent
            ),
        ));
    }

    if spent >= INIT_BUDGET_WARN_RATIO {
        return Some(Diagnostic::warning(
            script_uri,
            "init-budget",
            format!(
                "init() took {}ms of the {}ms deploy budget ({:.0}%). Exceeding it brings the \
                 script up with only the registrations init() made before the interrupt. Move \
                 slow work out of init() — into a scheduled job, or behind the first request \
                 that needs it.",
                duration_ms,
                budget_ms,
                spent * 100.0
            ),
        ));
    }

    None
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
            check_timeout_ms(5_000, None),
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
            check_timeout_ms(5_000, None),
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
    fn an_init_over_the_budget_is_an_error_not_a_warning() {
        let mut report = CheckReport::new("s".to_string());
        collect_diagnostics(
            "s",
            &RegistrationPassOutcome {
                pass: crate::js_engine::RegistrationPass {
                    had_init: true,
                    // Measurable only because the run had headroom above the
                    // 5s budget; under the budget as a ceiling this is a
                    // timeout instead, and says nothing about how far over.
                    duration_ms: 12_400,
                    collected: vec![route("/a", "GET", "h")],
                    ..Default::default()
                },
                error: None,
            },
            5_000,
            check_timeout_ms(5_000, None),
            &mut report,
        );

        let budget = report
            .diagnostics
            .iter()
            .find(|diagnostic| diagnostic.code == "init-budget")
            .expect("an over-budget run should be reported");
        assert_eq!(budget.severity, Severity::Error);
        assert!(budget.message.contains("12400ms"), "{}", budget.message);
        assert!(budget.message.contains("2.5x"), "{}", budget.message);
    }

    #[test]
    fn a_run_that_hit_the_ceiling_reports_a_timeout_rather_than_a_measurement() {
        let mut report = CheckReport::new("s".to_string());
        collect_diagnostics(
            "s",
            &RegistrationPassOutcome {
                pass: crate::js_engine::RegistrationPass {
                    had_init: true,
                    duration_ms: 20_000,
                    timed_out: true,
                    collected: vec![route("/a", "GET", "h")],
                    ..Default::default()
                },
                error: Some("Init function error: interrupted".to_string()),
            },
            5_000,
            20_000,
            &mut report,
        );

        let codes: Vec<&str> = report
            .diagnostics
            .iter()
            .map(|diagnostic| diagnostic.code)
            .collect();
        // Not the bare "interrupted" the runtime reports, and not a budget
        // measurement either — nothing was measured.
        assert_eq!(codes, vec!["init-timeout"]);
        assert!(report.diagnostics[0].message.contains("timeout_ms"));
        assert!(
            report.diagnostics[0].message.contains("1 registration"),
            "{}",
            report.diagnostics[0].message
        );
    }

    /// Multi-line message literals are joined with `\` continuations, which are
    /// easy to lose in an edit — and losing one leaves a run of indentation
    /// embedded in prose a user reads. Cheap to assert, invisible otherwise.
    #[test]
    fn no_diagnostic_message_carries_leftover_indentation() {
        let messages = [
            missing_handler_diagnostic(
                "s",
                &MissingHandler {
                    kind: RegistrationKind::Route,
                    name: "/a".to_string(),
                    handler: "h".to_string(),
                    found_type: Some("object".to_string()),
                },
            )
            .message,
            bundle_diagnostic(
                "s",
                &ModuleLoaderError::CircularImport("a -> b -> a".to_string()),
            )
            .message,
            budget_diagnostic("s", 12_400, 5_000)
                .expect("over budget")
                .message,
            budget_diagnostic("s", 4_000, 5_000)
                .expect("near budget")
                .message,
        ];

        for message in messages {
            assert!(
                !message.contains("  "),
                "a lost line continuation left indentation in: {:?}",
                message
            );
        }
    }

    #[test]
    fn the_ceiling_gives_headroom_and_is_bounded_at_both_ends() {
        // Headroom by default, so an over-budget init() is measured.
        assert!(check_timeout_ms(5_000, None) > 5_000);
        // Never below the budget: a ceiling under it would fail scripts a
        // deploy would have accepted.
        assert_eq!(check_timeout_ms(5_000, Some(10)), 5_000);
        // And bounded, since a check holds a blocking thread throughout.
        assert_eq!(
            check_timeout_ms(5_000, Some(u64::MAX)),
            MAX_CHECK_TIMEOUT_MS
        );
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
            check_timeout_ms(5_000, None),
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
            check_timeout_ms(5_000, None),
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
            check_timeout_ms(5_000, None),
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
