//! What a script registers, collected for a dry run or for the route index.

use crate::repository;

/// Where a registration lands.
///
/// `Rc` rather than `Box` because more than one registry function records
/// through it: a route and a file route are one kind of registration
/// differing only in what the engine does once the path matches, so they
/// share one sink rather than each owning a registry.
pub(super) type RouteRegisterFn = std::rc::Rc<
    dyn Fn(&str, &repository::RouteMetadata, Option<&str>) -> Result<(), rquickjs::Error>,
>;

/// Which registry a [`CollectedRegistration`] would have been written to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub enum RegistrationKind {
    Route,
    Stream,
    AssetRoute,
    McpTool,
    McpPrompt,
    McpResource,
    ScheduledJob,
}

impl RegistrationKind {
    /// The registering API's name, for messages that have to say what was
    /// skipped.
    pub fn api(self) -> &'static str {
        match self {
            RegistrationKind::Route | RegistrationKind::Stream | RegistrationKind::AssetRoute => {
                "routeRegistry.registerRoute"
            }
            RegistrationKind::McpTool => "mcpRegistry.registerTool",
            RegistrationKind::McpPrompt => "mcpRegistry.registerPrompt",
            RegistrationKind::McpResource => "mcpRegistry.registerResource",
            RegistrationKind::ScheduledJob => "schedulerService",
        }
    }
}

/// One registration a script made, recorded instead of applied.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CollectedRegistration {
    pub kind: RegistrationKind,
    /// What the registration is keyed by: a path, an operation name, a tool
    /// name, a message type, or a scheduled job's key.
    pub name: String,
    /// HTTP method, for the registrations that have one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// The script function the engine would call. `None` where a registration
    /// names no delegate — an asset route serves bytes, not code, and a stream
    /// without a customization function has nothing to call.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handler: Option<String>,
    /// Why the engine refused it, when it did. A refusal is answered to the
    /// script as a value it may not read, so a dry run keeps it here for the
    /// check to report; a refused registration is not part of what deploys.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refusal: Option<String>,
}

impl CollectedRegistration {
    pub fn new(kind: RegistrationKind, name: impl Into<String>) -> Self {
        Self {
            kind,
            name: name.into(),
            method: None,
            handler: None,
            refusal: None,
        }
    }

    pub fn with_refusal(mut self, reason: impl Into<String>) -> Self {
        self.refusal = Some(reason.into());
        self
    }

    pub fn with_method(mut self, method: impl Into<String>) -> Self {
        self.method = Some(method.into());
        self
    }

    pub fn with_handler(mut self, handler: impl Into<String>) -> Self {
        self.handler = Some(handler.into());
        self
    }
}

/// Where a dry run's registrations accumulate.
///
/// `Arc<Mutex<_>>` rather than `Rc<RefCell<_>>` because it is held in
/// [`GlobalSecurityConfig`], which callers build outside the JavaScript context
/// and move in; the contention is nil either way, since only the one QuickJS
/// thread ever touches it.
pub type RegistrationSink = std::sync::Arc<std::sync::Mutex<Vec<CollectedRegistration>>>;
