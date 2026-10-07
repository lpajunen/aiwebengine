//! Who an execution acts for (`Principal`), how it is configured, and installing
//! every global.

use super::*;
use crate::repository;
use crate::security::{SecurityAuditor, UserContext};
use rquickjs::Result as JsResult;

/// Secure wrapper for JavaScript global functions that enforces Rust-level validation
pub struct SecureGlobalContext {
    pub(super) user_context: UserContext,
    pub(super) auditor: SecurityAuditor,
    pub(super) config: GlobalSecurityConfig,
}

/// Controls the parts of the JavaScript API whose behaviour depends on *when* a
/// script runs rather than on who is calling it.
///
/// Every global and every method is installed in every context, so a script
/// never has to feature-detect. These flags only decide whether a registration
/// call takes effect.
#[derive(Debug, Clone)]
pub struct GlobalSecurityConfig {
    /// True only while a script's registrations are being collected: engine
    /// startup and the `init()` call that follows it.
    ///
    /// A script's top-level program is re-evaluated on *every* invocation, so
    /// registration calls written at top level run again on each request. In
    /// the registration phase they take effect; everywhere else they are
    /// no-ops that report what happened. They must not throw — that would
    /// break every script that registers at top level rather than in `init()`.
    pub registration_phase: bool,
    /// Disabled where there is no Tokio runtime to spawn the audit writer onto.
    pub enable_audit_logging: bool,
    /// When set, registration calls are validated as usual and then *recorded
    /// here* instead of reaching the engine's live registries.
    ///
    /// This is what makes `/engine/check_script` safe to run against a deployed
    /// script. Only `registerRoute` collects by design — every other registry
    /// (streams, asset routes, MCP, scheduler) is a process-wide
    /// singleton written to directly, so a candidate's `init()` would
    /// otherwise replace the deployed script's resolvers and jobs with its
    /// own, and a broken candidate would take the live script
    /// down with it. Nothing undoes those writes afterwards, which is why the
    /// test runner opts out of the registration phase entirely
    /// (`registration_phase: false`) rather than isolating it.
    ///
    /// Set only together with `registration_phase: true`: the phase check runs
    /// first, so a sink on an inactive context would never be reached.
    pub dry_run_sink: Option<RegistrationSink>,
    /// When set, `console` output is captured here as well as written to the
    /// script's log.
    ///
    /// Capture is what makes `/engine/eval_script` usable, not a convenience on top of
    /// it: `console` writes go through the repository, so they join whatever
    /// transaction is open — and an evaluation that rolls back would otherwise
    /// roll back its own output, losing exactly what the caller asked for.
    pub console_sink: Option<ConsoleSink>,
    /// Which invocation the script's `console` output is attributed to.
    ///
    /// Empty for contexts with no invocation to name; a line written under an
    /// empty context is stored with no invocation.
    pub log_context: repository::LogContext,
    /// The address the request came from, as the edge judged it
    /// ([`crate::security::client_ip`]). `None` where there is no HTTP request
    /// behind the execution. Read by `rateLimit`, which keys a caller's
    /// budget by it when nobody is signed in.
    pub client_ip: Option<String>,
    /// Who this execution acts for. Decides the capabilities every global is
    /// checked against, what a delegated execution is narrowed to, and whether
    /// the `engine` global exists. See [`Principal`].
    pub principal: Principal,
}

/// Who an execution acts for.
///
/// Every way into a script names one of these, and everything that depends on
/// *who* follows from it rather than being set beside it: the capabilities the
/// globals are checked against, the delegated scopes, and the management
/// surface. The last is the reason this is one value. The capability check
/// under `engine.call` is not enough on its own, because two kinds of
/// execution hold capabilities that were never a person's — the engine's own
/// actors, which hold what their script may do, and contained code such as
/// `sandbox.run`, whose capability subset cannot express "console but not
/// `read_logs` on any script". Deciding `engine` per site let a site get it
/// wrong; deciding it from the principal does not.
#[derive(Debug, Clone)]
pub enum Principal {
    /// Someone who presented a credential — an HTTP request or an MCP call,
    /// anonymous included. `engine` is installed and authorized against them,
    /// as `/engine/*` and `/mcp` would be.
    Caller(UserContext),
    /// A person who delegated background work
    /// ([`crate::delegation`]). `engine` is installed, and what they did not
    /// grant is not reachable: a scope not listed is not granted.
    Delegated {
        user: UserContext,
        scopes: Vec<crate::delegation::Scope>,
    },
    /// The caller's own authority running code that is not a handler serving
    /// them: a test run, an evaluation, `sandbox.run`. No `engine`, so
    /// `run_tests` and `eval_script` are not a way to spend whatever the
    /// caller holds on tools nobody named, and model-authored code cannot
    /// reach the management surface through a capability it was granted for
    /// something else.
    Contained(UserContext),
    /// Nobody: the engine itself — startup, `init()`, a scheduled job, a task
    /// nobody delegated. It holds what the script may do to its own things
    /// ([`UserContext::engine_actor`]) and gets no `engine`: nothing here came
    /// from a credential, so nothing here administers.
    Engine(&'static str),
}

impl Principal {
    /// The context the globals' capability checks are made against.
    pub fn user_context(&self) -> UserContext {
        match self {
            Self::Caller(user) | Self::Contained(user) => user.clone(),
            Self::Delegated { user, .. } => user.clone(),
            Self::Engine(label) => UserContext::engine_actor((*label).to_string()),
        }
    }

    /// The name an audit event records this principal under.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Caller(_) => "caller",
            Self::Delegated { .. } => "delegated",
            Self::Contained(_) => "contained",
            Self::Engine(_) => "engine",
        }
    }

    /// Whether the `engine` global is installed.
    pub fn reaches_engine_api(&self) -> bool {
        matches!(self, Self::Caller(_) | Self::Delegated { .. })
    }
}

impl GlobalSecurityConfig {
    /// A context acting for `principal` that registers nothing and writes its
    /// output under `log_context`. Registration and capture are opted into by
    /// overriding the fields.
    pub fn new(principal: Principal, log_context: repository::LogContext) -> Self {
        Self {
            // Fail closed: a caller that does not opt in cannot mutate
            // registries that outlive its own invocation.
            registration_phase: false,
            enable_audit_logging: false,
            dry_run_sink: None,
            console_sink: None,
            log_context,
            client_ip: None,
            principal,
        }
    }

    /// Whether this execution is acting on somebody's behalf.
    ///
    /// The question is not "is there a user" — an ordinary request has one too.
    /// It is whether the user is *absent*, which is what makes a grant the
    /// only authority for touching anything of theirs.
    pub fn is_delegated(&self) -> bool {
        matches!(self.principal, Principal::Delegated { .. })
    }

    /// Whether `scope` may be exercised here.
    ///
    /// True for every execution that is not delegated, because the person is
    /// present and acting for themselves. For a delegated one it is exactly
    /// what they ticked.
    pub fn allows_delegated(&self, scope: crate::delegation::Scope) -> bool {
        match &self.principal {
            Principal::Delegated { scopes, .. } => scopes.contains(&scope),
            _ => true,
        }
    }

    /// Record `registration` and return the reply to give JavaScript, or `None`
    /// when this context registers for real and the caller should carry on to
    /// the live registry.
    ///
    /// Call this *after* the validation and capability checks of the API it
    /// guards, so a dry run reports the same refusals a real registration
    /// would, and immediately before the registry write, so nothing that
    /// outlives the run has happened yet.
    pub(super) fn collect(&self, registration: CollectedRegistration) -> Option<String> {
        let sink = self.dry_run_sink.as_ref()?;
        let reply = format!(
            "{}: '{}' checked but not registered - this is a dry run",
            registration.kind.api(),
            registration.name
        );
        if let Ok(mut collected) = sink.lock() {
            collected.push(registration);
        }
        Some(reply)
    }

    /// Keep a refused registration where the check can report it. A no-op
    /// outside a dry run.
    pub(super) fn note_dry_run_refusal(&self, registration: CollectedRegistration) {
        if let Some(sink) = self.dry_run_sink.as_ref()
            && let Ok(mut collected) = sink.lock()
        {
            collected.push(registration);
        }
    }

    /// True when registration calls are being recorded rather than applied.
    pub(super) fn is_dry_run(&self) -> bool {
        self.dry_run_sink.is_some()
    }

    /// Record one `console` line if this context is capturing.
    pub(super) fn capture_console(&self, level: &str, message: &str) {
        let Some(sink) = self.console_sink.as_ref() else {
            return;
        };
        let Ok(mut capture) = sink.lock() else {
            return;
        };
        if capture.lines.len() >= MAX_CAPTURED_CONSOLE_LINES {
            capture.dropped += 1;
            return;
        }
        capture.lines.push(ConsoleLine {
            level: level.to_string(),
            message: message.to_string(),
            timestamp_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|since| since.as_millis() as u64)
                .unwrap_or_default(),
        });
    }
}

impl SecureGlobalContext {
    /// The globals for one execution, checked against `config.principal`.
    pub fn new(config: GlobalSecurityConfig) -> Self {
        let pool = crate::database::get_global_database().map(|db| db.pool().clone());

        Self {
            user_context: config.principal.user_context(),
            auditor: SecurityAuditor::new(pool),
            config,
        }
    }
}

impl SecureGlobalContext {
    /// Setup all secure global functions in the JavaScript context
    pub fn setup_secure_globals<'js>(
        &self,
        ctx: &'js rquickjs::Ctx<'js>,
        script_uri: &str,
    ) -> JsResult<()> {
        self.setup_secure_functions(ctx, script_uri, None)
    }
}

impl SecureGlobalContext {
    /// Setup secure global functions with optional route registration function
    pub fn setup_secure_functions(
        &self,
        ctx: &rquickjs::Ctx<'_>,
        script_uri: &str,
        register_fn: Option<RouteRegisterFn>,
    ) -> JsResult<()> {
        // Every global below is installed in every execution context. A script's
        // API surface must not depend on how it was entered: the same helper
        // may be reached from an HTTP handler, a scheduled job and a message
        // listener, and `typeof x === "undefined"` guards are not something
        // solution developers should have to write. Where an operation is
        // meaningless outside the registration phase, the method is still
        // present and still callable - see `registration_inactive`.
        self.setup_route_registry(ctx, script_uri, register_fn)?;
        self.setup_logging_functions(ctx, script_uri)?;
        self.setup_asset_management_functions(ctx, script_uri)?;
        self.setup_secrets_functions(ctx, script_uri)?;
        self.setup_fetch_function(ctx, script_uri)?;
        self.setup_database_functions(ctx, script_uri)?;
        self.setup_conversion_functions(ctx, script_uri)?;
        self.setup_script_properties_functions(ctx, script_uri)?;
        self.setup_user_properties_functions(ctx, script_uri)?;
        self.setup_mcp_functions(ctx, script_uri)?;
        self.setup_scheduler_functions(ctx, script_uri)?;
        self.setup_task_functions(ctx, script_uri)?;
        self.setup_sandbox_functions(ctx, script_uri)?;
        self.setup_tools_functions(ctx, script_uri)?;
        self.setup_crypto_object(ctx, script_uri)?;
        self.setup_rate_limit_object(ctx, script_uri)?;
        self.setup_audit_object(ctx, script_uri)?;
        self.setup_engine_object(ctx, script_uri)?;

        // Setup JSX factory functions for server-side HTML generation
        self.setup_jsx_functions(ctx)?;

        Ok(())
    }
}

#[cfg(test)]
mod api_surface_tests {
    use super::*;
    use rquickjs::{Context, Runtime};

    /// Evaluate `expr` against the globals a handler sees outside the
    /// registration phase — an HTTP handler, a scheduled job, a message
    /// listener or a test all get this surface.
    fn eval_outside_registration_phase(expr: &str) -> String {
        let rt = Runtime::new().expect("runtime");
        let ctx = Context::full(&rt).expect("context");
        ctx.with(|ctx| {
            let config = GlobalSecurityConfig::new(
                Principal::Engine("t"),
                repository::LogContext::default(),
            );
            let context = SecureGlobalContext::new(config);
            context
                .setup_secure_functions(&ctx, "test://script", None)
                .expect("install globals");
            ctx.eval::<String, _>(expr).expect("eval")
        })
    }

    /// The whole API surface is present regardless of how a script was entered,
    /// so shared helpers never need `typeof x === "undefined"` guards.
    #[test]
    fn every_global_is_installed_outside_the_registration_phase() {
        for global in [
            "routeRegistry",
            "files",
            "scriptStorage",
            "personalStorage",
            "secretStorage",
            "schedulerService",
            "scriptTasks",
            "personalTasks",
            "mcpRegistry",
            "database",
            "console",
            "convert",
            "McpClient",
            "fetch",
            "tools",
        ] {
            assert_eq!(
                eval_outside_registration_phase(&format!("typeof {}", global)),
                if global == "fetch" || global == "McpClient" {
                    "function"
                } else {
                    "object"
                },
                "global `{}` is missing outside the registration phase",
                global
            );
        }
    }

    /// Registration methods stay callable everywhere. They must not throw: a
    /// script's top-level program re-runs on every invocation, so a script that
    /// registers at top level rather than in `init()` would fail on every
    /// request if these raised.
    #[test]
    fn registration_methods_report_instead_of_throwing_or_registering() {
        // Every registration answers with a value rather than a sentence:
        // outside the registration phase the answer is `{ ok: false, reason }`.
        for (call, subject) in [
            ("routeRegistry.registerRoute('/r', { handler: 'h' })", "/r"),
            ("routeRegistry.registerRoute('/s', { stream: true })", "/s"),
            ("routeRegistry.registerRoute('/a', { file: 'a.txt' })", "/a"),
            (
                "mcpRegistry.registerTool('t', { description: 'd', inputSchema: {}, handler: 'h' })",
                "t",
            ),
            (
                "mcpRegistry.registerPrompt('p', { description: 'd', arguments: [], handler: 'h' })",
                "p",
            ),
            (
                "schedulerService.registerOnce({ handler: 'h', runAt: '2030-01-01T00:00:00Z' })",
                "h",
            ),
            (
                "schedulerService.registerRecurring({ handler: 'h', intervalMinutes: 5 })",
                "h",
            ),
        ] {
            let result = eval_outside_registration_phase(&format!(
                "(function (r) {{ return r.ok + '|' + r.reason; }})({})",
                call
            ));
            assert!(
                result.starts_with("false|")
                    && result.contains("not registered")
                    && result.contains(subject),
                "`{}` should report that it did not register, got: {}",
                call,
                result
            );
        }
    }

    /// The arguments the type declarations type `any` reach the host as JSON.
    ///
    /// QuickJS does not coerce an object to a `String`, so a binding taking one
    /// would raise `TypeError` on the objects the declarations pass. The tests
    /// are here rather than around the host functions because it is the
    /// JavaScript surface that would break.
    #[test]
    fn fetch_serializes_the_options_object_it_documents() {
        // `__hostFetch` is replaced so this tests the marshalling and not the
        // network: the prelude looks the name up on each call.
        let seen = eval_outside_registration_phase(
            r#"(function () {
                 var seen = null;
                 globalThis.__hostFetch = function (url, options) {
                   seen = options;
                   return JSON.stringify({ status: 200, ok: true, headers: {}, body: "" });
                 };
                 var response = fetch("https://example.com/x", {
                   method: "POST",
                   headers: { "Content-Type": "application/json" },
                   body: "{}",
                 });
                 return typeof seen + "|" + seen + "|" + response.status;
               })()"#,
        );
        assert!(
            seen.starts_with("string|"),
            "options should reach the host as JSON text, got: {}",
            seen
        );
        assert!(
            seen.contains(r#""method":"POST""#) && seen.ends_with("|200"),
            "the options the script wrote should be the options the host sees, got: {}",
            seen
        );
    }

    /// A script written against the host call sends JSON text already, and it
    /// must not be re-encoded into a quoted string.
    #[test]
    fn fetch_passes_a_string_of_options_through_untouched() {
        let seen = eval_outside_registration_phase(
            r#"(function () {
                 var seen = null;
                 globalThis.__hostFetch = function (url, options) {
                   seen = options;
                   return JSON.stringify({ status: 200, ok: true, headers: {}, body: "" });
                 };
                 fetch("https://example.com/x", JSON.stringify({ method: "PUT" }));
                 return seen;
               })()"#,
        );
        assert_eq!(seen, r#"{"method":"PUT"}"#);
    }

    /// Omitting the options must stay omitted rather than becoming `"null"`,
    /// which the host would fail to parse as `FetchOptions`.
    #[test]
    fn fetch_without_options_hands_the_host_nothing() {
        let seen = eval_outside_registration_phase(
            r#"(function () {
                 var seen = "not called";
                 globalThis.__hostFetch = function (url, options) {
                   seen = typeof options;
                   return JSON.stringify({ status: 200, ok: true, headers: {}, body: "" });
                 };
                 fetch("https://example.com/x");
                 return seen;
               })()"#,
        );
        assert_eq!(seen, "undefined");
    }

    /// The stream and dispatch calls take the object their examples pass. The
    /// assertion is that they answer at all: a conversion failure throws out
    /// of the binding before any of these can report anything.
    // The stream binding files an audit event on a spawned task, so this one
    // needs a runtime where the others do not.
    #[tokio::test]
    async fn the_data_arguments_take_the_object_their_examples_pass() {
        for call in [
            "routeRegistry.sendStreamMessage('/events/x', { type: 'alert', n: 1 })",
            "routeRegistry.sendStreamMessageFiltered('/events/x', { type: 'alert' }, \
             { role: 'admin' })",
        ] {
            // The call is wrapped in JavaScript so a refusal comes back as
            // text: what is being asserted is which refusal it is, and an
            // exception out of `eval` would take that with it.
            let result = eval_outside_registration_phase(&format!(
                "(function () {{ try {{ return JSON.stringify({}); }} \
                 catch (e) {{ return 'threw: ' + e; }} }})()",
                call
            ));
            assert!(
                !result.starts_with("threw: "),
                "`{}` should serialize its data rather than refusing it, got: {}",
                call,
                result
            );
        }
    }

    /// Argument validation is context-independent: a malformed call is reported
    /// the same way wherever it is made, rather than being masked by the phase.
    #[test]
    fn argument_validation_runs_before_the_phase_check() {
        let thrown = eval_outside_registration_phase(
            "(function () { try { routeRegistry.registerRoute('no-slash', { stream: true }); \
             return 'did not throw'; } catch (e) { return e.name + ': ' + e.message; } })()",
        );
        assert!(
            thrown.starts_with("TypeError: ") && thrown.contains("must start with"),
            "a malformed path is a mistake in the call, got: {}",
            thrown
        );
    }
}
