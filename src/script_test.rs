//! Results of running a script's test modules.
//!
//! A script's tests live in its own assets (`*.test.ts`, see
//! [`crate::module_loader::discover_test_modules`]) and are bundled and
//! executed on demand. This module holds the shape of what comes back: one
//! record per test case plus the run-level outcome, in a form both the REST
//! endpoint and the engine's own tests can consume.

use serde::Serialize;
use std::sync::OnceLock;
use std::time::Duration;
use tracing::{debug, warn};

/// Extra wall-clock time the outer timeout allows beyond the run's own ceiling,
/// so the in-run ceiling is what normally stops a long suite — it reports the
/// modules finished so far, the outer timeout cannot.
const TEST_RUN_TIMEOUT_GRACE_MS: u64 = 5_000;

/// Ceiling on a whole run when configuration does not set one.
pub const DEFAULT_TEST_RUN_TIMEOUT_MS: u64 = 60_000;

/// The per-module and whole-run budgets, set once at startup.
static CONFIGURED_TIMEOUTS: OnceLock<(u64, u64)> = OnceLock::new();

/// Record the test budgets: `(per module, whole run)`. Returns false if they
/// were already set.
pub fn configure_test_timeouts(module_timeout_ms: u64, run_timeout_ms: u64) -> bool {
    CONFIGURED_TIMEOUTS
        .set((module_timeout_ms, run_timeout_ms))
        .is_ok()
}

/// The budgets in effect, falling back to the JavaScript execution limit for a
/// module and [`DEFAULT_TEST_RUN_TIMEOUT_MS`] for a run.
pub fn configured_test_timeouts() -> (u64, u64) {
    CONFIGURED_TIMEOUTS.get().copied().unwrap_or_else(|| {
        (
            crate::js_engine::current_execution_limits().timeout_ms,
            DEFAULT_TEST_RUN_TIMEOUT_MS,
        )
    })
}

/// How a single test case ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TestStatus {
    Passed,
    Failed,
}

impl TestStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            TestStatus::Passed => "passed",
            TestStatus::Failed => "failed",
        }
    }
}

/// One executed test case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TestCaseResult {
    pub name: String,
    /// The test module this case came from, when the run can attribute it.
    ///
    /// A run that bundles every test module into one program cannot say which
    /// file registered which case — the cases arrive as a flat list — so this
    /// is `None` there and `Some` only when each module runs on its own.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    pub status: TestStatus,
    pub duration_ms: u64,
    /// The failure message, including whatever stack the engine could recover.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl TestCaseResult {
    pub fn passed(name: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            name: name.into(),
            file: None,
            status: TestStatus::Passed,
            duration_ms,
            error: None,
        }
    }

    pub fn failed(name: impl Into<String>, error: impl Into<String>, duration_ms: u64) -> Self {
        Self {
            name: name.into(),
            file: None,
            status: TestStatus::Failed,
            duration_ms,
            error: Some(error.into()),
        }
    }

    /// Attribute this case to the test module it was registered from.
    pub fn from_file(mut self, file: impl Into<String>) -> Self {
        self.file = Some(file.into());
        self
    }

    pub fn is_passed(&self) -> bool {
        self.status == TestStatus::Passed
    }
}

/// How the run as a whole ended, independently of the individual cases.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunOutcome {
    /// Every registered case ran to a verdict.
    Completed,
    /// The run hit its time budget partway through. The cases collected before
    /// the interrupt are still reported — that is the point of collecting them
    /// on the Rust side — but the ones after it never ran, so the run cannot be
    /// called a success no matter how the reported cases turned out.
    TimedOut,
    /// The run could not produce verdicts at all: the test modules failed to
    /// bundle, or evaluating the bundle threw before any case could register.
    Failed(String),
}

/// Everything one `run tests` request produced for one script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestRunResult {
    pub script_uri: String,
    pub cases: Vec<TestCaseResult>,
    pub duration_ms: u64,
    pub outcome: RunOutcome,
}

impl TestRunResult {
    /// A run that reached a verdict for every case it registered.
    pub fn completed(
        script_uri: impl Into<String>,
        cases: Vec<TestCaseResult>,
        duration_ms: u64,
    ) -> Self {
        Self {
            script_uri: script_uri.into(),
            cases,
            duration_ms,
            outcome: RunOutcome::Completed,
        }
    }

    /// A run cut short by its time budget, reporting the cases it did finish.
    pub fn timed_out(
        script_uri: impl Into<String>,
        cases: Vec<TestCaseResult>,
        duration_ms: u64,
    ) -> Self {
        Self {
            script_uri: script_uri.into(),
            cases,
            duration_ms,
            outcome: RunOutcome::TimedOut,
        }
    }

    /// A run that never got as far as executing cases.
    pub fn failed(
        script_uri: impl Into<String>,
        error: impl Into<String>,
        duration_ms: u64,
    ) -> Self {
        Self {
            script_uri: script_uri.into(),
            cases: Vec::new(),
            duration_ms,
            outcome: RunOutcome::Failed(error.into()),
        }
    }

    pub fn passed_count(&self) -> usize {
        self.cases.iter().filter(|case| case.is_passed()).count()
    }

    pub fn failed_count(&self) -> usize {
        self.cases.len() - self.passed_count()
    }

    /// Whether the run is a green light. A script with no test modules reports
    /// no cases, which is *not* a pass — callers that care about the difference
    /// check [`TestRunResult::is_empty`] first, so "I wrote no tests" can never
    /// read as "my tests pass".
    pub fn all_passed(&self) -> bool {
        self.outcome == RunOutcome::Completed && !self.cases.is_empty() && self.failed_count() == 0
    }

    pub fn is_empty(&self) -> bool {
        self.cases.is_empty()
    }

    /// The run-level error, if the run could not produce verdicts.
    pub fn error(&self) -> Option<&str> {
        match &self.outcome {
            RunOutcome::Failed(message) => Some(message),
            _ => None,
        }
    }

    /// The report as the REST endpoint and MCP tool serve it.
    pub fn to_json(&self) -> serde_json::Value {
        let cases = serde_json::to_value(&self.cases).unwrap_or(serde_json::Value::Null);

        let mut report = serde_json::json!({
            "scriptUri": self.script_uri,
            "success": self.all_passed(),
            "total": self.cases.len(),
            "passed": self.passed_count(),
            "failed": self.failed_count(),
            "durationMs": self.duration_ms,
            "timedOut": self.outcome == RunOutcome::TimedOut,
            "cases": cases,
        });

        if let Some(error) = self.error()
            && let Some(object) = report.as_object_mut()
        {
            object.insert("error".to_string(), serde_json::json!(error));
        }

        report
    }
}

/// What a caller asks for when running a script's tests.
#[derive(Debug, Clone)]
pub struct TestRunRequest {
    pub script_uri: String,
    pub user_context: crate::security::UserContext,
    /// Run only the cases whose name contains this substring.
    pub filter: Option<String>,
    /// Roll back the database writes the tests make.
    pub rollback: bool,
}

/// Runs a script's test modules off the async runtime and within a budget.
pub struct TestRunner {
    module_timeout_ms: u64,
    run_timeout_ms: u64,
}

impl TestRunner {
    pub fn new(module_timeout_ms: u64, run_timeout_ms: u64) -> Self {
        Self {
            module_timeout_ms,
            run_timeout_ms,
        }
    }

    /// A runner using the budgets configured at startup.
    pub fn with_configured_timeouts() -> Self {
        let (module_timeout_ms, run_timeout_ms) = configured_test_timeouts();
        Self::new(module_timeout_ms, run_timeout_ms)
    }

    /// Discover and run `script_uri`'s test modules.
    ///
    /// Two budgets bound this call: the run ceiling the blocking loop enforces
    /// itself, and the outer one here. The inner ceiling is the better of the
    /// two — it returns the modules that finished, whereas expiring here
    /// abandons the blocking task and every verdict with it. So the outer
    /// budget gets a grace period and serves only as the backstop the interrupt
    /// cannot cover: JavaScript blocked in a host call, where no bytecode runs
    /// for the handler to interrupt.
    pub async fn run(&self, request: TestRunRequest) -> TestRunResult {
        let started = std::time::Instant::now();
        let script_uri = request.script_uri.clone();

        let params = crate::js_engine::TestRunParams {
            script_uri: request.script_uri,
            user_context: request.user_context,
            timeout_ms: self.module_timeout_ms,
            run_timeout_ms: self.run_timeout_ms,
            filter: request.filter,
            rollback: request.rollback,
        };

        let backstop = Duration::from_millis(
            self.run_timeout_ms
                .saturating_add(TEST_RUN_TIMEOUT_GRACE_MS),
        );

        let outcome = tokio::time::timeout(
            backstop,
            tokio::task::spawn_blocking(move || {
                let modules = crate::module_loader::discover_test_modules(&params.script_uri);
                debug!(
                    script_uri = %params.script_uri,
                    modules = modules.len(),
                    "Running script tests"
                );
                crate::js_engine::execute_test_run(&params, &modules)
            }),
        )
        .await;

        let duration_ms = started.elapsed().as_millis() as u64;

        match outcome {
            Ok(Ok(result)) => result,
            Ok(Err(join_error)) => {
                warn!(script_uri = %script_uri, "Test run task failed: {}", join_error);
                TestRunResult::failed(
                    script_uri,
                    format!("Test run task failed: {}", join_error),
                    duration_ms,
                )
            }
            Err(_elapsed) => {
                // The backstop fired: the blocking task is abandoned mid-run,
                // so there are no verdicts to recover here.
                warn!(
                    script_uri = %script_uri,
                    "Test run exceeded its {}ms backstop (blocked in a host call?)",
                    backstop.as_millis()
                );
                TestRunResult::failed(
                    script_uri,
                    format!(
                        "Test run timeout ({}ms + {}ms grace)",
                        self.run_timeout_ms, TEST_RUN_TIMEOUT_GRACE_MS
                    ),
                    duration_ms,
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_cases() -> Vec<TestCaseResult> {
        vec![
            TestCaseResult::passed("totals an empty basket", 3),
            TestCaseResult::failed("rejects negative quantities", "Expected 0, got -1", 7),
        ]
    }

    #[test]
    fn counts_split_passed_and_failed_cases() {
        let result = TestRunResult::completed("myapp", sample_cases(), 12);

        assert_eq!(result.passed_count(), 1);
        assert_eq!(result.failed_count(), 1);
        assert!(!result.all_passed());
    }

    #[test]
    fn a_completed_run_with_only_passing_cases_is_a_pass() {
        let result = TestRunResult::completed(
            "myapp",
            vec![TestCaseResult::passed("totals an empty basket", 3)],
            3,
        );

        assert!(result.all_passed());
        assert_eq!(result.failed_count(), 0);
        assert!(result.error().is_none());
    }

    #[test]
    fn a_run_with_no_cases_is_not_a_pass() {
        let result = TestRunResult::completed("myapp", Vec::new(), 1);

        assert!(result.is_empty());
        assert!(!result.all_passed());
    }

    #[test]
    fn a_timed_out_run_reports_its_finished_cases_but_never_passes() {
        let result = TestRunResult::timed_out(
            "myapp",
            vec![TestCaseResult::passed("totals an empty basket", 3)],
            5_000,
        );

        assert_eq!(result.passed_count(), 1);
        assert!(!result.all_passed());
        assert_eq!(result.to_json()["timedOut"], serde_json::json!(true));
    }

    #[test]
    fn a_failed_run_carries_its_error_and_no_cases() {
        let result = TestRunResult::failed("myapp", "Transpilation error: unexpected token", 4);

        assert!(result.is_empty());
        assert!(!result.all_passed());
        assert_eq!(
            result.error(),
            Some("Transpilation error: unexpected token")
        );
        assert_eq!(
            result.to_json()["error"],
            serde_json::json!("Transpilation error: unexpected token")
        );
    }

    #[test]
    fn report_json_has_the_documented_shape() {
        let result = TestRunResult::completed(
            "myapp",
            vec![
                TestCaseResult::passed("totals an empty basket", 3)
                    .from_file("tests/orders.test.ts"),
                TestCaseResult::failed("rejects negative quantities", "Expected 0, got -1", 7),
            ],
            12,
        );

        let report = result.to_json();

        assert_eq!(report["scriptUri"], serde_json::json!("myapp"));
        assert_eq!(report["success"], serde_json::json!(false));
        assert_eq!(report["total"], serde_json::json!(2));
        assert_eq!(report["passed"], serde_json::json!(1));
        assert_eq!(report["failed"], serde_json::json!(1));
        assert_eq!(report["durationMs"], serde_json::json!(12));
        assert!(report.get("error").is_none());

        let cases = report["cases"]
            .as_array()
            .expect("cases should serialize as an array");
        assert_eq!(cases.len(), 2);
        assert_eq!(
            cases[0]["name"],
            serde_json::json!("totals an empty basket")
        );
        assert_eq!(cases[0]["status"], serde_json::json!("passed"));
        assert_eq!(cases[0]["file"], serde_json::json!("tests/orders.test.ts"));
        assert_eq!(cases[0]["durationMs"], serde_json::json!(3));
        assert!(
            cases[0].get("error").is_none(),
            "a passing case carries no error field"
        );
        assert_eq!(cases[1]["status"], serde_json::json!("failed"));
        assert_eq!(cases[1]["error"], serde_json::json!("Expected 0, got -1"));
        assert!(
            cases[1].get("file").is_none(),
            "an unattributed case omits the file field"
        );
    }
}
