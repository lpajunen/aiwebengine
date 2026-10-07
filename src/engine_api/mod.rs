//! Native engine management API for scripts and assets.
//!
//! These REST routes and MCP tools are engine functionality, so they live in
//! Rust. They are the only way to administer scripts, assets, users, secrets
//! and logs: the JavaScript sandbox exposes no engine-management API, and every
//! call here is authorized against the calling user's capabilities, ownership
//! of the target script, and role.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use serde_json::{Value, json};

use crate::auth::AuthUser;
use crate::repository;
use crate::security::{Capability, SecurityAuditor, UserContext};
mod checks;
mod files;
mod git;
mod history;
mod hosts;
mod introspection;
mod logs;
mod operations;
mod pages;
mod script_settings;
mod scripts;
mod search;
mod secrets;
mod users;
pub use checks::*;
pub use files::*;
pub use git::*;
pub use history::*;
pub use hosts::*;
pub use introspection::*;
pub use logs::*;
pub use operations::*;
pub use pages::*;
pub use script_settings::*;
pub use scripts::*;
pub use search::*;
pub use secrets::*;
pub use users::*;

/// Maximum asset size accepted by the write paths (same limit as the sandbox).
pub const MAX_ASSET_BYTES: usize = 10 * 1024 * 1024;

/// Maximum number of files one batch write may carry.
pub const MAX_BATCH_FILES: usize = 256;

/// Maximum total decoded size of one batch write. The per-file ceiling still
/// applies to each file, so a batch cannot smuggle in an oversized asset.
pub const MAX_BATCH_BYTES: usize = MAX_ASSET_BYTES;

/// Request body ceiling for the batch route: the decoded ceiling, plus the
/// third base64 adds, plus room for the JSON envelope around it.
///
/// The management router's own limit is `security.max_request_body_bytes`,
/// which defaults to 1MB — far below what a batch of source files needs — so
/// the batch route overrides it (see `lib.rs`).
pub const MAX_BATCH_BODY_BYTES: usize = MAX_BATCH_BYTES * 4 / 3 + 1024 * 1024;

/// Request body ceiling for the single-asset routes, on the same reasoning as
/// [`MAX_BATCH_BODY_BYTES`] and for a failure the batch route was spared by
/// having one.
///
/// An asset's content travels base64-encoded, so a body limit bounds three
/// quarters of an asset. Inheriting the router's `max_request_body_bytes` made
/// the largest asset writable here three quarters of that — 786 KB against the
/// 1 MB default, 7.86 MB against the 10 MB a deployment ships — while
/// [`MAX_ASSET_BYTES`], the sandbox and the published document all named
/// 10,000,000 bytes. The endpoint whose job is writing an asset was the one
/// path that could not write the largest one, and it answered with a 413 about
/// a body rather than anything about the limit it was failing.
pub const MAX_ASSET_BODY_BYTES: usize = MAX_ASSET_BYTES * 4 / 3 + 64 * 1024;

/// Maximum number of edits one patch may carry.
pub const MAX_PATCH_EDITS: usize = 128;

/// How many matching lines a `grep=` read reports before it stops looking.
pub const MAX_GREP_MATCHES: usize = 200;

/// How much of a matching line a `grep=` read echoes back.
pub const MAX_GREP_LINE_CHARS: usize = 512;

/// Longest `grep=` pattern accepted, so a read cannot hand the regex engine an
/// arbitrarily large program to compile.
pub const MAX_GREP_PATTERN_CHARS: usize = 512;

fn iso_timestamp() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn user_context_from(auth_user: Option<&AuthUser>) -> UserContext {
    UserContext::for_session(auth_user.map(AuthUser::roles).unwrap_or_default())
}

fn auditor() -> SecurityAuditor {
    let pool = crate::database::get_global_database().map(|db| db.pool().clone());
    SecurityAuditor::new(pool)
}

fn user_owns_script(user: &UserContext, script_uri: &str) -> bool {
    match &user.user_id {
        Some(user_id) => repository::user_owns_script(script_uri, user_id).unwrap_or(false),
        None => false,
    }
}

/// Capability-only admin check: does this caller hold `AdministerEngine`?
///
/// **This does not mean the caller is an authenticated administrator.**
/// Capability alone. No tier grants `AdministerEngine` without a session any
/// more, so in practice this and [`is_user_admin`] agree — the two are kept
/// apart because they answer different questions, and the stricter one is what
/// anything exposing user data, engine topology or role changes should ask.
/// Whether a caller may reach the engine's administration surface at all.
///
/// Capabilities alone do not answer this. An anonymous caller holds
/// `ReadScripts` and `ReadAssets` on purpose: a script serving a public
/// request runs with the requesting user's context, and reads its own modules
/// and assets through the sandbox with those capabilities, so a public page
/// would stop working without them. Who may read a script's tree *through*
/// `/engine/*` is a different question, and this is where the two part —
/// otherwise an unauthenticated caller can list, download, and now search
/// every script's files, and a script that carries an embedded credential
/// carries it in public.
///
fn may_administer(user: &UserContext) -> bool {
    user.is_authenticated
}

fn has_admin_capability(user: &UserContext) -> bool {
    user.has_capability(&Capability::AdministerEngine)
}

fn is_admin_or_owner(user: &UserContext, script_uri: &str) -> bool {
    has_admin_capability(user) || user_owns_script(user, script_uri)
}

// ============================================================================
// REST routes (contracts identical to the core.js/cli.js handlers)
// ============================================================================

fn json_response(status: StatusCode, body: Value) -> Response {
    (
        status,
        [("content-type", "application/json")],
        body.to_string(),
    )
        .into_response()
}

fn error_response(status: StatusCode, message: String) -> Response {
    json_response(status, json!({ "error": message }))
}

#[cfg(test)]
mod tests;
