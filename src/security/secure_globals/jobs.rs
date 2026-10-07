//! `schedulerService`: jobs declared in `init()`.

use super::*;
use crate::scheduler;
use chrono::Duration as ChronoDuration;
use rquickjs::{Function, Result as JsResult};

/// Builds `schedulerService` over `__hostScheduler`.
pub(super) const SCHEDULER_PRELUDE: &str = include_str!("../../../assets/scheduler_prelude.js");

impl SecureGlobalContext {
    pub(super) fn setup_scheduler_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
    ) -> JsResult<()> {
        // `__hostScheduler` answers in the envelope `scheduler_prelude.js`
        // unwraps, and the prelude builds `schedulerService` from it. The
        // object is present in every context; registering is what depends on
        // the phase, and outside it a registration is refused as a value —
        // `{ ok: false, reason }` — exactly as `routeRegistry.registerRoute`'s
        // is, since top-level code runs on every execution and must not throw.
        let host = rquickjs::Object::new(ctx.clone())?;
        let scheduler_handle = scheduler::get_scheduler();

        /// Read `handler` off a registration's options, or say what is wrong.
        fn handler_of(options: &rquickjs::Object<'_>, api: &str) -> Result<String, String> {
            let handler: String = options
                .get("handler")
                .map_err(|_| format!("{}: options.handler is required", api))?;
            let handler = handler.trim();
            if handler.is_empty() {
                return Err(format!("{}: options.handler must name a function", api));
            }
            Ok(handler.to_string())
        }

        fn refused(reason: String) -> String {
            host_ok(serde_json::json!({ "ok": false, "reason": reason }))
        }

        let register_once_handle = scheduler_handle.clone();
        let script_uri_once = script_uri.to_string();
        let config_once = self.config.clone();
        let register_once = Function::new(
            ctx.clone(),
            move |options: rquickjs::Object<'_>| -> String {
                const API: &str = "schedulerService.registerOnce";
                let handler_name = match handler_of(&options, API) {
                    Ok(handler) => handler,
                    Err(message) => return host_failure("TypeError", &message),
                };
                let run_at_value: String = match options.get("runAt") {
                    Ok(value) => value,
                    Err(_) => {
                        return host_failure(
                            "TypeError",
                            &format!("{}: options.runAt is required (a UTC ISO string)", API),
                        );
                    }
                };
                let run_at = match scheduler::parse_utc_timestamp(&run_at_value) {
                    Ok(ts) => ts,
                    Err(err) => return host_failure("TypeError", &format!("{}: {}", API, err)),
                };
                if !config_once.registration_phase {
                    return refused(registration_inactive(API, &handler_name));
                }

                let name = options.get::<_, String>("name").ok();
                if config_once
                    .collect(
                        CollectedRegistration::new(
                            RegistrationKind::ScheduledJob,
                            name.clone().unwrap_or_else(|| handler_name.clone()),
                        )
                        .with_handler(handler_name.clone()),
                    )
                    .is_some()
                {
                    return host_ok(serde_json::json!({ "ok": true }));
                }

                match register_once_handle.register_one_off(
                    &script_uri_once,
                    &handler_name,
                    name,
                    run_at,
                ) {
                    Ok(job) => host_ok(serde_json::json!({
                        "ok": true,
                        "jobId": job.id.to_string(),
                        "name": job.key,
                        "nextRun": job.schedule.next_run().to_rfc3339(),
                    })),
                    Err(err) => host_failure("Error", &format!("{}: {}", API, err)),
                }
            },
        )?;

        let register_recurring_handle = scheduler_handle.clone();
        let script_uri_recurring = script_uri.to_string();
        let config_recurring = self.config.clone();
        let register_recurring = Function::new(
            ctx.clone(),
            move |options: rquickjs::Object<'_>| -> String {
                const API: &str = "schedulerService.registerRecurring";
                let handler_name = match handler_of(&options, API) {
                    Ok(handler) => handler,
                    Err(message) => return host_failure("TypeError", &message),
                };

                let interval_ms_opt = options.get::<_, f64>("intervalMilliseconds").ok();
                let interval_min_opt = options.get::<_, f64>("intervalMinutes").ok();
                let interval = match (interval_ms_opt, interval_min_opt) {
                    (Some(_), Some(_)) => {
                        return host_failure(
                            "TypeError",
                            &format!(
                                "{}: give intervalMilliseconds or intervalMinutes, not both",
                                API
                            ),
                        );
                    }
                    (Some(ms), None) if ms.is_finite() && ms >= 100.0 => {
                        ChronoDuration::milliseconds(ms.floor() as i64)
                    }
                    (Some(_), None) => {
                        return host_failure(
                            "RangeError",
                            &format!("{}: intervalMilliseconds must be at least 100", API),
                        );
                    }
                    (None, Some(min)) if min.is_finite() && min >= 1.0 => {
                        ChronoDuration::minutes(min.floor() as i64)
                    }
                    (None, Some(_)) => {
                        return host_failure(
                            "RangeError",
                            &format!("{}: intervalMinutes must be at least 1", API),
                        );
                    }
                    (None, None) => {
                        return host_failure(
                            "TypeError",
                            &format!(
                                "{}: options.intervalMilliseconds or options.intervalMinutes is required",
                                API
                            ),
                        );
                    }
                };

                let first_run = match options.get::<_, String>("startAt") {
                    Ok(start_at) => match scheduler::parse_utc_timestamp(&start_at) {
                        Ok(ts) => Some(ts),
                        Err(err) => {
                            return host_failure("TypeError", &format!("{}: {}", API, err));
                        }
                    },
                    Err(_) => None,
                };
                if !config_recurring.registration_phase {
                    return refused(registration_inactive(API, &handler_name));
                }

                let name = options.get::<_, String>("name").ok();
                if config_recurring
                    .collect(
                        CollectedRegistration::new(
                            RegistrationKind::ScheduledJob,
                            name.clone().unwrap_or_else(|| handler_name.clone()),
                        )
                        .with_handler(handler_name.clone()),
                    )
                    .is_some()
                {
                    return host_ok(serde_json::json!({ "ok": true }));
                }

                match register_recurring_handle.register_recurring(
                    &script_uri_recurring,
                    &handler_name,
                    name,
                    interval,
                    first_run,
                ) {
                    Ok(job) => host_ok(serde_json::json!({
                        "ok": true,
                        "jobId": job.id.to_string(),
                        "name": job.key,
                        "nextRun": job.schedule.next_run().to_rfc3339(),
                    })),
                    Err(err) => host_failure("Error", &format!("{}: {}", API, err)),
                }
            },
        )?;

        let script_uri_clear = script_uri.to_string();
        let config_clear = self.config.clone();
        let clear_all = Function::new(ctx.clone(), move || -> String {
            // Clearing is the inverse of registering and mutates the same
            // registry, so it follows the same phase rule.
            if !config_clear.registration_phase {
                return refused(
                    "schedulerService.clearAll: no jobs cleared - scheduled job changes only \
                     take effect during script startup and init()"
                        .to_string(),
                );
            }
            // A dry run reports what a script would register, and an emptied
            // job table is not part of that.
            if config_clear.is_dry_run() {
                return host_ok(serde_json::json!({ "ok": true, "cleared": 0 }));
            }
            let removed = scheduler::clear_script_jobs(&script_uri_clear);
            host_ok(serde_json::json!({ "ok": true, "cleared": removed }))
        })?;

        host.set("registerOnce", register_once)?;
        host.set("registerRecurring", register_recurring)?;
        host.set("clearAll", clear_all)?;
        ctx.globals().set("__hostScheduler", host)?;
        crate::bytecode::eval_program(ctx, "engine://scheduler-prelude", SCHEDULER_PRELUDE)
            .map_err(|e| {
                rquickjs::Error::new_from_js_message(
                    "schedulerService",
                    "prelude",
                    &format!("scheduler prelude failed to load: {}", e),
                )
            })?;

        Ok(())
    }
}
