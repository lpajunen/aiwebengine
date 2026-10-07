//! Checking candidate files, running tests and evaluating snippets.

use super::*;
use crate::repository;
use crate::security::{Capability, UserContext};
use serde::Deserialize;
use serde_json::{Value, json};
use tracing::warn;

/// Why a test run was refused before it started.
pub enum TestRunRefusal {
    NotFound,
    AccessDenied,
}

/// Whether `user` may run `uri`'s tests.
///
/// A run executes the script's own code with the caller's capabilities, so the
/// bar is the one for changing the script: an administrator, or an owner who
/// may write scripts. Anything looser would let someone with read access run
/// arbitrary code as themselves.
pub fn authorize_test_run(user: &UserContext, uri: &str) -> Result<(), TestRunRefusal> {
    if repository::fetch_script(uri).is_none() {
        return Err(TestRunRefusal::NotFound);
    }
    if user.require_capability(&Capability::WriteScripts).is_err() {
        return Err(TestRunRefusal::AccessDenied);
    }
    let is_admin = user.has_capability(&Capability::AdministerEngine);
    if !is_admin && !user_owns_script(user, uri) {
        warn!(
            user_id = ?user.user_id,
            script_name = %uri,
            "Permission denied: only an administrator or owner may run a script's tests"
        );
        return Err(TestRunRefusal::AccessDenied);
    }
    Ok(())
}

/// Why a check was refused before it started.
pub enum CheckRefusal {
    NotFound,
    AccessDenied,
}

/// Whether `user` may check `uri`.
///
/// A check executes the script's own code with the caller's capabilities — the
/// same thing a test run does — so it takes the same bar: an administrator, or
/// an owner who may write scripts. When the caller supplies candidate content
/// there is no deployed script to own, and writing that content is the only
/// thing the check is a preview of, so `WriteScripts` alone is the bar there.
pub fn authorize_check(
    user: &UserContext,
    uri: &str,
    has_candidate: bool,
) -> Result<(), CheckRefusal> {
    let deployed = repository::fetch_script(uri).is_some();
    if !deployed && !has_candidate {
        return Err(CheckRefusal::NotFound);
    }
    if user.require_capability(&Capability::WriteScripts).is_err() {
        return Err(CheckRefusal::AccessDenied);
    }
    if deployed {
        let is_admin = user.has_capability(&Capability::AdministerEngine);
        if !is_admin && !user_owns_script(user, uri) {
            warn!(
                user_id = ?user.user_id,
                script_name = %uri,
                "Permission denied: only an administrator or owner may check a script"
            );
            return Err(CheckRefusal::AccessDenied);
        }
    }
    Ok(())
}

/// One file of a candidate change: source that has not been written anywhere.
#[derive(Deserialize)]
pub struct CandidateFile {
    /// Source text. Not base64, unlike the asset write paths: a module the
    /// bundler can read has to be UTF-8 text anyway, so encoding it would buy
    /// nothing and cost the caller a step.
    pub(super) content: String,
    /// Inferred from the extension when omitted, as it is for a batch write.
    pub(super) mimetype: Option<String>,
}

/// The files of a candidate change, by path. `null` means the change deletes
/// that file.
pub(super) type CandidateFiles = std::collections::BTreeMap<String, Option<CandidateFile>>;

/// Build the view a candidate change describes, over `base`.
///
/// The point of the whole thing: a change that spans modules can be checked
/// while it is still a proposal. `content` alone only ever answered for the
/// root, so a change to a schema module and the three modules that read it had
/// to be written — all of it, to the deployment other people are using —
/// before anything could tell you whether it bundled.
///
/// A `null` entry is a deletion, which is as much a part of a change as a
/// rewrite: a check that quietly kept reading a module the change removes
/// would pass on a program that cannot be built once it lands.
pub(super) fn candidate_overlay(
    files: CandidateFiles,
    base: crate::source_view::SourceView,
) -> Result<(crate::source_view::SourceView, usize), String> {
    use crate::source_view::{OverlayEntry, SourceFile};

    if files.len() > MAX_BATCH_FILES {
        return Err(format!(
            "Too many candidate files: {} (limit {})",
            files.len(),
            MAX_BATCH_FILES
        ));
    }

    let mut total = 0usize;
    let mut entries = std::collections::BTreeMap::new();
    for (path, file) in files {
        validate_asset_uri(&path).map_err(|e| match e {
            AssetWriteError::Validation(message) => message,
            _ => format!("Invalid candidate path '{}'", path),
        })?;

        let entry = match file {
            None => OverlayEntry::Deleted,
            Some(file) => {
                if file.content.len() > MAX_ASSET_BYTES {
                    return Err(format!("Candidate file '{}' is too large", path));
                }
                total = total.saturating_add(file.content.len());
                if total > MAX_BATCH_BYTES {
                    return Err(format!(
                        "Candidate files exceed the {}-byte ceiling",
                        MAX_BATCH_BYTES
                    ));
                }
                let mimetype = file
                    .mimetype
                    .unwrap_or_else(|| mimetype_for(&path).to_string());
                OverlayEntry::Written(SourceFile::text(file.content, mimetype))
            }
        };
        entries.insert(path, entry);
    }

    let count = entries.len();
    Ok((
        crate::source_view::SourceView::overlay_on(base, entries),
        count,
    ))
}

/// Whether `user` may evaluate a snippet against `uri`.
///
/// The same bar as a test run, because it is the same act: caller-authored
/// JavaScript executed in the script's sandbox with the caller's own
/// capabilities. Anything looser would let someone with read access run
/// arbitrary code as themselves.
pub fn authorize_eval(user: &UserContext, uri: &str) -> Result<(), CheckRefusal> {
    if repository::fetch_script(uri).is_none() {
        return Err(CheckRefusal::NotFound);
    }
    if user.require_capability(&Capability::WriteScripts).is_err() {
        return Err(CheckRefusal::AccessDenied);
    }
    let is_admin = user.has_capability(&Capability::AdministerEngine);
    if !is_admin && !user_owns_script(user, uri) {
        warn!(
            user_id = ?user.user_id,
            script_name = %uri,
            "Permission denied: only an administrator or owner may evaluate against a script"
        );
        return Err(CheckRefusal::AccessDenied);
    }
    Ok(())
}

/// Evaluate a snippet and return the same report the REST endpoint serves.
///
/// Runs on the blocking pool, like every native tool — which is also what the
/// isolating transaction needs, being thread-local.
pub(super) fn tool_eval_script(args: &Value, user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let Some(source) = arg_str(args, "source").filter(|source| !source.trim().is_empty()) else {
        return missing_arg("source");
    };

    match authorize_eval(user, uri) {
        Ok(()) => {}
        Err(CheckRefusal::NotFound) => {
            return refuse(Refusal::NotFound, format!("Script not found: {}", uri));
        }
        Err(CheckRefusal::AccessDenied) => {
            return refuse(
                Refusal::Forbidden,
                format!(
                    "Permission denied. You must be an administrator or owner to evaluate against script '{}'",
                    uri
                ),
            );
        }
    }

    let rollback = args
        .get("rollback")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let timeout_ms = args.get("timeoutMs").and_then(Value::as_u64);

    let report = crate::script_eval::eval_blocking(crate::script_eval::EvalRequest {
        timeout_ms,
        rollback,
        ..crate::script_eval::EvalRequest::new(uri.to_string(), source.to_string(), user.clone())
    });

    let mut body = report.to_json();
    if let Some(object) = body.as_object_mut() {
        object.insert("timestamp".to_string(), json!(iso_timestamp()));
    }
    body
}

/// Check a script and return the same report the REST endpoint serves.
///
/// Runs on the blocking pool, like every native tool: a check evaluates the
/// script's program and calls its `init()` under the deploy budget, and the
/// transaction that isolates it is bound to the thread that opens it.
pub(super) fn tool_check_script(args: &Value, user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let content = arg_str(args, "content").map(str::to_string);
    let files: CandidateFiles = match args.get("files") {
        None | Some(Value::Null) => CandidateFiles::default(),
        Some(value) => match serde_json::from_value(value.clone()) {
            Ok(files) => files,
            Err(e) => {
                return refuse(
                    Refusal::BadRequest,
                    format!(
                        "Invalid 'files': expected an object of path -> {{content, mimetype?}} or null: {}",
                        e
                    ),
                );
            }
        },
    };

    let has_candidate = content.is_some()
        || files.iter().any(|(path, candidate)| {
            candidate.is_some() && crate::module_loader::is_root_module_name(path)
        });

    match authorize_check(user, uri, has_candidate) {
        Ok(()) => {}
        Err(CheckRefusal::NotFound) => {
            return json!({
                "status": Refusal::NotFound.status(),
                "error": format!("Script not found: {}", uri),
                "message": "Pass 'content' to check a script that is not deployed yet",
            });
        }
        Err(CheckRefusal::AccessDenied) => {
            return refuse(
                Refusal::Forbidden,
                format!(
                    "Permission denied. You must be an administrator or owner to check script '{}'",
                    uri
                ),
            );
        }
    }

    let rollback = args
        .get("rollback")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let base = match crate::database::run_blocking(resolve_view(uri, arg_str(args, "revision"))) {
        Ok(view) => view,
        Err(message) => return refuse_text(message),
    };

    let (view, candidate_files) = if files.is_empty() {
        (base, 0)
    } else {
        match candidate_overlay(files, base) {
            Ok(built) => built,
            Err(message) => return refuse_text(message),
        }
    };

    // Through the async runner rather than straight to `check_blocking`, so a
    // call over MCP gets the same answer one over HTTP does when `init()` will
    // not stop: the registrations collected before it stalled, rather than only
    // the dispatcher's report that nothing came back. That is the half of the
    // answer worth having — it says how far `init()` got.
    //
    // Bridging back to async from this blocking thread costs a second one while
    // the check runs, and the dispatcher's own backstop bounds how long that
    // can last.
    let report = crate::database::run_blocking(
        crate::script_check::ScriptChecker::with_configured_timeout().run(
            crate::script_check::CheckRequest {
                script_uri: uri.to_string(),
                content,
                rollback,
                timeout_ms: args.get("timeoutMs").and_then(Value::as_u64),
                view,
            },
        ),
    );

    let mut body = report.to_json();
    if let Some(object) = body.as_object_mut() {
        object.insert("candidateFiles".to_string(), json!(candidate_files));
        object.insert("timestamp".to_string(), json!(iso_timestamp()));
    }
    body
}

pub(super) fn tool_run_tests(args: &Value, user: &UserContext) -> Value {
    let Some(uri) = arg_str(args, "script") else {
        return missing_arg("script");
    };

    match authorize_test_run(user, uri) {
        Ok(()) => {}
        Err(TestRunRefusal::NotFound) => {
            return refuse(Refusal::NotFound, format!("Script not found: {}", uri));
        }
        Err(TestRunRefusal::AccessDenied) => {
            return refuse(
                Refusal::Forbidden,
                format!(
                    "Permission denied. You must be an administrator or owner to run tests for script '{}'",
                    uri
                ),
            );
        }
    }

    let filter = arg_str(args, "filter").map(str::to_string);
    // Isolation is the default here as it is over HTTP: a test that writes
    // should not leave rows behind unless the caller says so.
    let rollback = args
        .get("rollback")
        .and_then(Value::as_bool)
        .unwrap_or(true);

    let view = match crate::database::run_blocking(resolve_view(uri, arg_str(args, "revision"))) {
        Ok(view) => view,
        Err(message) => return refuse_text(message),
    };

    let (timeout_ms, run_timeout_ms) = crate::script_test::configured_test_timeouts();
    let modules = crate::module_loader::discover_test_modules_in(uri, &view);
    let result = crate::js_engine::execute_test_run(
        &crate::js_engine::TestRunParams {
            script_uri: uri.to_string(),
            user_context: user.clone(),
            timeout_ms,
            run_timeout_ms,
            filter,
            rollback,
            view,
        },
        &modules,
    );

    let mut report = result.to_json();
    if let Some(object) = report.as_object_mut() {
        object.insert("timestamp".to_string(), json!(iso_timestamp()));
        if result.is_empty() && result.error().is_none() {
            object.insert(
                "message".to_string(),
                json!(
                    "No test modules found. Tests are assets named '*.test.ts' (or .js/.jsx/.tsx)."
                ),
            );
        }
    }
    report
}
