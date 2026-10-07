//! `rateLimit`: budgets the script sizes and the engine keys.

use super::*;
use rquickjs::{Function, Result as JsResult};
use tracing::debug;

pub(super) const RATE_LIMIT_PRELUDE: &str = include_str!("../../../assets/rate_limit_prelude.js");

impl SecureGlobalContext {
    /// `rateLimit` — a budget the script sizes and the engine keys.
    ///
    /// A script with a public form otherwise writes its own limiter, keyed by
    /// whatever it can read, which is a header the caller wrote. Here the
    /// script says how big a bucket is and the engine says whose it is: the
    /// signed-in person, else the address the edge judged, so neither a forged
    /// header nor a script bug can hand a caller a fresh allowance or spend
    /// somebody else's. Buckets live in Postgres, so the budget holds across
    /// every instance of the engine.
    pub(super) fn setup_rate_limit_object(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
    ) -> JsResult<()> {
        let global = ctx.globals();
        let host = rquickjs::Object::new(ctx.clone())?;

        let script = script_uri.to_string();
        let person = self.user_context.user_id.clone();
        let client_ip = self.config.client_ip.clone();
        let consume = Function::new(
            ctx.clone(),
            move |options_json: String| -> JsResult<String> {
                let refuse = |message: String| {
                    rquickjs::Error::new_from_js_message(
                        "rateLimit.consume",
                        "range_error",
                        &format!("rateLimit.consume: {}", message),
                    )
                };
                let options: serde_json::Value = serde_json::from_str(&options_json)
                    .map_err(|e| refuse(format!("options are not valid JSON: {}", e)))?;

                let bucket = options["bucket"].as_str().unwrap_or_default();
                if bucket.is_empty()
                    || bucket.len() > 64
                    || !bucket
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
                {
                    return Err(refuse(format!(
                        "'{}' is not a bucket name: use 1-64 letters, digits, '_', '-' or '.'",
                        bucket
                    )));
                }
                let whole = |name: &str, low: u64, high: u64| -> JsResult<u32> {
                    match options[name].as_u64() {
                        Some(n) if (low..=high).contains(&n) => Ok(n as u32),
                        _ => Err(refuse(format!(
                            "'{}' must be a whole number from {} to {}",
                            name, low, high
                        ))),
                    }
                };
                let limit = whole("limit", 1, 1_000_000)?;
                let window_seconds = whole("windowSeconds", 1, 7 * 24 * 3600)?;
                let cost = if options["cost"].is_null() {
                    1
                } else {
                    whole("cost", 1, u64::from(limit))?
                };

                // Whose bucket, decided here and never by the script.
                let who = match options["per"].as_str().unwrap_or("caller") {
                    "caller" => match (&person, &client_ip) {
                        (Some(id), _) => format!("user:{}", id),
                        (None, Some(ip)) => format!("ip:{}", ip),
                        (None, None) => "anonymous".to_string(),
                    },
                    "ip" => format!(
                        "ip:{}",
                        client_ip
                            .as_deref()
                            .unwrap_or(crate::security::client_ip::UNKNOWN)
                    ),
                    "script" => "script".to_string(),
                    other => {
                        return Err(unknown_name_error(
                            "rateLimit.consume",
                            "per",
                            other,
                            &["caller", "ip", "script"],
                        ));
                    }
                };

                let Some(limiter) = crate::security::rate_limiting::shared() else {
                    // No limiter before startup — a unit test. Nothing to spend
                    // against, so nothing is refused, as on a database error.
                    return Ok(serde_json::json!({
                        "allowed": true,
                        "remaining": limit - cost,
                        "retryAfterSeconds": null,
                    })
                    .to_string());
                };
                let key = crate::security::RateLimitKey::Script {
                    script: script.clone(),
                    bucket: bucket.to_string(),
                    who,
                };
                let result = crate::database::run_blocking(limiter.consume_script_budget(
                    key,
                    cost,
                    limit,
                    window_seconds,
                ));

                let remaining = result.remaining_tokens.max(0.0);
                let retry_after = (!result.allowed).then(|| {
                    let missing = f64::from(cost) - remaining;
                    (missing * f64::from(window_seconds) / f64::from(limit))
                        .ceil()
                        .max(1.0) as u64
                });
                Ok(serde_json::json!({
                    "allowed": result.allowed,
                    "remaining": remaining.floor() as u64,
                    "retryAfterSeconds": retry_after,
                })
                .to_string())
            },
        )?;

        host.set("consume", consume)?;
        global.set("__hostRateLimit", host)?;

        crate::bytecode::eval_program(ctx, "engine://rate-limit-prelude", RATE_LIMIT_PRELUDE)
            .map_err(|e| {
                rquickjs::Error::new_from_js_message(
                    "rateLimit",
                    "prelude",
                    &format!("rateLimit prelude failed to load: {}", e),
                )
            })?;

        debug!("rateLimit initialized for script: {}", script_uri);
        Ok(())
    }
}
