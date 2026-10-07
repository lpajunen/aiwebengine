//! `routeRegistry`: routes, streams and stream messages.

use super::*;
use crate::repository;
use crate::security::{SecurityAuditor, UserContext};
use rquickjs::{Function, Result as JsResult};
use std::collections::HashMap;

/// Builds `routeRegistry` over `__hostRouteRegistry`.
pub(super) const ROUTE_PRELUDE: &str = include_str!("../../../assets/route_prelude.js");

/// What a registered path leads to.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum RouteTarget {
    /// A script function, called for one HTTP method.
    Handler { name: String, method: String },
    /// A server-sent event stream the engine holds open. `authorize` names
    /// the function that decides who may connect.
    Stream { authorize: Option<String> },
    /// A file of the script's tree, served by the engine. `authorize` names
    /// the function that decides who may read it.
    File {
        path: String,
        authorize: Option<String>,
    },
}

/// One `registerRoute` spec, once read.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct RouteSpec {
    pub(super) target: RouteTarget,
    pub(super) summary: Option<String>,
    pub(super) description: Option<String>,
    pub(super) tags: Vec<String>,
    pub(super) parameters: Option<serde_json::Value>,
    pub(super) request_body: Option<serde_json::Value>,
}

/// The keys a spec may carry. Anything else is refused rather than ignored,
/// since a misspelt `authorize` that was silently dropped would publish the
/// file it was meant to guard.
pub(super) const ROUTE_SPEC_KEYS: [&str; 10] = [
    "handler",
    "stream",
    "file",
    "method",
    "authorize",
    "summary",
    "description",
    "tags",
    "parameters",
    "requestBody",
];

/// Read a `registerRoute` spec. The error is the message a `TypeError`
/// carries: every way a spec can be wrong is a mistake in the call.
pub(super) fn parse_route_spec(value: &serde_json::Value) -> Result<RouteSpec, String> {
    let object = value
        .as_object()
        .ok_or("the spec is an object: { handler }, { stream: true } or { file }")?;
    if let Some(unknown) = object
        .keys()
        .find(|key| !ROUTE_SPEC_KEYS.contains(&key.as_str()))
    {
        return Err(format!(
            "unknown spec key '{}' (expected one of {})",
            unknown,
            ROUTE_SPEC_KEYS.join(", ")
        ));
    }

    let text = |key: &str| -> Result<Option<String>, String> {
        match object.get(key) {
            None | Some(serde_json::Value::Null) => Ok(None),
            Some(serde_json::Value::String(s)) if !s.trim().is_empty() => Ok(Some(s.clone())),
            Some(_) => Err(format!("'{}' must be a non-empty string", key)),
        }
    };

    let handler = text("handler")?;
    let file = text("file")?;
    let stream = match object.get("stream") {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::Bool(true)) => true,
        Some(_) => return Err("'stream' is either true or absent".to_string()),
    };
    let targets = [handler.is_some(), stream, file.is_some()]
        .into_iter()
        .filter(|present| *present)
        .count();
    if targets != 1 {
        return Err(
            "a spec names exactly one target: { handler }, { stream: true } or { file }"
                .to_string(),
        );
    }

    let authorize = text("authorize")?;
    if let Some(name) = authorize.as_deref() {
        validate_function_name("authorize", name)?;
    }
    let parameters = object.get("parameters").filter(|v| !v.is_null()).cloned();
    let request_body = object.get("requestBody").filter(|v| !v.is_null()).cloned();
    let method = text("method")?;

    let target = if let Some(name) = handler {
        // The handler *is* a handler route's authorization: it runs as the
        // requesting user and answers whatever it decides to. A second
        // function in front of it would be two places deciding one thing.
        if authorize.is_some() {
            return Err(
                "'authorize' is for streams and files; a handler route's handler is \
                 where it decides who may call it"
                    .to_string(),
            );
        }
        validate_function_name("handler", &name)?;
        RouteTarget::Handler {
            name,
            method: method.unwrap_or_else(|| "GET".to_string()),
        }
    } else {
        if method.is_some() {
            return Err("'method' is for handler routes; streams and files answer GET".to_string());
        }
        if parameters.is_some() || request_body.is_some() {
            return Err("'parameters' and 'requestBody' describe a handler route".to_string());
        }
        match file {
            Some(path) => RouteTarget::File { path, authorize },
            None => RouteTarget::Stream { authorize },
        }
    };

    let tags = match object.get("tags") {
        None | Some(serde_json::Value::Null) => Vec::new(),
        Some(serde_json::Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| "'tags' is an array of strings".to_string())
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("'tags' is an array of strings".to_string()),
    };

    Ok(RouteSpec {
        target,
        summary: text("summary")?,
        description: text("description")?,
        tags,
        parameters,
        request_body,
    })
}

/// A function a registration names must be one a script can define.
pub(super) fn validate_function_name(key: &str, name: &str) -> Result<(), String> {
    if name.len() > 100 {
        return Err(format!("'{}' is too long (max 100 characters)", key));
    }
    if !name
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '$')
    {
        return Err(format!(
            "'{}' must name a function: letters, digits, '_' and '$' only",
            key
        ));
    }
    Ok(())
}

/// What a well-formed registration came to.
#[derive(Debug, PartialEq)]
pub(super) enum Registered {
    Done,
    /// Not registered, for a reason the script is told about rather than
    /// thrown at, so its other registrations survive.
    Refused(String),
}

/// A registration that could not be attempted: the name is the error type
/// the prelude throws.
pub(super) struct RegistrationFailure {
    pub(super) name: &'static str,
    pub(super) message: String,
}

impl RegistrationFailure {
    pub(super) fn type_error(message: impl Into<String>) -> Self {
        Self {
            name: "TypeError",
            message: message.into(),
        }
    }

    pub(super) fn error(message: impl Into<String>) -> Self {
        Self {
            name: "Error",
            message: message.into(),
        }
    }
}

/// Everything `registerRoute` needs, captured once per execution.
pub(super) struct RouteRegistrar {
    pub(super) user_context: UserContext,
    pub(super) auditor: SecurityAuditor,
    pub(super) script_uri: String,
    pub(super) config: GlobalSecurityConfig,
    pub(super) record: Option<RouteRegisterFn>,
}

pub(super) const REGISTER_ROUTE: &str = "routeRegistry.registerRoute";

impl RouteRegistrar {
    pub(super) fn register(
        &self,
        path: &str,
        spec: RouteSpec,
    ) -> Result<Registered, RegistrationFailure> {
        // Path checks run in every context, so a bad path is reported the same
        // way wherever the call is made.
        if let Some(prefix) = crate::engine_api::reserved_route_prefix(path) {
            return Err(RegistrationFailure::error(format!(
                "path '{}' is reserved for the engine (prefix '{}')",
                path, prefix
            )));
        }
        if !path.starts_with('/') {
            return Err(RegistrationFailure::type_error(format!(
                "path '{}' must start with '/'",
                path
            )));
        }
        if path.len() > 500 {
            return Err(RegistrationFailure::type_error(
                "path too long (max 500 characters)",
            ));
        }
        if path.contains("..") || path.contains('\\') {
            return Err(RegistrationFailure::type_error(format!(
                "path '{}' may not contain '..' or '\\'",
                path
            )));
        }

        let RouteSpec {
            target,
            summary,
            description,
            tags,
            parameters,
            request_body,
        } = spec;

        let (mut meta, method) = match target {
            RouteTarget::Handler { name, method } => {
                let Some(record) = self.record.as_ref() else {
                    return Ok(Registered::Refused(registration_inactive(
                        REGISTER_ROUTE,
                        path,
                    )));
                };
                // A dry run reports a collision as its own diagnostic, which
                // needs the registration to get that far.
                if !self.config.is_dry_run()
                    && let Some(collision) =
                        crate::route_index::refusal_for(&self.script_uri, path, &method)
                {
                    return Ok(Registered::Refused(collision.reason()));
                }
                let mut meta = repository::RouteMetadata::simple(name);
                meta.summary = summary;
                meta.description = description;
                meta.tags = tags;
                meta.parameters = parameters;
                meta.request_body = request_body;
                record(path, &meta, Some(&method))
                    .map_err(|e| RegistrationFailure::error(e.to_string()))?;
                return Ok(Registered::Done);
            }
            RouteTarget::Stream { authorize } => {
                self.require(&crate::security::Capability::ManageStreams, "stream", path)?;
                if !self.config.registration_phase {
                    return Ok(Registered::Refused(registration_inactive(
                        REGISTER_ROUTE,
                        path,
                    )));
                }
                let registration = CollectedRegistration::new(RegistrationKind::Stream, path);
                let registration = match authorize.as_ref() {
                    Some(function) => registration.with_handler(function.clone()),
                    None => registration,
                };
                if self.config.collect(registration).is_some() {
                    return Ok(Registered::Done);
                }
                if !self.config.is_dry_run()
                    && let Some(collision) = crate::route_index::refusal_for(
                        &self.script_uri,
                        path,
                        repository::STREAM_METHOD,
                    )
                {
                    return Ok(Registered::Refused(collision.reason()));
                }
                (
                    repository::RouteMetadata::stream(authorize),
                    repository::STREAM_METHOD,
                )
            }
            RouteTarget::File {
                path: file,
                authorize,
            } => {
                self.require(&crate::security::Capability::WriteAssets, "file", path)?;
                if file.len() > 255 || file.contains("..") || file.contains('\\') {
                    return Err(RegistrationFailure::type_error(format!(
                        "'{}' is not a file path of this script's tree",
                        file
                    )));
                }
                if !self.config.registration_phase {
                    return Ok(Registered::Refused(registration_inactive(
                        REGISTER_ROUTE,
                        path,
                    )));
                }
                if repository::fetch_asset(&self.script_uri, &file).is_none() {
                    return Ok(Registered::Refused(format!(
                        "'{}' is not a file of this script",
                        file
                    )));
                }
                // Exposure belongs to the tree, not to this call. A file
                // outside `public/` is one the directory says is private, and
                // publishing it was the mistake nothing else in the engine
                // could see — not the write path, not the revision manifest,
                // not a git diff. Refused rather than warned about, which is
                // what makes the directory the answer rather than a
                // suggestion: publishing a file is moving it, which is a
                // reviewable act.
                if !crate::exposure::is_publishable(&file) {
                    crate::exposure::note_refusal(&self.script_uri, false, path, &file);
                    tracing::warn!(
                        script = %self.script_uri,
                        path = %path,
                        file = %file,
                        "Refused to publish a file from outside '{}'",
                        crate::exposure::PUBLIC_DIR,
                    );
                    return Ok(Registered::Refused(format!(
                        "'{}' is not under '{}', so it is not a file the world may read. \
                         Move it to '{}{}' and register that. A file's directory is what \
                         says whether it is public; see /engine/exposure_report.",
                        file,
                        crate::exposure::PUBLIC_DIR,
                        crate::exposure::PUBLIC_DIR,
                        file,
                    )));
                }
                if self
                    .config
                    .collect(CollectedRegistration::new(
                        RegistrationKind::AssetRoute,
                        path,
                    ))
                    .is_some()
                {
                    return Ok(Registered::Done);
                }
                if !self.config.is_dry_run()
                    && let Some(collision) = crate::route_index::refusal_for(
                        &self.script_uri,
                        path,
                        repository::ASSET_METHOD,
                    )
                {
                    return Ok(Registered::Refused(collision.reason()));
                }
                (
                    repository::RouteMetadata::file(file, authorize),
                    repository::ASSET_METHOD,
                )
            }
        };

        // A stream and a file route record into the sink a handler route
        // does: one kind of registration, differing only in what the engine
        // does once the path matches. Being in the script's registrations is
        // what gives them `:param` and `/*`, the host filter, and an
        // unregistration when the script stops making the call.
        let Some(record) = self.record.as_ref() else {
            return Ok(Registered::Refused(registration_inactive(
                REGISTER_ROUTE,
                path,
            )));
        };
        meta.summary = summary;
        meta.description = description;
        meta.tags = tags;
        record(path, &meta, Some(method)).map_err(|e| RegistrationFailure::error(e.to_string()))?;
        Ok(Registered::Done)
    }

    /// The capability a stream or a file route takes, audited when missing.
    pub(super) fn require(
        &self,
        capability: &crate::security::Capability,
        resource: &str,
        path: &str,
    ) -> Result<(), RegistrationFailure> {
        let Err(e) = self.user_context.require_capability(capability) else {
            return Ok(());
        };
        if self.config.enable_audit_logging
            && let Ok(rt) = tokio::runtime::Handle::try_current()
        {
            let auditor = self.auditor.clone();
            let user_id = self.user_context.user_id.clone();
            let resource = resource.to_string();
            let path = path.to_string();
            rt.spawn(async move {
                let _ = auditor
                    .log_event(
                        crate::security::SecurityEvent::new(
                            crate::security::SecurityEventType::AuthorizationFailure,
                            crate::security::SecuritySeverity::Medium,
                            user_id,
                        )
                        .with_resource(resource)
                        .with_action("register".to_string())
                        .with_detail("path", &path),
                    )
                    .await;
            });
        }
        Err(RegistrationFailure::error(e.to_string()))
    }
}

/// Send to a stream's connections, answering in the host envelope.
///
/// The shared `/system/` namespace is open to every script. The engine's own
/// script-update stream is deliberately not: it is broadcast to from Rust
/// (`engine_api::broadcast_script_update`), which never passes through here,
/// so exempting it would only let a script forge engine notifications to
/// every subscriber.
pub(super) fn send_stream_message(
    user_context: &UserContext,
    auditor: &SecurityAuditor,
    api: &str,
    path: &str,
    message: &str,
    filter: Option<(
        &HashMap<String, String>,
        crate::stream_registry::FilterMatchMode,
    )>,
) -> String {
    let audit = |event_type, severity, action: &str| {
        if let Ok(rt) = tokio::runtime::Handle::try_current() {
            let auditor = auditor.clone();
            let user_id = user_context.user_id.clone();
            let action = action.to_string();
            let path = path.to_string();
            let length = message.len().to_string();
            rt.spawn(async move {
                let _ = auditor
                    .log_event(
                        crate::security::SecurityEvent::new(event_type, severity, user_id)
                            .with_resource("stream".to_string())
                            .with_action(action)
                            .with_detail("path", &path)
                            .with_detail("message_length", length),
                    )
                    .await;
            });
        }
    };

    if !path.starts_with("/system/")
        && let Err(e) = user_context.require_capability(&crate::security::Capability::ManageStreams)
    {
        audit(
            crate::security::SecurityEventType::AuthorizationFailure,
            crate::security::SecuritySeverity::Medium,
            api,
        );
        return host_failure("Error", &format!("routeRegistry.{}: {}", api, e));
    }
    audit(
        crate::security::SecurityEventType::SystemSecurityEvent,
        crate::security::SecuritySeverity::Low,
        api,
    );

    let registry = &crate::stream_registry::GLOBAL_STREAM_REGISTRY;
    let result = match filter {
        Some((filter, mode)) => {
            registry.broadcast_to_stream_with_filter_mode(path, message, filter, mode)
        }
        None => registry.broadcast_to_stream(path, message),
    };
    match result {
        Ok(sent) => host_ok(serde_json::json!({
            "delivered": sent.successful_sends,
            "connections": sent.total_connections,
            "failed": sent.failed_connections.len(),
        })),
        Err(e) => host_failure(
            "Error",
            &format!("routeRegistry.{}: '{}': {}", api, path, e),
        ),
    }
}

impl SecureGlobalContext {
    /// Install `__hostRouteRegistry` and the prelude that builds
    /// `routeRegistry` over it.
    ///
    /// One registration call, `registerRoute(path, spec)`, whose spec says what
    /// the path leads to: `{ handler }`, `{ stream: true }` or `{ file }` — one
    /// call because it is one record (`RouteMetadata` carries a `RouteKind`).
    ///
    /// A refusal, a success and a misuse are distinguishable without parsing
    /// English: misuse throws — a malformed spec, a reserved path, a capability the
    /// caller lacks — and a *refusal* is a value, `{ ok: false, reason }`, so a
    /// script that gets one path wrong keeps the rest of its registrations.
    pub(super) fn setup_route_registry(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
        register_fn: Option<RouteRegisterFn>,
    ) -> JsResult<()> {
        let host = rquickjs::Object::new(ctx.clone())?;

        let registrar = RouteRegistrar {
            user_context: self.user_context.clone(),
            auditor: self.auditor.clone(),
            script_uri: script_uri.to_string(),
            config: self.config.clone(),
            record: register_fn,
        };
        let register = Function::new(
            ctx.clone(),
            move |path: String, spec_json: String| -> String {
                let spec = match serde_json::from_str::<serde_json::Value>(&spec_json)
                    .map_err(|e| e.to_string())
                    .and_then(|value| parse_route_spec(&value))
                {
                    Ok(spec) => spec,
                    Err(message) => {
                        return host_failure(
                            "TypeError",
                            &format!("routeRegistry.registerRoute: {}", message),
                        );
                    }
                };
                let attempted = match &spec.target {
                    RouteTarget::Handler { name, method } => {
                        CollectedRegistration::new(RegistrationKind::Route, path.clone())
                            .with_method(method.clone())
                            .with_handler(name.clone())
                    }
                    RouteTarget::Stream { .. } => {
                        CollectedRegistration::new(RegistrationKind::Stream, path.clone())
                    }
                    RouteTarget::File { .. } => {
                        CollectedRegistration::new(RegistrationKind::AssetRoute, path.clone())
                    }
                };
                match registrar.register(&path, spec) {
                    Ok(Registered::Done) => host_ok(serde_json::json!({ "ok": true })),
                    Ok(Registered::Refused(reason)) => {
                        registrar
                            .config
                            .note_dry_run_refusal(attempted.with_refusal(reason.clone()));
                        host_ok(serde_json::json!({ "ok": false, "reason": reason }))
                    }
                    Err(failure) => host_failure(
                        failure.name,
                        &format!("routeRegistry.registerRoute: {}", failure.message),
                    ),
                }
            },
        )?;
        host.set("register", register)?;

        let user_ctx_send = self.user_context.clone();
        let auditor_send = self.auditor.clone();
        let send = Function::new(
            ctx.clone(),
            move |path: String, message: rquickjs::Value<'_>| -> JsResult<String> {
                // Typed `any` in the declarations and serialized here, so the
                // object every example passes is the object that arrives.
                let message = json_arg(message, "data")?;
                Ok(send_stream_message(
                    &user_ctx_send,
                    &auditor_send,
                    "sendStreamMessage",
                    &path,
                    &message,
                    None,
                ))
            },
        )?;
        host.set("send", send)?;

        let user_ctx_filtered = self.user_context.clone();
        let auditor_filtered = self.auditor.clone();
        let send_filtered = Function::new(
            ctx.clone(),
            move |path: String,
                  message: rquickjs::Value<'_>,
                  filter_json: Option<String>,
                  match_mode: Option<String>|
                  -> JsResult<String> {
                let message = json_arg(message, "data")?;
                // The prelude serialises the filter object, so a parse failure
                // here is the engine's bug rather than the caller's.
                let filter: HashMap<String, String> = match filter_json {
                    Some(json) => match serde_json::from_str(&json) {
                        Ok(filter) => filter,
                        Err(e) => {
                            return Ok(host_failure(
                                "TypeError",
                                &format!(
                                    "routeRegistry.sendStreamMessageFiltered: the filter maps \
                                     names to strings: {}",
                                    e
                                ),
                            ));
                        }
                    },
                    None => HashMap::new(),
                };
                let match_mode = match match_mode
                    .map(|raw| raw.parse::<crate::stream_registry::FilterMatchMode>())
                    .transpose()
                {
                    Ok(mode) => mode.unwrap_or(crate::stream_registry::FilterMatchMode::Subset),
                    Err(message) => {
                        return Ok(host_failure(
                            "TypeError",
                            &format!("routeRegistry.sendStreamMessageFiltered: {}", message),
                        ));
                    }
                };
                Ok(send_stream_message(
                    &user_ctx_filtered,
                    &auditor_filtered,
                    "sendStreamMessageFiltered",
                    &path,
                    &message,
                    Some((&filter, match_mode)),
                ))
            },
        )?;
        host.set("sendFiltered", send_filtered)?;

        ctx.globals().set("__hostRouteRegistry", host)?;
        crate::bytecode::eval_program(ctx, "engine://route-prelude", ROUTE_PRELUDE).map_err(
            |e| {
                rquickjs::Error::new_from_js_message(
                    "routeRegistry",
                    "prelude",
                    &format!("route prelude failed to load: {}", e),
                )
            },
        )?;

        Ok(())
    }
}

#[cfg(test)]
mod route_spec_tests {
    use super::{RouteTarget, parse_route_spec};
    use serde_json::json;

    #[test]
    fn a_handler_spec_defaults_to_get_and_keeps_its_documentation() {
        let spec = parse_route_spec(&json!({
            "handler": "getThing",
            "summary": "One thing",
            "tags": ["Things"],
            "parameters": [{ "name": "id", "in": "path" }],
        }))
        .expect("a handler spec should parse");
        assert_eq!(
            spec.target,
            RouteTarget::Handler {
                name: "getThing".to_string(),
                method: "GET".to_string()
            }
        );
        assert_eq!(spec.summary.as_deref(), Some("One thing"));
        assert_eq!(spec.tags, vec!["Things".to_string()]);
        assert!(spec.parameters.is_some(), "parameters are an object now");
    }

    #[test]
    fn streams_and_files_carry_their_authorize_function() {
        let stream = parse_route_spec(&json!({ "stream": true, "authorize": "mayWatch" }))
            .expect("a stream spec should parse");
        assert_eq!(
            stream.target,
            RouteTarget::Stream {
                authorize: Some("mayWatch".to_string())
            }
        );
        let file =
            parse_route_spec(&json!({ "file": "public/a.css" })).expect("a file spec should parse");
        assert_eq!(
            file.target,
            RouteTarget::File {
                path: "public/a.css".to_string(),
                authorize: None
            }
        );
    }

    /// Every one of these is a mistake in the call, and every one is the kind
    /// that would otherwise do something other than what was written: a
    /// silently dropped `authorize` publishes the file it was meant to guard.
    #[test]
    fn a_spec_that_is_not_exactly_one_thing_is_refused() {
        for (spec, why) in [
            (json!({}), "no target"),
            (json!({ "handler": "h", "file": "public/a" }), "two targets"),
            (json!({ "stream": false }), "stream must be true"),
            (
                json!({ "file": "public/a", "authorise": "f" }),
                "a misspelt key",
            ),
            (
                json!({ "handler": "h", "authorize": "f" }),
                "authorize on a handler",
            ),
            (
                json!({ "stream": true, "method": "POST" }),
                "a method on a stream",
            ),
            (
                json!({ "file": "public/a", "parameters": [] }),
                "parameters on a file",
            ),
            (
                json!({ "handler": "not a name" }),
                "a handler that is not a name",
            ),
            (
                json!({ "handler": "h", "tags": "x" }),
                "tags that are not an array",
            ),
            (json!("h"), "not an object"),
        ] {
            assert!(parse_route_spec(&spec).is_err(), "should refuse {}", why);
        }
    }
}
