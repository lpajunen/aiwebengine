//! Running an ad hoc snippet against a deployed script's sandbox.
//!
//! The gap this closes: answering "what is actually in that table" or "what
//! does this helper return for that input" used to mean authoring a test file,
//! deploying it, running the suite, reading the answer out of an assertion
//! message, and deleting the file again. The engine could already run
//! caller-authored code in a script's sandbox — that is what
//! [`crate::script_test`] does — so what was missing was a way to hand it one
//! expression and get the value back.
//!
//! An evaluation adds no authority. It runs with the *caller's*
//! [`UserContext`], exactly as a test run does, so it can reach nothing the
//! caller could not already reach by writing a test. That is the whole security
//! argument, and it is why the authorization here is the same bar as a test run
//! rather than something stricter.

use serde::Serialize;
use serde_json::{Value, json};

use crate::js_engine::{EvalOutcome, EvalParams};
use crate::security::UserContext;

/// Ceiling on a single evaluation when the caller does not ask for one, and the
/// cap their request is clamped to.
///
/// Clamping matters: the budget arms the interrupt that stops a runaway
/// snippet, and it is held by a blocking thread for its whole duration. An
/// unclamped `timeout_ms` would be a one-parameter way to pin those threads.
pub fn default_eval_timeout_ms() -> u64 {
    crate::js_engine::current_execution_limits().timeout_ms
}

/// What to evaluate.
pub struct EvalRequest {
    pub script_uri: String,
    pub source: String,
    pub user_context: UserContext,
    /// Requested budget. Clamped to [`default_eval_timeout_ms`].
    pub timeout_ms: Option<u64>,
    /// Roll back the database writes the snippet makes. On by default.
    pub rollback: bool,
}

/// Everything one evaluation produced, plus the script it ran against.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EvalReport {
    pub script_uri: String,
    /// False when the snippet threw, timed out, or could not be reached at all
    /// because the script itself would not load.
    pub ok: bool,
    #[serde(flatten)]
    pub outcome: EvalOutcome,
}

impl EvalReport {
    pub fn to_json(&self) -> Value {
        serde_json::to_value(self).unwrap_or_else(|e| {
            json!({
                "scriptUri": self.script_uri,
                "ok": false,
                "error": format!("Failed to serialize evaluation report: {}", e),
            })
        })
    }
}

/// Evaluate on the current thread.
///
/// One thread throughout, because the transaction isolating the run is
/// thread-local. Callers on an async task go through [`ScriptEvaluator::run`],
/// which moves this to the blocking pool; the MCP dispatcher is already there
/// and calls it directly.
pub fn eval_blocking(request: EvalRequest) -> EvalReport {
    let budget = request
        .timeout_ms
        .unwrap_or_else(default_eval_timeout_ms)
        .clamp(1, default_eval_timeout_ms());

    let outcome = crate::js_engine::evaluate_snippet(&EvalParams {
        script_uri: request.script_uri.clone(),
        source: request.source,
        user_context: request.user_context,
        timeout_ms: budget,
        rollback: request.rollback,
    });

    EvalReport {
        script_uri: request.script_uri,
        ok: outcome.error.is_none(),
        outcome,
    }
}

/// Runs evaluations off the async runtime.
pub struct ScriptEvaluator;

impl ScriptEvaluator {
    pub async fn run(request: EvalRequest) -> EvalReport {
        let script_uri = request.script_uri.clone();

        match tokio::task::spawn_blocking(move || eval_blocking(request)).await {
            Ok(report) => report,
            Err(join_error) => EvalReport {
                script_uri,
                ok: false,
                outcome: EvalOutcome {
                    error: Some(format!(
                        "The evaluation could not be completed: {}",
                        join_error
                    )),
                    ..EvalOutcome::default()
                },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_report_carries_the_outcome_at_the_top_level() {
        let report = EvalReport {
            script_uri: "s".to_string(),
            ok: true,
            outcome: EvalOutcome {
                value: Some(json!(41)),
                value_type: Some("number".to_string()),
                duration_ms: 3,
                ..EvalOutcome::default()
            },
        };

        let body = report.to_json();
        assert_eq!(body["scriptUri"], json!("s"));
        assert_eq!(body["ok"], json!(true));
        // Flattened, so callers read `value` rather than `outcome.value`.
        assert_eq!(body["value"], json!(41));
        assert_eq!(body["valueType"], json!("number"));
        assert_eq!(body["durationMs"], json!(3));
    }

    #[test]
    fn an_absent_value_is_omitted_rather_than_reported_as_null() {
        let report = EvalReport {
            script_uri: "s".to_string(),
            ok: true,
            outcome: EvalOutcome {
                value: None,
                value_type: Some("undefined".to_string()),
                ..EvalOutcome::default()
            },
        };

        let body = report.to_json();
        assert!(
            body.get("value").is_none(),
            "an undefined result must not be reported as a null one: {}",
            body
        );
        assert_eq!(body["valueType"], json!("undefined"));
    }
}
