//! What a script registered: route kinds and the metadata the route index is built from.

use std::collections::HashMap;

/// What a route points at.
///
/// A script publishes three kinds of thing on a path, and they are one kind of
/// registration — one index, one invalidation path — differing in what the
/// engine does once the path matches: run a handler, open a stream, or send a
/// file.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RouteKind {
    /// Run the script's handler, which decides everything including who may
    /// call it — it runs under the requesting user's context.
    #[default]
    Handler,
    /// Send one of the script's files. The engine moves the bytes, so
    /// [`RouteMetadata::authorize`] is where "who may read this" lives.
    File,
    /// Open a Server-Sent Events connection. The engine holds the socket, so
    /// [`RouteMetadata::authorize`] is again where the decision lives — and
    /// it returns the connection's filter criteria along with it.
    Stream,
}

/// The pseudo-method a stream route is keyed under, for the reason
/// [`ASSET_METHOD`] gives.
pub const STREAM_METHOD: &str = "STREAM";

/// The pseudo-method a file route is keyed under.
///
/// Not `GET`, although that is the method it answers. Keying it as `GET`
/// would put it in the same slot as a handler on the same path, and the
/// index would silently keep whichever registered last; keeping it distinct
/// lets [`crate::route_index::find_route_handler`] state the precedence in
/// one place instead of it being an accident of the order three registries
/// happened to be consulted in.
pub const ASSET_METHOD: &str = "ASSET";

/// OpenAPI metadata for a registered route
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RouteMetadata {
    /// The handler to run. Empty for a route that is not a handler.
    pub handler_name: String,
    /// What the engine does when this path matches.
    #[serde(default)]
    pub kind: RouteKind,
    /// The file this route serves, for [`RouteKind::File`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// The function that decides who may read this, for a route the engine
    /// answers without running a handler. `None` means anyone who can reach
    /// the host, which is what a handler that checks nothing also means.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub authorize: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parameters: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "requestBody")]
    pub request_body: Option<serde_json::Value>,
}

impl RouteMetadata {
    pub fn simple(handler_name: String) -> Self {
        Self {
            handler_name,
            kind: RouteKind::Handler,
            file: None,
            authorize: None,
            summary: None,
            description: None,
            tags: Vec::new(),
            parameters: None,
            request_body: None,
        }
    }

    /// A route that opens a Server-Sent Events connection.
    pub fn stream(authorize: Option<String>) -> Self {
        Self {
            handler_name: String::new(),
            kind: RouteKind::Stream,
            file: None,
            authorize,
            summary: None,
            description: None,
            tags: Vec::new(),
            parameters: None,
            request_body: None,
        }
    }

    /// A route that sends one of the script's files.
    pub fn file(path: String, authorize: Option<String>) -> Self {
        Self {
            handler_name: String::new(),
            kind: RouteKind::File,
            file: Some(path),
            authorize,
            summary: None,
            description: None,
            tags: Vec::new(),
            parameters: None,
            request_body: None,
        }
    }

    /// What this route points at, for a listing: the handler, or the file.
    pub fn target(&self) -> &str {
        match self.kind {
            RouteKind::Handler => &self.handler_name,
            RouteKind::File => self.file.as_deref().unwrap_or_default(),
            // A stream has no target beyond the path; what a listing wants to
            // show for it is the function that decides who may subscribe.
            RouteKind::Stream => self.authorize.as_deref().unwrap_or_default(),
        }
    }

    /// The default Swagger group for a route of this kind.
    pub fn default_tag(&self) -> &'static str {
        match self.kind {
            RouteKind::Handler => "Scripts",
            RouteKind::File => "Assets",
            RouteKind::Stream => "Streams",
        }
    }
}

/// Route registration: (path, method) -> RouteMetadata
pub type RouteRegistrations = HashMap<(String, String), RouteMetadata>;
