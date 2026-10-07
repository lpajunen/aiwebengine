//! `audit`: events a script records and cannot take back.

use super::*;
use rquickjs::{Function, Result as JsResult};
use tracing::debug;

pub(super) const AUDIT_PRELUDE: &str = include_str!("../../../assets/audit_prelude.js");

impl SecureGlobalContext {
    /// `audit` — events a script records and cannot take back.
    ///
    /// The script says what happened; the engine says who, from this
    /// execution's principal and the address the edge judged, so an event
    /// cannot be attributed to anybody else. See [`crate::script_audit`].
    pub(super) fn setup_audit_object(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
    ) -> JsResult<()> {
        let global = ctx.globals();
        let host = rquickjs::Object::new(ctx.clone())?;

        let script = script_uri.to_string();
        let actor_kind = self.config.principal.kind();
        let actor_id = self.user_context.user_id.clone();
        let client_ip = self.config.client_ip.clone();
        let request_id = self.config.log_context.request_id.clone();
        let record = Function::new(
            ctx.clone(),
            move |action: String, details_json: String| -> JsResult<i64> {
                let refuse = |message: String| {
                    rquickjs::Error::new_from_js_message(
                        "audit.record",
                        "range_error",
                        &format!("audit.record: {}", message),
                    )
                };
                let details: Option<serde_json::Value> = if details_json.is_empty() {
                    None
                } else {
                    Some(
                        serde_json::from_str(&details_json)
                            .map_err(|e| refuse(format!("details are not JSON: {}", e)))?,
                    )
                };
                if let Some(reason) = crate::script_audit::refusal(&action, details.as_ref()) {
                    return Err(refuse(reason));
                }

                crate::database::run_blocking(crate::script_audit::record(
                    crate::script_audit::NewAuditEvent {
                        script_uri: script.clone(),
                        action,
                        details,
                        actor_kind,
                        actor_id: actor_id.clone(),
                        client_ip: client_ip.clone(),
                        request_id: request_id.clone(),
                    },
                ))
                .map_err(|e| {
                    rquickjs::Error::new_from_js_message(
                        "audit.record",
                        "storage_error",
                        &format!("audit.record: the event could not be stored: {}", e),
                    )
                })
            },
        )?;

        host.set("record", record)?;
        global.set("__hostAudit", host)?;

        crate::bytecode::eval_program(ctx, "engine://audit-prelude", AUDIT_PRELUDE).map_err(
            |e| {
                rquickjs::Error::new_from_js_message(
                    "audit",
                    "prelude",
                    &format!("audit prelude failed to load: {}", e),
                )
            },
        )?;

        debug!("audit initialized for script: {}", script_uri);
        Ok(())
    }
}
