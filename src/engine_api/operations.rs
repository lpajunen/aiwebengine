//! The operation table: every engine operation once, with its schema, and how a
//! refusal reports its kind. HTTP and MCP are generated from it.

use super::*;
use crate::security::UserContext;
use serde_json::{Value, json};

// ============================================================================
// Native MCP tools (names and result shapes identical to the script tools)
// ============================================================================

/// Descriptor of a native MCP tool for tools/list.
pub struct NativeToolDescriptor {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
}

pub(super) type NativeToolHandler = fn(&Value, &UserContext) -> Value;
pub(super) type NativeToolEntry = (&'static str, &'static str, fn() -> Value, NativeToolHandler);

pub(super) fn arg_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args.get(key).and_then(Value::as_str)
}

/// A tool call's `edits` argument, read the way the HTTP routes read theirs.
///
/// Going through [`StringEditBody`] rather than picking the fields out of the
/// JSON is what keeps an `edit_file` tool call and a `/engine/edit_file` of the
/// same edits from disagreeing about which of them is malformed.
pub(super) fn parse_tool_edits(edits: &[Value]) -> Result<Vec<StringEdit>, String> {
    let bodies: Vec<StringEditBody> = edits
        .iter()
        .enumerate()
        .map(|(index, edit)| {
            serde_json::from_value(edit.clone()).map_err(|e| format!("edits[{}]: {}", index, e))
        })
        .collect::<Result<_, _>>()?;
    prepare_edits(bodies)
}

/// Why an operation did not do what it was asked.
///
/// The operation table answers every caller — an MCP client, a script calling
/// `engine.call`, an HTTP request — with the same JSON, so a refusal is a value
/// like any other result. What HTTP needs on top is a status line, and it used
/// to be recovered by reading the message: a text that happened to contain
/// `not found` became a 404. A refusal now says what kind it is where it is
/// made, in a `status` field the HTTP layer reads, and the message is only
/// ever something for a person.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// The arguments are missing or malformed, or ask for something that
    /// cannot be done as stated.
    BadRequest,
    /// The caller may not do this.
    Forbidden,
    /// What the arguments name does not exist.
    NotFound,
    /// What is stored disagrees: it already exists, or it has moved on since
    /// the caller read it.
    Conflict,
    /// The operation ran out of time.
    TimedOut,
    /// Something went wrong that was not the caller's doing.
    Failed,
}

impl Refusal {
    /// The kind a refusal's *text* implies, for the errors the layers below
    /// still report as strings (`authorize_script_write`, the revision
    /// resolvers). It is the one place the engine reads a message to decide a
    /// status, kept so that the typed refusals around it can say what they are
    /// and this can shrink as those layers do.
    pub fn from_message(message: &str) -> Self {
        let lower = message.to_ascii_lowercase();
        let has = |needle: &str| lower.contains(needle);
        if has("access denied")
            || has("permission denied")
            || has("this takes an administrator")
            || has("administrator privileges")
            || has("insufficient permissions")
        {
            Refusal::Forbidden
        } else if has("already exists")
            || has("cannot remove the last")
            || has("has changed since")
            || has("are already held")
        {
            Refusal::Conflict
        } else if has("old_string")
            || has("edits[")
            || has("would leave")
            || has("missing required")
            || has("is required")
            || has("must be")
            || has("invalid")
            || has("is not")
            || has("not a ")
            || has("escapes")
            || has("does not match")
        {
            Refusal::BadRequest
        } else if has("not found") || has("no such") || has("no revision") {
            Refusal::NotFound
        } else if has("timed out") {
            Refusal::TimedOut
        } else if has("failed to") {
            Refusal::Failed
        } else {
            Refusal::BadRequest
        }
    }

    pub fn status(self) -> u16 {
        match self {
            Refusal::BadRequest => 400,
            Refusal::Forbidden => 403,
            Refusal::NotFound => 404,
            Refusal::Conflict => 409,
            Refusal::TimedOut => 504,
            Refusal::Failed => 500,
        }
    }
}

/// A refusal as an operation's result: `{ "error": message, "status": code }`.
pub fn refuse(kind: Refusal, message: impl Into<String>) -> Value {
    json!({ "error": message.into(), "status": kind.status() })
}

/// A refusal from an [`AppError`], whose own status says what kind it is.
pub fn refuse_app(error: &crate::error::AppError, message: impl Into<String>) -> Value {
    json!({ "error": message.into(), "status": error.status_code() })
}

/// A refusal whose kind is read from its text; see [`Refusal::from_message`].
pub(super) fn refuse_text(message: impl Into<String>) -> Value {
    let message = message.into();
    refuse(Refusal::from_message(&message), message)
}

/// A refusal from a git operation, by what kind of failure it was.
pub(super) fn refuse_sync(error: &crate::git_sync::SyncError) -> Value {
    use crate::git_sync::SyncError;
    let kind = match error {
        SyncError::AccessDenied(_) => Refusal::Forbidden,
        SyncError::Diverged(_) | SyncError::WouldOverwrite(_) => Refusal::Conflict,
        SyncError::Layout(_) | SyncError::TooLarge(_) => Refusal::BadRequest,
        SyncError::RateLimited(_) => Refusal::Forbidden,
        SyncError::Archive(_) | SyncError::Remote(_) | SyncError::Storage(_) => Refusal::Failed,
    };
    refuse(kind, error.to_string())
}

pub(super) fn missing_arg(name: &str) -> Value {
    refuse(
        Refusal::BadRequest,
        format!("Missing required parameter: {}", name),
    )
}

pub(super) fn native_tools() -> &'static [NativeToolEntry] {
    &[
        (
            "list_scripts",
            "List the scripts in this engine, optionally filtered by a pattern over their URIs. A script is a tree of files; list_files lists one script's files.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "pattern": { "type": "string", "description": "Optional regex pattern to filter scripts by URI" }
                    }
                })
            },
            tool_list_scripts,
        ),
        (
            "rename_script",
            "Rename a script; files, history, secrets, tables and settings go with it and routes are registered again. The new name is a slug: lower-case letters, digits, '-' and '_'. Owner or administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "Current name of the script" },
                        "to": { "type": "string", "description": "The new name, a slug" }
                    },
                    "required": ["script", "to"]
                })
            },
            tool_rename_script,
        ),
        (
            "delete_script",
            "Delete a script and everything that belongs to it: its files, its revisions, its tables and its queued work. To remove one file of a script, use delete_file.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script to delete" }
                    },
                    "required": ["script"]
                })
            },
            tool_delete_script,
        ),
        (
            "search_files",
            "Search script sources and assets for a pattern, to find which file to read. read_file's 'grep' searches one file you can already name.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "query": { "type": "string", "description": "Text or regex pattern to search for" },
                        "caseInsensitive": { "type": "boolean", "description": "Whether search should be case-insensitive", "default": true },
                        "scope": { "type": "string", "enum": ["all", "scripts", "assets"], "description": "Which files to read: 'all' (default), root sources only, or assets only" },
                        "script": { "type": "string", "description": "Search only this script's files" }
                    },
                    "required": ["query"]
                })
            },
            tool_search_files,
        ),
        (
            "read_logs",
            "Read log messages for one script ('script') or every script you own. Owners and administrators only. Each entry carries its invocation (requestId, kind, route), so one request's lines can be read alone.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "Script URI; omit for all" },
                        "level": { "type": "string", "description": "Only entries at this level, e.g. ERROR" },
                        "since": { "type": "string", "description": "Only entries at or after this time (epoch millis or RFC 3339)" },
                        "after_seq": { "type": "integer", "description": "Only entries after this seq; pass the highest seq of the last read for what is new" },
                        "contains": { "type": "string", "description": "Only entries whose message contains this substring" },
                        "request_id": { "type": "string", "description": "Only one invocation's entries, by x-request-id or invocation id" },
                        "kind": { "type": "string", "description": "httpRoute, scheduled, streamCustomization, mcpTool, mcpPrompt, init, eval or test" },
                        "route": { "type": "string", "description": "Only entries while serving this route pattern, e.g. /things/:id" },
                        "revision": { "type": "integer", "description": "Only entries written while this revision was running" },
                        "limit": { "type": "integer", "description": "At most this many of the newest matches" }
                    }
                })
            },
            tool_read_logs,
        ),
        (
            "read_audit",
            "Read the events one script recorded with audit.record, newest first. Its owner or an administrator only; nothing deletes them.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "Script whose events to read" },
                        "action": { "type": "string", "description": "Only events with this action" },
                        "before_id": { "type": "integer", "description": "Only events older than this id; pass the smallest id of the last read for the next page" },
                        "limit": { "type": "integer", "description": "At most this many (default 100, at most 1000)" }
                    },
                    "required": ["script"]
                })
            },
            tool_read_audit,
        ),
        (
            "clear_logs",
            "Delete one script's log messages. Retention across every script is applied by the engine's own pruner and is not something a caller triggers.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "Script URI whose logs to clear" }
                    },
                    "required": ["script"]
                })
            },
            tool_clear_logs,
        ),
        (
            "list_routes",
            "List every registration in the engine: script HTTP routes, SSE streams (method STREAM) and asset routes (method ASSET). Pass 'host' to see only what is live on that host.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "host": { "type": "string", "description": "Only registrations published on this host" }
                    }
                })
            },
            tool_list_routes,
        ),
        (
            "exposure_report",
            "List registrations the engine refused to publish. 'scripts': a file outside the directory that exposes it ('public/' is served, 'resources/' is an MCP resource, the rest is private). 'collisions': a host, path and method already held by an older script. 'unclassified': scripts whose init() has not run cleanly.",
            || {
                json!({
                    "type": "object",
                    "properties": {}
                })
            },
            tool_exposure_report,
        ),
        (
            "read_init_status",
            "Read init() status for scripts (useful for debugging). Returns status for one script when 'script' is given, otherwise for all scripts.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "Optional script URI to retrieve init status for; omit to list all scripts" }
                    }
                })
            },
            tool_read_init_status,
        ),
        (
            "list_files",
            "List the files of a script: its entrypoint (main.ts, main.js, main.tsx or main.jsx) and every module, template and other file beside it. Requires the user to own the script, have ReadAssets capability, or be an administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script whose files to list (e.g., 'https://example.com/myscript')" }
                    },
                    "required": ["script"]
                })
            },
            tool_list_files,
        ),
        (
            "read_file",
            "Read one file of a script, or part of it with 'lines' or 'grep'. The bytes are 'content', with 'encoding' saying 'utf8' or 'base64'; the reply carries the file's sha256 (edit_file's base_sha256). Reads head, which for a pinned script is not what it serves; a 'deployment' block says so.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script that owns the file" },
                        "path": { "type": "string", "description": "Path of the file within the script, e.g. 'main.ts', 'lib/util.ts' or 'images/logo.png'" },
                        "lines": { "type": "string", "description": "1-based inclusive line range, e.g. '120-180', '120-' or '120'" },
                        "grep": { "type": "string", "description": "Regular expression; answers with matching lines instead of the file" }
                    },
                    "required": ["script", "path"]
                })
            },
            tool_read_file,
        ),
        (
            "write_file",
            "Create or update one file. Writing the entrypoint (main.ts/.js/.tsx/.jsx) writes the script itself, creating it if needed, and takes WriteScripts and ownership instead of WriteAssets. Use write_files for several files.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script that owns the file" },
                        "path": { "type": "string", "description": "Path of the file within the script, e.g. 'main.ts' or 'lib/util.ts'" },
                        "mimetype": { "type": "string", "description": "MIME type; inferred from the extension when omitted" },
                        "text": { "type": "string", "description": "The file as text — what a module is. Exactly one of 'text' and 'content' is required." },
                        "content": { "type": "string", "description": "The file as base64, for content that is not text (max 10MB)" }
                    },
                    "required": ["script", "path"]
                })
            },
            tool_write_file,
        ),
        (
            "write_files",
            "Write several of a script's files as one change, then run init() once. Modules go in 'text' as plain source ('content_base64' for non-text); 'remove' deletes paths. One revision, nothing written if any file is rejected. The entrypoint (main.ts/.js/.tsx/.jsx) is one of the files, or 'content'; writing it takes WriteScripts, the rest WriteAssets, both with ownership or administrator. The answer includes a 'check' report: read its diagnostics first.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script whose files these are" },
                        "files": {
                            "type": "array",
                            "description": "Files to write (max 256, 10MB of content in total)",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "name": { "type": "string", "description": "URI/path of the asset (e.g., '/lib/util.ts')" },
                                    "text": { "type": "string", "description": "The file as text; exactly one of 'text' and 'content_base64'" },
                                    "content_base64": { "type": "string", "description": "The file as base64, for content that is not text (max 10MB)" },
                                    "mimetype": { "type": "string", "description": "MIME type; inferred from the file extension when omitted" },
                                    "sha256": { "type": "string", "description": "Expected SHA-256 (hex); the batch is rejected on a mismatch" }
                                },
                                "required": ["name"]
                            }
                        },
                        "content": { "type": "string", "description": "The script's root source as this change leaves it. Omit to leave it alone." },
                        "remove": {
                            "type": "array",
                            "description": "Asset paths this change removes. Naming a file the script does not have is not an error.",
                            "items": { "type": "string" }
                        },
                        "reinit": { "type": "string", "enum": ["after", "never"], "description": "Run the script's init() once after the batch lands (default 'after'), or leave it alone" },
                        "check": { "type": "boolean", "description": "Answer with the check_script report for what was written (default true)" }
                    },
                    "required": ["script"]
                })
            },
            tool_write_files,
        ),
        (
            "edit_file",
            "Edit one file in place by replacing strings, without resending it, then run init() once. Each old_string must be present and unique unless replace_all is set; nothing is written unless every edit applies. Editing the entrypoint takes WriteScripts and ownership; other files WriteAssets and ownership. Edits head; a pinned script keeps serving its pinned revision. The answer includes a 'check' report.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script that owns the file" },
                        "path": { "type": "string", "description": "Path of the file within the script, e.g. 'main.ts' or 'lib/util.ts'" },
                        "edits": {
                            "type": "array",
                            "description": "Edits applied in order (max 128)",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "old_string": { "type": "string", "description": "Text to find. Must appear exactly once unless replace_all is set." },
                                    "new_string": { "type": "string", "description": "Text to put in its place; empty string deletes." },
                                    "replace_all": { "type": "boolean", "description": "Replace every occurrence rather than requiring exactly one (default false)" }
                                },
                                "required": ["old_string", "new_string"]
                            }
                        },
                        "base_sha256": { "type": "string", "description": "SHA-256 from read_file; refused if the file has changed since" },
                        "reinit": { "type": "string", "enum": ["after", "never"], "description": "Run the script's init() once the edits land (default 'after'), or leave it alone" },
                        "check": { "type": "boolean", "description": "Answer with the check_script report for the script as edited (default true)" }
                    },
                    "required": ["script", "path", "edits"]
                })
            },
            tool_edit_file,
        ),
        (
            "create_file",
            "Create a new file; fails if the path exists, unlike write_file. Creating the entrypoint (main.ts/.js/.tsx/.jsx) creates the script.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script that will own the file" },
                        "path": { "type": "string", "description": "Path of the file within the script, e.g. 'main.ts' or 'lib/util.ts'" },
                        "mimetype": { "type": "string", "description": "MIME type; inferred from the file extension when omitted" },
                        "text": { "type": "string", "description": "The file as text — what a module is. Exactly one of 'text' and 'content' is required." },
                        "content": { "type": "string", "description": "The file as base64, for content that is not text (max 10MB)" }
                    },
                    "required": ["script", "path"]
                })
            },
            tool_create_file,
        ),
        (
            "delete_file",
            "Delete one file from a script. Takes DeleteAssets and ownership, or — for the entrypoint, whose removal leaves the script with no program — DeleteScripts. To remove the whole script, use delete_script.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script that owns the file" },
                        "path": { "type": "string", "description": "Path of the file within the script, e.g. 'lib/util.ts' or 'images/logo.png'" }
                    },
                    "required": ["script", "path"]
                })
            },
            tool_delete_file,
        ),
        (
            "list_script_owners",
            "List the owners of a script",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "Script URI" }
                    },
                    "required": ["script"]
                })
            },
            tool_list_script_owners,
        ),
        (
            "add_script_owner",
            "Add an owner to a script. Requires the user to own the script or be an administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "Script URI" },
                        "owner": { "type": "string", "description": "User id to add as owner" }
                    },
                    "required": ["script", "owner"]
                })
            },
            tool_add_script_owner,
        ),
        (
            "remove_script_owner",
            "Remove an owner from a script. Requires the user to own the script or be an administrator; non-admins cannot remove the last owner.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "Script URI" },
                        "owner": { "type": "string", "description": "Owner user id to remove" }
                    },
                    "required": ["script", "owner"]
                })
            },
            tool_remove_script_owner,
        ),
        (
            "list_secrets",
            "List the secret keys stored for a script (values are never returned). Requires the user to own the script or be an administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script whose secrets to manage" }
                    },
                    "required": ["script"]
                })
            },
            tool_list_secrets,
        ),
        (
            "write_secret",
            "Store a secret for a script. Requires the user to own the script or be an administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script whose secrets to manage" },
                        "key": { "type": "string", "description": "Secret key" },
                        "value": { "type": "string", "description": "Secret value (max 1MB)" }
                    },
                    "required": ["script", "key", "value"]
                })
            },
            tool_write_secret,
        ),
        (
            "delete_secret",
            "Remove one secret from a script. Requires the user to own the script or be an administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script whose secrets to manage" },
                        "key": { "type": "string", "description": "Secret key to remove" }
                    },
                    "required": ["script", "key"]
                })
            },
            tool_delete_secret,
        ),
        (
            "clear_secrets",
            "Remove all secrets stored for a script. Requires the user to own the script or be an administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script whose secrets to manage" }
                    },
                    "required": ["script"]
                })
            },
            tool_clear_secrets,
        ),
        (
            "list_users",
            "List all users with their roles and linked identity providers. Administrator privileges required.",
            || {
                json!({
                    "type": "object",
                    "properties": {}
                })
            },
            tool_list_users,
        ),
        (
            "add_user_role",
            "Grant a role to a user. Administrator privileges required.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "user_id": { "type": "string", "description": "Id of the user to modify" },
                        "role": {
                            "type": "string",
                            "description": "Role to grant",
                            "enum": ["Editor", "Administrator", "Authenticated"]
                        }
                    },
                    "required": ["user_id", "role"]
                })
            },
            tool_add_user_role,
        ),
        (
            "set_user_realm",
            "Move a user into a realm: the host they authenticate on, or * for every host. Administrator privileges required. No sign-in path produces *, so this is how an account is given access across hosts. Takes effect on the user's next sign-in.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "user_id": { "type": "string", "description": "Id of the user to modify" },
                        "realm": {
                            "type": "string",
                            "description": "Host name the user authenticates on, or * for every host"
                        }
                    },
                    "required": ["user_id", "realm"]
                })
            },
            tool_set_user_realm,
        ),
        (
            "remove_user_role",
            "Revoke a role from a user. Administrator privileges required. The Authenticated role and the last remaining Administrator cannot be removed.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "user_id": { "type": "string", "description": "Id of the user to modify" },
                        "role": {
                            "type": "string",
                            "description": "Role to revoke",
                            "enum": ["Editor", "Administrator"]
                        }
                    },
                    "required": ["user_id", "role"]
                })
            },
            tool_remove_user_role,
        ),
        (
            "get_script_hosts",
            "Read which hostnames a script's registrations are published on. Administrator privileges required.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script to inspect" }
                    },
                    "required": ["script"]
                })
            },
            tool_get_script_hosts,
        ),
        (
            "set_script_hosts",
            "Set which hostnames a script's registrations are published on. Administrator privileges required. Pass '*' to publish on every configured host, or an empty list to return the script to the default host.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script to modify" },
                        "hosts": {
                            "type": "array",
                            "description": "Hostnames to publish on. ['*'] means every configured host; [] returns the script to the default host.",
                            "items": { "type": "string" }
                        }
                    },
                    "required": ["script", "hosts"]
                })
            },
            tool_set_script_hosts,
        ),
        (
            "run_tests",
            "Run a script's test modules ('*.test.ts' or .js/.jsx/.tsx assets) and report a verdict per case. Runs the stored files (head). Owner or administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script whose tests to run" },
                        "filter": { "type": "string", "description": "Run only cases whose name contains this text" },
                        "rollback": {
                            "type": "boolean",
                            "description": "Roll back the database writes the tests make (default true)",
                            "default": true
                        },
                        "revision": {
                            "type": "string",
                            "description": "A revision number, 'head', 'last-good' or a label; omit for head"
                        }
                    },
                    "required": ["script"]
                })
            },
            tool_run_tests,
        ),
        (
            "list_revisions",
            "A script's revision history: what changed, when, by whom, and whether init() succeeded. 'lastGood' is the newest revision whose init() succeeded. Pass 'asset' for one file's history.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script whose history to read" },
                        "asset": { "type": "string", "description": "Only the revisions in which this file changed" },
                        "limit": { "type": "integer", "description": "Keep at most this many of the newest revisions (default 50)" }
                    },
                    "required": ["script"]
                })
            },
            tool_list_revisions,
        ),
        (
            "revert_script",
            "Restore a script's files to a revision, recorded as a new revision. Removes files that revision did not contain; refuses a target that does not bundle unless 'force'. 'dryRun' previews. Owner or administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script to restore" },
                        "revision": { "type": "string", "description": "Which version to restore: a revision number, 'head', 'last-good', or a label" },
                        "dryRun": {
                            "type": "boolean",
                            "description": "Report what would change without changing it",
                            "default": false
                        },
                        "force": {
                            "type": "boolean",
                            "description": "Restore even if the target revision does not bundle",
                            "default": false
                        },
                        "reinit": {
                            "type": "string",
                            "description": "'after' runs the script's init() once the files land (default); 'never' leaves it alone"
                        }
                    },
                    "required": ["script", "revision"]
                })
            },
            tool_revert_script,
        ),
        (
            "diff_revisions",
            "A unified diff per file between two revisions: see what you changed, or what a revert would undo. With neither 'from' nor 'to', the newest change.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script to compare" },
                        "from": { "type": "string", "description": "The older side: a revision number, 'head', 'last-good' or a label" },
                        "to": { "type": "string", "description": "The newer side. Defaults to 'head'." },
                        "context": { "type": "integer", "description": "Lines of context around each hunk (default 3)" }
                    },
                    "required": ["script"]
                })
            },
            tool_diff_revisions,
        ),
        (
            "label_revision",
            "Name a revision so it can be restored by name and survives retention. Omit 'label' to clear one.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script whose revision to name" },
                        "revision": { "type": "string", "description": "Which revision: a number, 'head', 'last-good', or an existing label" },
                        "label": { "type": "string", "description": "The name. Omit to clear the revision's label." }
                    },
                    "required": ["script", "revision"]
                })
            },
            tool_label_revision,
        ),
        (
            "set_script_limits",
            "Give one script its own execution budget, to raise it for a slow model API or lower it to contain a runaway; effective without a restart. Administrator only, because it claims shared slots, threads and memory. Omit a field to follow the engine; omit all to remove the override.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script" },
                        "timeoutMs": { "type": "integer", "description": "Wall clock for one request-shaped invocation" },
                        "jobTimeoutMs": { "type": "integer", "description": "Wall clock for one scheduled job or queued task" },
                        "maxMemoryBytes": { "type": "integer", "description": "Heap ceiling for this script's runtime" },
                        "note": { "type": "string", "description": "Why, for whoever reads this next" }
                    },
                    "required": ["script"]
                })
            },
            tool_set_script_limits,
        ),
        (
            "get_script_limits",
            "What one script may spend, and what that resolves to once the engine's own settings are laid under it. Omit 'script' to list every override in the engine — which is the way to find out why one script behaves differently from the rest.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of one script; omit to list every override" }
                    }
                })
            },
            tool_get_script_limits,
        ),
        (
            "list_tasks",
            "Read a script's queued work: pending, running, and failed (with the error). A task that succeeded is deleted; its output is in the log under its invocation id. A pending task may be waiting behind another in the same 'lane'.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script whose queue to read" },
                        "limit": { "type": "integer", "description": "How many to return, newest first (default 50, max 500)" }
                    },
                    "required": ["script"]
                })
            },
            tool_list_tasks,
        ),
        (
            "cancel_task",
            "Cancel one pending task (one already running is not stopped), or with finished=true discard the failed and cancelled ones.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script whose queue to act on" },
                        "task": { "type": "string", "description": "Id of the pending task to cancel" },
                        "finished": {
                            "type": "boolean",
                            "description": "Discard this script's failed and cancelled tasks instead of cancelling one.",
                            "default": false
                        }
                    },
                    "required": ["script"]
                })
            },
            tool_cancel_task,
        ),
        (
            "deploy_script",
            "Choose which revision of a script is served. After the first deploy, writes advance head without changing what is served, until you deploy again. revision='head' takes the newest; follow=true stops pinning.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script to deploy" },
                        "revision": { "type": "string", "description": "Which version to serve: a revision number, 'head', 'last-good', or a label" },
                        "follow": {
                            "type": "boolean",
                            "description": "Stop pinning; serve the newest revision. Ignores 'revision'.",
                            "default": false
                        }
                    },
                    "required": ["script"]
                })
            },
            tool_deploy_script,
        ),
        (
            "set_git_credential",
            "Store your personal access token for a git host so pulls can read private repositories. Encrypted at rest, never returned, checked against the host first. Always your own credential.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "token": { "type": "string", "description": "Personal access token; read access to the repositories' contents is enough" },
                        "host": { "type": "string", "description": "Git host. Defaults to github.com, the only host supported.", "default": "github.com" }
                    },
                    "required": ["token"]
                })
            },
            tool_set_git_credential,
        ),
        (
            "list_git_credentials",
            "The git credentials you have stored: host, the account each belongs to, and when it \
            was added and last used. Never the token itself.",
            || json!({ "type": "object", "properties": {} }),
            tool_list_git_credentials,
        ),
        (
            "delete_git_credential",
            "Remove your stored credential for a git host. Pulls of private repositories stop \
            working; public ones carry on.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "host": { "type": "string", "description": "Git host to forget. Defaults to github.com.", "default": "github.com" }
                    }
                })
            },
            tool_delete_git_credential,
        ),
        (
            "list_git_bindings",
            "Which repository each script tracks, and where the two last agreed. Shows only \
            scripts you can read.",
            || json!({ "type": "object", "properties": {} }),
            tool_list_git_bindings,
        ),
        (
            "clear_git_remote",
            "Stop a script tracking a repository. The script and its files stay exactly as they \
            are; what goes is the record of where they came from, so later pulls no longer treat \
            it as that repository's to replace.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script to unbind" }
                    },
                    "required": ["script"]
                })
            },
            tool_clear_git_remote,
        ),
        (
            "get_git_status",
            "Where a script stands against its repository: unbound, in_sync, behind, ahead, diverged or unreachable. Ask this before pushing rather than reading the refusal. Also reports the deployment pin.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script to report on" }
                    },
                    "required": ["script"]
                })
            },
            tool_get_git_status,
        ),
        (
            "push_to_git",
            "Publish a script's files to a GitHub repository as one commit, laid out the way pull_from_git reads them back. Files the script does not own are left alone. Refuses when both sides changed since the last sync; the engine does not merge. Needs a stored credential with write access.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script to publish" },
                        "repo": { "type": "string", "description": "'owner/repo' or a GitHub URL; optional once the script was pulled" },
                        "branch": { "type": "string", "description": "Branch to write. Defaults to the one it was pulled from, or the repository's default branch." },
                        "message": { "type": "string", "description": "Commit message. Defaults to one naming the script." },
                        "force": {
                            "type": "boolean",
                            "description": "Publish even if the repository seems to have moved; GitHub still refuses a non-fast-forward",
                            "default": false
                        }
                    },
                    "required": ["script"]
                })
            },
            tool_push_to_git,
        ),
        (
            "pull_from_git",
            "Pull a public GitHub repository in as scripts: a directory holding main.ts (or .js/.tsx/.jsx) is one script, other files under it become assets at the same relative path, and files removed upstream are removed here.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "repo": { "type": "string", "description": "'owner/repo', or any GitHub URL naming it" },
                        "branch": { "type": "string", "description": "Branch to read. Defaults to the repository's default branch." },
                        "prefix": { "type": "string", "description": "URI prefix for the scripts; defaults to the repository name" },
                        "force": {
                            "type": "boolean",
                            "description": "Re-apply even if the repository has not moved",
                            "default": false
                        }
                    },
                    "required": ["repo"]
                })
            },
            tool_pull_from_git,
        ),
        (
            "get_deployment",
            "What a script serves, what its newest revision is, and how many revisions separate them. Use before deploying to see what taking head would mean.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script to report on" }
                    },
                    "required": ["script"]
                })
            },
            tool_get_deployment,
        ),
        (
            "check_script",
            "Check a script's stored files (head) as if deployed, without deploying: bundle its imports, run init() with registrations withheld and database writes rolled back, and report diagnostics {file, line, severity, code, message}. Pass 'content' (or 'files') to check code before writing it. Owner or administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script to check" },
                        "content": {
                            "type": "string",
                            "description": "Candidate source to check instead of what is deployed"
                        },
                        "rollback": {
                            "type": "boolean",
                            "description": "Roll back the database writes init() makes (default true)",
                            "default": true
                        },
                        "timeoutMs": {
                            "type": "integer",
                            "description": "Ceiling for the init() run; defaults to several times the deploy budget"
                        },
                        "revision": {
                            "type": "string",
                            "description": "A revision number, 'head', 'last-good' or a label; omit for head"
                        },
                        "files": {
                            "type": "object",
                            "description": "Candidate files laid over what is deployed: path -> {content, mimetype?}, or path -> null to check a removal",
                            "additionalProperties": true
                        }
                    },
                    "required": ["script"]
                })
            },
            tool_check_script,
        ),
        (
            "eval_script",
            "Evaluate a JavaScript snippet in a deployed script's sandbox and return its value plus what it logged. The script's program is loaded first, so the snippet can call its functions and import its modules. Database writes roll back by default; registrations do nothing. Owner or administrator.",
            || {
                json!({
                    "type": "object",
                    "properties": {
                        "script": { "type": "string", "description": "URI of the script whose sandbox to evaluate in" },
                        "source": {
                            "type": "string",
                            "description": "The snippet; its last expression is the value. Synchronous: no async/await."
                        },
                        "rollback": {
                            "type": "boolean",
                            "description": "Roll back the database writes the snippet makes (default true)",
                            "default": true
                        },
                        "timeoutMs": {
                            "type": "integer",
                            "description": "Budget for the evaluation, clamped to the engine's execution timeout"
                        }
                    },
                    "required": ["script", "source"]
                })
            },
            tool_eval_script,
        ),
    ]
}

/// Ceiling for a native tool that does nothing but read or write the
/// repository.
///
/// Generous, because it is a backstop rather than a budget: these tools are
/// database round trips that finish in milliseconds, and the only way to reach
/// this is a connection that will never answer.
pub(super) const NATIVE_TOOL_CEILING_MS: u64 = 30_000;

/// The longest a native tool can legitimately run, for the MCP dispatcher's
/// backstop.
///
/// Returns `None` for a name that is not a native tool, so the dispatcher falls
/// back to the JavaScript execution budget that bounds a script-registered one.
/// Each tool that enforces its own ceiling reports that ceiling here, so the
/// backstop never cuts short a call the tool would have completed — it only
/// fires once a tool is past every limit it sets for itself, which means it is
/// blocked somewhere no interrupt can reach.
pub fn native_tool_ceiling_ms(tool_name: &str) -> Option<u64> {
    match tool_name {
        "run_tests" => Some(crate::script_test::configured_test_timeouts().1),
        "check_script" => Some(crate::script_check::MAX_CHECK_TIMEOUT_MS),
        "eval_script" => Some(crate::script_eval::default_eval_timeout_ms()),
        name if is_native_mcp_tool(name) => Some(NATIVE_TOOL_CEILING_MS),
        _ => None,
    }
}

/// Whether `name` is one of the engine's own MCP tools.
///
/// Native tools take precedence over script-registered ones at dispatch
/// ([`crate::mcp::execute_mcp_tool`]), so anything deciding whether a call is
/// allowed has to ask this before consulting the script registry — otherwise a
/// script registering a colliding name would answer for the native tool.
pub fn is_native_mcp_tool(name: &str) -> bool {
    native_tools().iter().any(|(tool, _, _, _)| *tool == name)
}

/// Descriptors of all native MCP tools, for tools/list.
pub fn native_mcp_tool_descriptors() -> Vec<NativeToolDescriptor> {
    native_tools()
        .iter()
        .map(|(name, description, schema, _)| NativeToolDescriptor {
            name,
            description,
            input_schema: schema(),
        })
        .collect()
}

/// Execute a native MCP tool. Returns None when no native tool has this name
/// (the caller then falls back to script-registered tools).
pub fn execute_native_mcp_tool(
    tool_name: &str,
    arguments: &Value,
    user_context: &UserContext,
) -> Option<Value> {
    let handler = native_tools()
        .iter()
        .find(|(name, _, _, _)| *name == tool_name)
        .map(|(_, _, _, handler)| *handler)?;
    Some(handler(arguments, user_context))
}
