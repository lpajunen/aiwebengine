//! `fetch`, `fetchAll` and `fetchStream`.

use super::*;
use crate::security::Capability;
use rquickjs::{Function, Result as JsResult};
use tracing::debug;

/// The JavaScript half of `fetch()`: wraps the Rust call's JSON envelope in a
/// response that can be awaited, read as an object, or parsed as a string.
pub(super) const FETCH_PRELUDE: &str = include_str!("../../../assets/fetch_prelude.js");

/// One entry of a `fetchAll` list, as JavaScript writes it.
///
/// `{ url, options }` rather than the positional pair `fetch` takes, because
/// a list of two-element arrays is the shape nobody reads back correctly six
/// months later.
#[derive(serde::Deserialize)]
pub(super) struct FetchRequestSpec {
    pub(super) url: String,
    #[serde(default)]
    pub(super) options: crate::http_client::FetchOptions,
}

impl SecureGlobalContext {
    /// Setup fetch() function for HTTP requests with secret injection
    pub(super) fn setup_fetch_function(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
    ) -> JsResult<()> {
        let global = ctx.globals();
        let script_uri_owned = script_uri.to_string();
        // Capture the user_id at script setup time for secret lookup in
        // user_secrets.
        //
        // Withheld when this execution is acting for somebody who did not
        // authorise it to use their secrets. The lookup then finds no personal
        // key and falls back to the script's own, which is what an undelegated
        // background task already gets — so a narrower grant lands the caller
        // in the weaker position rather than in an error, and `{{secret:...}}`
        // goes on meaning what it means.
        let user_id_for_fetch = if self
            .config
            .allows_delegated(crate::delegation::Scope::Secrets)
        {
            self.user_context.user_id.clone()
        } else {
            None
        };

        // Checked inside the call rather than by withholding `__hostFetch`:
        // the prelude defines `fetch` on top of it and a missing global would
        // be a `ReferenceError` naming an engine-private name, where a script
        // that is not allowed out wants to hear that and catch it.
        let user_ctx_fetch = self.user_context.clone();
        // Whether a `{{secret:...}}` template in this request may be resolved.
        // Withheld separately from the network itself, because "may call an
        // API" and "may use the key" are different grants: model-authored code
        // that may fetch a public endpoint should not thereby reach the
        // account's credentials.
        let may_read_secrets = self.user_context.has_capability(&Capability::ReadSecrets);

        // Clones for the parallel and streaming bindings below, which resolve
        // secrets against the same person this one does.
        let user_id_all = user_id_for_fetch.clone();
        let user_id_stream = user_id_for_fetch.clone();

        // Create the fetch function (synchronous version)
        let fetch_fn = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  url: String,
                  options_json: Option<String>|
                  -> JsResult<String> {
                if !user_ctx_fetch.has_capability(&Capability::UseNetwork) {
                    return Err(capability_error(
                        "fetch",
                        &Capability::UseNetwork,
                        &user_ctx_fetch,
                    ));
                }

                // Parse options from JSON string
                let options: crate::http_client::FetchOptions = if let Some(json_str) = options_json
                {
                    serde_json::from_str(&json_str).map_err(|e| {
                        rquickjs::Error::new_from_js_message(
                            "options",
                            "FetchOptions",
                            &format!("Invalid fetch options: {}", e),
                        )
                    })?
                } else {
                    Default::default()
                };
                // The engine's field, written after the caller's options are
                // parsed. It is `#[serde(skip)]`, so nothing the script wrote
                // could have set it — this is the only way it is ever filled.
                let mut options = options;
                options.network_scope = user_ctx_fetch.network_scope.clone();

                // A template this execution may not resolve is refused, not
                // sent as itself: a request carrying the literal
                // `{{secret:...}}` in an `Authorization` header is a
                // credential-shaped string going to a third party, and the
                // 401 that comes back explains nothing. This differs from a
                // missing delegation scope on purpose — there the person is
                // absent and falling back to the script's own key is the
                // weaker position, here the caller asked to hold less and
                // should hear that it does.
                if !may_read_secrets && crate::http_client::names_a_secret(&url, &options) {
                    return Err(capability_error(
                        "fetch",
                        &Capability::ReadSecrets,
                        &user_ctx_fetch,
                    ));
                }

                tracing::debug!("Fetching URL: {} from script: {}", url, script_uri_owned);

                // Create HTTP client
                let client = crate::http_client::HttpClient::new().map_err(|e| {
                    rquickjs::Error::new_from_js_message(
                        "fetch",
                        "client_init",
                        &format!("Failed to create HTTP client: {}", e),
                    )
                })?;

                // Perform the fetch (synchronous) with script_uri and user_id for secret resolution
                let response = client
                    .fetch(
                        url.clone(),
                        options,
                        Some(&script_uri_owned),
                        user_id_for_fetch.as_deref(),
                    )
                    .map_err(|e| {
                        rquickjs::Error::new_from_js_message(
                            "fetch",
                            "request_failed",
                            &format!("Fetch error: {}", e),
                        )
                    })?;

                // Convert response to JSON string
                let response_json = serde_json::to_string(&response).map_err(|e| {
                    rquickjs::Error::new_from_js_message(
                        "fetch",
                        "serialize",
                        &format!("Failed to serialize response: {}", e),
                    )
                })?;

                Ok(response_json)
            },
        )?;

        // The Rust half is installed under a private name. `fetch()` itself is
        // defined by the prelude below, which wraps this envelope in something
        // that can be awaited, read as an object, or parsed as the string this
        // used to return.
        global.set("__hostFetch", fetch_fn)?;

        // `__hostFetchAll` — several requests in flight at once.
        //
        // `fetch` is a synchronous host call, so `Promise.all` over three of
        // them sequences them and the wall clock is the sum. For an agent
        // running three tool calls that is the difference between fitting
        // inside the execution budget and not.
        //
        // Same checks per request as a single `fetch`, because it *is* a
        // single fetch per request — only on the blocking pool rather than
        // on this thread. The capability gates are here rather than inside,
        // so a refused batch costs no connections.
        let script_uri_all = script_uri.to_string();
        let user_ctx_all = self.user_context.clone();
        let fetch_all = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, requests_json: String| -> JsResult<String> {
                if !user_ctx_all.has_capability(&Capability::UseNetwork) {
                    return Err(capability_error(
                        "fetchAll",
                        &Capability::UseNetwork,
                        &user_ctx_all,
                    ));
                }

                let described: Vec<FetchRequestSpec> = serde_json::from_str(&requests_json)
                    .map_err(|e| {
                        rquickjs::Error::new_from_js_message(
                            "fetchAll",
                            "requests",
                            &format!("Invalid request list: {}", e),
                        )
                    })?;

                // Checked across the whole batch before any of it is sent,
                // for the reason a single `fetch` checks before sending: a
                // template this execution may not resolve must not reach a
                // third party as itself.
                if !may_read_secrets
                    && described.iter().any(|request| {
                        crate::http_client::names_a_secret(&request.url, &request.options)
                    })
                {
                    return Err(capability_error(
                        "fetchAll",
                        &Capability::ReadSecrets,
                        &user_ctx_all,
                    ));
                }

                let requests = described
                    .into_iter()
                    .map(|request| crate::http_client::ParallelRequest {
                        url: request.url,
                        options: crate::http_client::FetchOptions {
                            // Per request, because each carries its own
                            // options object and a batch that bounded only the
                            // first would bound nothing.
                            network_scope: user_ctx_all.network_scope.clone(),
                            ..request.options
                        },
                    })
                    .collect();

                // Each answer is its own envelope. One refused URL is an
                // error in its own slot rather than a failed batch: the
                // caller asked for several answers and has a use for the
                // ones that arrived.
                let client = crate::http_client::HttpClient::new().map_err(|e| {
                    rquickjs::Error::new_from_js_message(
                        "fetchAll",
                        "client_init",
                        &format!("Failed to create HTTP client: {}", e),
                    )
                })?;

                let answers: Vec<serde_json::Value> = client
                    .fetch_all(requests, Some(&script_uri_all), user_id_all.as_deref())
                    .into_iter()
                    .map(|answer| match answer {
                        Ok(response) => serde_json::json!({
                            "ok": true,
                            "response": serde_json::to_value(&response).unwrap_or_default(),
                        }),
                        Err(e) => serde_json::json!({
                            "ok": false,
                            "error": e.to_string(),
                        }),
                    })
                    .collect();

                Ok(serde_json::Value::Array(answers).to_string())
            },
        )?;
        global.set("__hostFetchAll", fetch_all)?;

        // `__hostFetchStreamStart` — a response read a piece at a time.
        //
        // What a buffered `fetch` cannot do: consume a model's token stream,
        // an events endpoint, a log tail on another service. The connection
        // stays open between host calls, held against this execution and
        // dropped with it.
        let script_uri_stream = script_uri.to_string();
        let user_ctx_stream = self.user_context.clone();
        let stream_start = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>,
                  url: String,
                  options_json: Option<String>|
                  -> JsResult<String> {
                if !user_ctx_stream.has_capability(&Capability::UseNetwork) {
                    return Err(capability_error(
                        "fetchStream",
                        &Capability::UseNetwork,
                        &user_ctx_stream,
                    ));
                }

                let options: crate::http_client::FetchOptions = match options_json {
                    Some(json) => serde_json::from_str(&json).map_err(|e| {
                        rquickjs::Error::new_from_js_message(
                            "fetchStream",
                            "options",
                            &format!("Invalid fetch options: {}", e),
                        )
                    })?,
                    None => Default::default(),
                };
                let mut options = options;
                options.network_scope = user_ctx_stream.network_scope.clone();

                if !may_read_secrets && crate::http_client::names_a_secret(&url, &options) {
                    return Err(capability_error(
                        "fetchStream",
                        &Capability::ReadSecrets,
                        &user_ctx_stream,
                    ));
                }

                let client = crate::http_client::HttpClient::new().map_err(|e| {
                    rquickjs::Error::new_from_js_message(
                        "fetchStream",
                        "client_init",
                        &format!("Failed to create HTTP client: {}", e),
                    )
                })?;

                let stream = client
                    .fetch_streaming(
                        url,
                        options,
                        Some(&script_uri_stream),
                        user_id_stream.as_deref(),
                    )
                    .map_err(|e| {
                        rquickjs::Error::new_from_js_message(
                            "fetchStream",
                            "request_failed",
                            &format!("Fetch error: {}", e),
                        )
                    })?;

                let opening = serde_json::json!({
                    "status": stream.status,
                    "ok": stream.ok,
                    "headers": stream.headers,
                });

                let id = crate::http_client::register_stream(stream).map_err(|e| {
                    rquickjs::Error::new_from_js_message("fetchStream", "too_many", &e.to_string())
                })?;

                let mut opening = opening;
                opening["streamId"] = serde_json::json!(id.to_string());
                Ok(opening.to_string())
            },
        )?;
        global.set("__hostFetchStreamStart", stream_start)?;

        // Blocks until the next piece arrives, which is the point: the
        // caller has asked for the next token and has nothing to do until it
        // has one. An ended stream answers `done` rather than an error, so a
        // loop over it terminates without a `try`.
        let stream_read = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, stream_id: String| -> JsResult<String> {
                let Ok(id) = stream_id.parse::<u64>() else {
                    return Err(rquickjs::Error::new_from_js_message(
                        "fetchStream",
                        "read",
                        "that is not a stream id",
                    ));
                };

                match crate::http_client::read_stream(id) {
                    Ok(Some(chunk)) => Ok(serde_json::json!({
                        "done": false,
                        "value": chunk,
                    })
                    .to_string()),
                    Ok(None) => Ok(serde_json::json!({ "done": true }).to_string()),
                    Err(e) => Err(rquickjs::Error::new_from_js_message(
                        "fetchStream",
                        "read",
                        &e.to_string(),
                    )),
                }
            },
        )?;
        global.set("__hostFetchStreamRead", stream_read)?;

        let stream_close = Function::new(
            ctx.clone(),
            move |_ctx: rquickjs::Ctx<'_>, stream_id: String| -> JsResult<bool> {
                Ok(stream_id
                    .parse::<u64>()
                    .map(crate::http_client::close_stream)
                    .unwrap_or(false))
            },
        )?;
        global.set("__hostFetchStreamClose", stream_close)?;

        // Compiled once per process and cached under a stable key, the way the
        // test prelude is. Installing it here covers every context that gets
        // host functions, rather than each entry point remembering to do it.
        crate::bytecode::eval_program(ctx, "engine://fetch-prelude", FETCH_PRELUDE).map_err(
            |e| {
                rquickjs::Error::new_from_js_message(
                    "fetch",
                    "prelude",
                    &format!("fetch prelude failed to load: {}", e),
                )
            },
        )?;

        debug!("fetch() function initialized with secret injection support");

        Ok(())
    }
}
