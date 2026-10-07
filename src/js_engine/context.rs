//! What a handler is handed: the request, the invocation kind and the handler
//! context.

use crate::repository;
use crate::security::UserContext;
use serde_json::Value as JsonValue;
use std::collections::HashMap;

/// Parameters for secure script execution in request context
#[derive(Debug)]
pub struct RequestExecutionParams {
    pub script_uri: String,
    pub handler_name: String,
    pub path: String,
    pub method: String,
    pub query_params: Option<HashMap<String, String>>,
    /// Absolute URL of the request, origin included. See [`JsRequestContext::url`].
    pub url: Option<String>,
    pub form_data: Option<HashMap<String, String>>,
    pub raw_body: Option<String>,
    pub headers: HashMap<String, String>,
    pub user_context: UserContext,
    /// Optional OAuth authentication context for JavaScript auth API
    pub auth_context: Option<crate::auth::JsAuthContext>,
    /// Route parameters extracted from path patterns like /users/:id
    pub route_params: Option<HashMap<String, String>>,
    /// Uploaded files from multipart form data
    pub uploaded_files: Option<Vec<crate::parsers::UploadedFile>>,
    /// The request's `x-request-id`, if it came through the HTTP stack. Every
    /// log line the handler writes is filed under it, so a caller holding the
    /// response header can ask for exactly the lines its own call produced.
    pub request_id: Option<String>,
    /// The registered route pattern that matched (`/things/:id`), as opposed to
    /// the concrete `path`. Filtering logs by it aggregates every call to the
    /// handler instead of splitting them per parameter value.
    pub route_pattern: Option<String>,
}

/// Kinds of handler invocations supported by the runtime.
#[derive(Debug, Clone, Copy)]
pub enum HandlerInvocationKind {
    HttpRoute,
    StreamCustomization,
    /// The function that decides whether a caller may read a file the engine
    /// serves directly. Its sibling is `StreamCustomization`: both answer the
    /// question a handler would have answered, on a surface where no handler
    /// runs.
    AssetAuthorization,
    Init,
    Scheduled,
    McpTool,
    /// An MCP prompt handler. Unlike the other kinds it is never given a
    /// handler context object, but its output is still attributable.
    McpPrompt,
    Test,
    /// An ad hoc snippet run against a script's sandbox by `/engine/eval_script`.
    Eval,
}

impl HandlerInvocationKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            HandlerInvocationKind::HttpRoute => "httpRoute",
            HandlerInvocationKind::StreamCustomization => "streamCustomization",
            HandlerInvocationKind::AssetAuthorization => "assetAuthorization",
            HandlerInvocationKind::Init => "init",
            HandlerInvocationKind::Scheduled => "scheduled",
            HandlerInvocationKind::McpTool => "mcpTool",
            HandlerInvocationKind::McpPrompt => "mcpPrompt",
            HandlerInvocationKind::Test => "test",
            HandlerInvocationKind::Eval => "eval",
        }
    }

    /// Attribute a script's log output to one invocation of this kind.
    ///
    /// `route` names what was being served — the registered route pattern for
    /// an HTTP route, otherwise the job, stream or tool name — so that
    /// filtering by it collects every run of the same handler rather than one
    /// concrete path per parameter value.
    pub fn log_context(
        self,
        script_uri: &str,
        invocation_id: impl Into<String>,
        route: Option<String>,
    ) -> repository::LogContext {
        repository::LogContext {
            request_id: Some(invocation_id.into()),
            kind: Some(self.as_str().to_string()),
            route,
            // Read once, here, rather than per line: an invocation that spans
            // a write still produced all of its output from the version it
            // started under. `script_uri` is a parameter rather than something
            // this resolves later so a call site cannot forget it — an
            // unattributed line is indistinguishable from one written before
            // revisions existed.
            revision: crate::deployments::serving_revision(script_uri),
        }
    }
}

/// Normalized view of inbound request data passed to JavaScript.
#[derive(Debug, Clone, Default)]
pub struct JsRequestContext {
    pub path: Option<String>,
    /// The absolute URL the request arrived on, origin included.
    ///
    /// `path` alone cannot say which host served a request, and the engine
    /// serves several. This is also where the prelude reads the raw query
    /// string from, which is the only place duplicate parameters survive.
    pub url: Option<String>,
    pub method: Option<String>,
    pub headers: HashMap<String, String>,
    pub query_params: HashMap<String, String>,
    pub form_data: HashMap<String, String>,
    pub body: Option<String>,
    /// Route parameters extracted from path patterns like /users/:id
    pub route_params: HashMap<String, String>,
    /// Uploaded files from multipart form data
    pub uploaded_files: Vec<crate::parsers::UploadedFile>,
}

/// Builder that assembles the single context object passed to all handlers.
#[derive(Debug, Clone)]
pub struct JsHandlerContextBuilder {
    pub(super) kind: HandlerInvocationKind,
    pub(super) script_uri: Option<String>,
    pub(super) handler_name: Option<String>,
    pub(super) request: Option<JsRequestContext>,
    pub(super) args: Option<JsonValue>,
    pub(super) auth_context: Option<crate::auth::JsAuthContext>,
    pub(super) connection_metadata: Option<HashMap<String, String>>,
    pub(super) metadata: HashMap<String, JsonValue>,
    pub(super) invocation_id: Option<String>,
}

impl JsHandlerContextBuilder {
    pub fn new(kind: HandlerInvocationKind) -> Self {
        Self {
            kind,
            script_uri: None,
            handler_name: None,
            request: None,
            args: None,
            auth_context: None,
            connection_metadata: None,
            metadata: HashMap::new(),
            invocation_id: None,
        }
    }

    /// Identify this invocation to the script, so a handler can echo the id
    /// that its log lines are filed under into a response or an error report.
    pub fn with_invocation_id(mut self, invocation_id: impl Into<String>) -> Self {
        self.invocation_id = Some(invocation_id.into());
        self
    }

    pub fn with_script_metadata(
        mut self,
        script_uri: impl Into<String>,
        handler: impl Into<String>,
    ) -> Self {
        self.script_uri = Some(script_uri.into());
        self.handler_name = Some(handler.into());
        self
    }

    pub fn with_request(mut self, request: JsRequestContext) -> Self {
        self.request = Some(request);
        self
    }

    pub fn with_args(mut self, args: JsonValue) -> Self {
        self.args = Some(args);
        self
    }

    pub fn with_auth_context(mut self, auth_ctx: crate::auth::JsAuthContext) -> Self {
        self.auth_context = Some(auth_ctx);
        self
    }

    pub fn with_connection_metadata(mut self, metadata: HashMap<String, String>) -> Self {
        self.connection_metadata = Some(metadata);
        self
    }

    pub fn with_metadata_value(mut self, key: &str, value: JsonValue) -> Self {
        self.metadata.insert(key.to_string(), value);
        self
    }

    pub(super) fn build_request_object<'js>(
        request: Option<JsRequestContext>,
        auth_context: Option<crate::auth::JsAuthContext>,
        ctx: &rquickjs::Ctx<'js>,
    ) -> Result<Option<rquickjs::Object<'js>>, rquickjs::Error> {
        let Some(request) = request else {
            return Ok(None);
        };

        let request_obj = rquickjs::Object::new(ctx.clone())?;

        if let Some(path) = &request.path {
            request_obj.set("path", path)?;
        }
        if let Some(url) = &request.url {
            request_obj.set("url", url.as_str())?;
        }
        if let Some(method) = &request.method {
            request_obj.set("method", method)?;
        }

        // Headers
        if !request.headers.is_empty() {
            let headers_obj = rquickjs::Object::new(ctx.clone())?;
            for (name, value) in &request.headers {
                headers_obj.set(name.as_str(), value.as_str())?;
            }
            request_obj.set("headers", headers_obj)?;
        }

        // Query params
        let query_obj = rquickjs::Object::new(ctx.clone())?;
        for (key, value) in &request.query_params {
            query_obj.set(key.as_str(), value.as_str())?;
        }
        request_obj.set("query", query_obj)?;

        // Form data
        let form_obj = rquickjs::Object::new(ctx.clone())?;
        for (key, value) in &request.form_data {
            form_obj.set(key.as_str(), value.as_str())?;
        }
        request_obj.set("form", form_obj)?;

        // Route params
        let route_obj = rquickjs::Object::new(ctx.clone())?;
        for (key, value) in &request.route_params {
            route_obj.set(key.as_str(), value.as_str())?;
        }
        request_obj.set("params", route_obj)?;

        // Uploaded files (base64-encoded data)
        let files_array = rquickjs::Array::new(ctx.clone())?;
        for (idx, file) in request.uploaded_files.iter().enumerate() {
            let file_obj = rquickjs::Object::new(ctx.clone())?;
            file_obj.set("field", file.field_name.as_str())?;
            if let Some(ref filename) = file.filename {
                file_obj.set("filename", filename.as_str())?;
            }
            if let Some(ref content_type) = file.content_type {
                file_obj.set("contentType", content_type.as_str())?;
            }
            // Encode file data as base64 for JavaScript
            let base64_data =
                base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &file.data);
            file_obj.set("data", base64_data)?;
            file_obj.set("size", file.size as u32)?;
            files_array.set(idx, file_obj)?;
        }
        request_obj.set("files", files_array)?;

        // Body
        if let Some(body) = &request.body {
            request_obj.set("body", body.as_str())?;
        } else {
            request_obj.set("body", rquickjs::Value::new_null(ctx.clone()))?;
        }

        if let Some(auth_ctx) = auth_context {
            let auth_obj = crate::auth::AuthJsApi::create_auth_object(ctx, auth_ctx.clone())?;
            request_obj.set("auth", auth_obj)?;
        }

        // The request prelude gives this object the methods a handler expects of
        // one — `text()`, `json()`, a `Headers` that does not care how the
        // client capitalised a name. It is absent only in a context built
        // before globals were installed, where the plain object is still fine.
        let globals = ctx.globals();
        if let Ok(enhance) = globals.get::<_, rquickjs::Function>("__enhanceRequest") {
            let enhanced: rquickjs::Object = enhance.call((request_obj.clone(),))?;
            return Ok(Some(enhanced));
        }

        Ok(Some(request_obj))
    }

    pub fn build<'js>(
        self,
        ctx: &rquickjs::Ctx<'js>,
    ) -> Result<rquickjs::Object<'js>, rquickjs::Error> {
        let JsHandlerContextBuilder {
            kind,
            script_uri,
            handler_name,
            request,
            args,
            auth_context,
            connection_metadata,
            metadata,
            invocation_id,
        } = self;

        let request_obj = Self::build_request_object(request, auth_context, ctx)?;

        let context_obj = rquickjs::Object::new(ctx.clone())?;
        context_obj.set("kind", kind.as_str())?;

        // The id this invocation's log lines are filed under; `/engine/read_logs`
        // takes it as `request_id`.
        if let Some(invocation_id) = invocation_id {
            context_obj.set("invocationId", invocation_id)?;
        }

        if let Some(script_uri) = script_uri {
            context_obj.set("scriptUri", script_uri)?;
        }

        if let Some(handler_name) = handler_name {
            context_obj.set("handlerName", handler_name)?;
        }

        // Ensure there's always a request object with at least an empty query object
        // This provides "query object guarantees" so scripts can safely access context.request.query
        if let Some(request_obj) = request_obj {
            context_obj.set("request", request_obj)?;
        } else {
            // Create a minimal request object with empty query for non-HTTP handlers
            let minimal_request = rquickjs::Object::new(ctx.clone())?;
            let empty_query = rquickjs::Object::new(ctx.clone())?;
            minimal_request.set("query", empty_query)?;
            context_obj.set("request", minimal_request)?;
        }

        if let Some(args) = args {
            let args_value = serde_json_to_js_value(ctx, &args)?;
            context_obj.set("args", args_value)?;
        } else {
            context_obj.set("args", rquickjs::Value::new_null(ctx.clone()))?;
        }

        if let Some(metadata) = connection_metadata {
            let metadata_obj = rquickjs::Object::new(ctx.clone())?;
            for (key, value) in metadata {
                metadata_obj.set(key.as_str(), value.as_str())?;
            }
            context_obj.set("connectionMetadata", metadata_obj)?;
        }

        if !metadata.is_empty() {
            let meta_obj = rquickjs::Object::new(ctx.clone())?;
            for (key, value) in metadata {
                let js_value = serde_json_to_js_value(ctx, &value)?;
                meta_obj.set(key.as_str(), js_value)?;
            }
            context_obj.set("meta", meta_obj)?;
        }

        Ok(context_obj)
    }
}

pub(super) fn serde_json_to_js_value<'js>(
    ctx: &rquickjs::Ctx<'js>,
    value: &JsonValue,
) -> Result<rquickjs::Value<'js>, rquickjs::Error> {
    let json_string = serde_json::to_string(value).map_err(|e| {
        let msg = format!("Failed to serialize JSON value: {}", e);
        rquickjs::Error::new_from_js("JSON", Box::leak(msg.into_boxed_str()))
    })?;

    let json_obj: rquickjs::Object = ctx.globals().get("JSON")?;
    let json_parse: rquickjs::Function = json_obj.get("parse")?;
    let js_value: rquickjs::Value = json_parse.call((json_string,))?;
    Ok(js_value)
}
