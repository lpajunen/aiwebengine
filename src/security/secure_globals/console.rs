//! `console`, and the request prelude every handler sees.

use super::*;
use crate::repository;
use crate::security::{SecurityEventType, SecuritySeverity};
use rquickjs::{Function, Result as JsResult};
use tracing::debug;

/// The JavaScript half of `console`: joins a variadic call, fills in format
/// specifiers and renders values, so the host binding — which takes one string
/// and throws on anything else — is handed something it accepts.
pub(super) const CONSOLE_PRELUDE: &str = include_str!("../../../assets/console_prelude.js");

/// `Headers`, `URLSearchParams`, and the methods `context.request` gains so a
/// body a script receives reads the way a body it fetched does.
pub(super) const REQUEST_PRELUDE: &str = include_str!("../../../assets/request_prelude.js");

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

impl SecureGlobalContext {
    /// Setup secure logging functions
    pub(super) fn setup_logging_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
    ) -> JsResult<()> {
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
            move |_ctx: rquickjs::Ctx<'_>, message: String, level: String| -> JsResult<()> {
                // Capture before the capability check, not after it. The two
                // are different channels: `ViewLogs` gates writing to the
                // script's stored log, while capture hands the output back to
                // whoever asked for this run — an `/engine/eval_script` caller, or a
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
                    // Dropped, not thrown: `console.log` answers `undefined` and
                    // never fails, as the browser's does. The refusal is in the
                    // audit log above.
                    let _ = e;
                    return Ok(());
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
                Ok(())
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
}
