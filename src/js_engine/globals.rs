//! Installing every global for an execution, and the response builders.

use super::*;
use crate::repository;
use crate::security::secure_globals::{GlobalSecurityConfig, SecureGlobalContext};
use std::time::Instant;

/// Function type for registering functions in different execution contexts
/// Where a registration lands.
///
/// `Rc` rather than `Box` because more than one registry function records
/// through it: a route, a file route and (in time) a stream are one kind of
/// registration, so they share one sink rather than each owning a registry.
pub(super) type RegisterFunctionType = std::rc::Rc<
    dyn Fn(&str, &repository::RouteMetadata, Option<&str>) -> Result<(), rquickjs::Error>,
>;

/// Installs every global, each checked against `config.principal`.
///
/// The caller's authentication is not a global: it reaches the script as
/// `context.request.auth`, through the handler context.
pub(super) fn setup_secure_global_functions(
    ctx: &rquickjs::Ctx<'_>,
    script_uri: &str,
    config: GlobalSecurityConfig,
    register_fn: Option<RegisterFunctionType>,
) -> Result<(), rquickjs::Error> {
    let t = Instant::now();
    let secure_context = SecureGlobalContext::new(config);
    let d_ctor = t.elapsed();

    // Setup secure functions with proper capability validation
    let t = Instant::now();
    secure_context.setup_secure_functions(ctx, script_uri, register_fn)?;
    let d_native = t.elapsed();

    // Add Response builder helpers
    let t = Instant::now();
    setup_response_builders(ctx)?;
    let d_resp = t.elapsed();

    GLOBALS_BREAKDOWN.with(|b| b.set(Some((d_ctor, d_native, d_resp))));

    // Auth is not a global: it is attached to req.auth by the caller

    Ok(())
}

/// Sets up Response builder helpers for JavaScript handlers
///
/// Provides convenient methods for creating HTTP responses:
/// - Response.json(data, status) - JSON response
/// - Response.text(text, status) - Text response
/// - Response.html(html, status) - HTML response
/// - Response.error(status, message) - Error response
/// - Response.noContent() - 204 No Content
/// - Response.redirect(url) - 302 redirect
pub(super) fn setup_response_builders(ctx: &rquickjs::Ctx<'_>) -> Result<(), rquickjs::Error> {
    // Create the ResponseBuilder object with builder methods using JavaScript
    ctx.eval::<(), _>(
        r#"
        globalThis.ResponseBuilder = {
            json: function(data, status = 200) {
                const body = JSON.stringify(data);
                return {
                    status: status,
                    body: body,
                    contentType: "application/json"
                };
            },
            text: function(text, status = 200) {
                return {
                    status: status,
                    body: text,
                    contentType: "text/plain; charset=UTF-8"
                };
            },
            html: function(html, status = 200) {
                return {
                    status: status,
                    body: html,
                    contentType: "text/html; charset=UTF-8"
                };
            },
            error: function(status, message) {
                const body = JSON.stringify({ error: message });
                return {
                    status: status,
                    body: body,
                    contentType: "application/json"
                };
            },
            noContent: function() {
                return {
                    status: 204,
                    body: "",
                    contentType: ""
                };
            },
            redirect: function(url, status = 302) {
                return {
                    status: status,
                    body: "Redirecting to " + url,
                    contentType: "text/plain; charset=UTF-8",
                    headers: {
                        "Location": url
                    }
                };
            }
        };
        "#,
    )?;

    Ok(())
}
