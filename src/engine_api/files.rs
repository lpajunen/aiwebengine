//! Reading, writing, editing and deleting the files of a script's tree.

use super::*;
use crate::repository;
use crate::revisions;
use crate::security::{
    Capability, SecurityEvent, SecurityEventType, SecuritySeverity, UserContext,
};
use serde::Deserialize;
use serde_json::{Value, json};

/// Whether the user may access assets of `script_uri` given the per-operation
/// capability: capability holders, script owners, and admins all qualify.
pub(super) fn can_access_assets(
    user: &UserContext,
    script_uri: &str,
    capability: &Capability,
) -> bool {
    may_administer(user)
        && (user.has_capability(&Capability::AdministerEngine)
            || (user.has_capability(capability) && user_owns_script(user, script_uri)))
}

/// List asset metadata for a script (empty when access is denied).
pub fn list_assets_authorized(user: &UserContext, script_uri: &str) -> Vec<Value> {
    if !can_access_assets(user, script_uri, &Capability::ReadAssets) {
        return Vec::new();
    }
    let millis = |t: std::time::SystemTime| {
        t.duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as f64
    };
    repository::fetch_assets(script_uri)
        .values()
        .map(|asset| {
            json!({
                "uri": asset.uri,
                "name": asset.name,
                "size": asset.content.len(),
                "mimetype": asset.mimetype,
                "createdAt": millis(asset.created_at),
                "updatedAt": millis(asset.updated_at),
            })
        })
        .collect()
}

pub enum AssetFetchError {
    AccessDenied,
    NotFound,
}

/// An inclusive, 1-based range of lines, as `lines=120-180` spells it. An
/// absent `end` means "to the end of the file".
#[derive(Clone, Copy)]
pub struct LineRange {
    pub start: usize,
    pub end: Option<usize>,
}

impl LineRange {
    /// Parse the `lines=` parameter: `120-180`, `120-` (to the end of the
    /// file), or `120` (that line alone).
    pub fn parse(raw: &str) -> Result<Self, String> {
        let raw = raw.trim();
        let (start_raw, end_raw) = match raw.split_once('-') {
            Some((start, end)) => (start.trim(), end.trim()),
            None => (raw, raw),
        };
        let number = |value: &str| -> Result<usize, String> {
            value
                .parse::<usize>()
                .ok()
                .filter(|line| *line > 0)
                .ok_or_else(|| {
                    format!(
                        "Invalid lines range '{}': expected 'start-end', 'start-', or 'start', \
                         counting from 1",
                        raw
                    )
                })
        };
        let start = number(start_raw)?;
        let end = if end_raw.is_empty() {
            None
        } else {
            Some(number(end_raw)?)
        };
        if let Some(end) = end
            && end < start
        {
            return Err(format!(
                "Invalid lines range '{}': end is before start",
                raw
            ));
        }
        Ok(LineRange { start, end })
    }
}

/// How a read of one file is scoped — one of a script's assets, or its root
/// source. The two filters compose: a `grep` inside a `lines` range searches
/// only that range.
#[derive(Default)]
pub struct FileReadOptions {
    pub lines: Option<LineRange>,
    pub grep: Option<String>,
}

impl FileReadOptions {
    /// Whether the caller asked for a view of part of the file rather than the
    /// whole of it.
    pub(super) fn is_scoped(&self) -> bool {
        self.lines.is_some() || self.grep.is_some()
    }
}

/// One line a `grep=` read matched.
pub struct GrepMatch {
    pub line: usize,
    pub text: String,
    /// Whether `text` is only the beginning of a longer line.
    pub truncated: bool,
}

impl GrepMatch {
    pub(super) fn to_json(&self) -> Value {
        json!({
            "line": self.line,
            "text": self.text,
            "truncated": self.truncated,
        })
    }
}

/// What a read returned: the whole file, a range of it, or the places a
/// pattern matched.
pub enum FileView {
    Full {
        content_base64: String,
    },
    /// The whole file as text. What a read of a script's root source answers
    /// with: it is a program, so there is nothing base64 would be protecting.
    Whole {
        content: String,
    },
    Range {
        content: String,
        start_line: usize,
        end_line: usize,
    },
    Matches {
        matches: Vec<GrepMatch>,
        truncated: bool,
    },
}

/// One file as read, whole or scoped.
pub struct FileRead {
    pub view: FileView,
    /// Digest of the whole stored file, whichever part of it was returned —
    /// this is what a following patch sends as `base_sha256`.
    pub sha256: String,
    /// Size of the whole stored file, in bytes.
    pub bytes: usize,
    /// Line count of the whole file; absent from a whole-file read, which
    /// does not require the content to be text at all.
    pub total_lines: Option<usize>,
}

impl FileRead {
    /// The response fields for this read, to be merged with the file the
    /// caller named.
    pub fn to_json(&self) -> Value {
        let mut body = match &self.view {
            // Said rather than implied. One `read_file` answers with either
            // spelling depending on whether the bytes are text, so a caller
            // branches on a field that is present rather than on one that is
            // absent.
            FileView::Full { content_base64 } => {
                json!({ "encoding": "base64", "content": content_base64 })
            }
            FileView::Whole { content } => json!({ "encoding": "utf8", "content": content }),
            FileView::Range {
                content,
                start_line,
                end_line,
            } => json!({
                "encoding": "utf8",
                "content": content,
                "start_line": start_line,
                "end_line": end_line,
            }),
            FileView::Matches { matches, truncated } => json!({
                "encoding": "utf8",
                "matches": matches.iter().map(GrepMatch::to_json).collect::<Vec<Value>>(),
                "match_count": matches.len(),
                "truncated": truncated,
            }),
        };
        if let Some(object) = body.as_object_mut() {
            object.insert("sha256".to_string(), json!(self.sha256));
            object.insert("bytes".to_string(), json!(self.bytes));
            if let Some(total_lines) = self.total_lines {
                object.insert("total_lines".to_string(), json!(total_lines));
            }
        }
        body
    }
}

pub enum FileReadError {
    AccessDenied,
    NotFound,
    /// The read asked for a text view of something that is not text, or asked
    /// for it in a way that does not parse.
    Validation(String),
}

/// Compile a `grep=` pattern, with the bounds a request-time regex needs.
pub(super) fn compile_grep(pattern: &str) -> Result<regex::Regex, FileReadError> {
    if pattern.is_empty() {
        return Err(FileReadError::Validation(
            "Empty grep pattern: there is nothing to search for".to_string(),
        ));
    }
    if pattern.chars().count() > MAX_GREP_PATTERN_CHARS {
        return Err(FileReadError::Validation(format!(
            "grep pattern too long (max {} characters)",
            MAX_GREP_PATTERN_CHARS
        )));
    }
    regex::RegexBuilder::new(pattern)
        .size_limit(1 << 20)
        .dfa_size_limit(1 << 20)
        .build()
        .map_err(|e| FileReadError::Validation(format!("Invalid grep pattern: {}", e)))
}

/// Cut a line down to what a match listing echoes back, on a character
/// boundary. Reports whether anything was cut.
pub(super) fn truncate_chars(line: &str, max: usize) -> (String, bool) {
    match line.char_indices().nth(max) {
        Some((index, _)) => (line[..index].to_string(), true),
        None => (line.to_string(), false),
    }
}

/// The part of a file a scoped read asked for, and how many lines the whole of
/// it has.
///
/// Shared by every read that can be scoped, because slicing a file and
/// searching it are the same operations whether the file is one of a script's
/// assets or the script's own root source. `file` and `kind` are quoted back
/// in the refusals — the only part that differs between them.
pub(super) fn scoped_view(
    content: &[u8],
    options: &FileReadOptions,
    file: &str,
    kind: &str,
) -> Result<(FileView, usize), FileReadError> {
    let text = std::str::from_utf8(content).map_err(|_| {
        FileReadError::Validation(format!(
            "{} '{}' is not UTF-8 text, so it has no lines to read: fetch it without \
             'lines' or 'grep' to get its bytes",
            kind, file
        ))
    })?;

    let lines: Vec<&str> = text.lines().collect();
    let range = options.lines.unwrap_or(LineRange {
        start: 1,
        end: None,
    });
    // A `start` past the end of the file asks for a part that is not there —
    // most often a range computed against a version that has since shrunk —
    // and an empty 200 would leave the caller to work that out for itself. An
    // `end` past the end is a different request: "through line 1000" of a
    // 400-line file plainly means the rest of it, so it clamps.
    if options.lines.is_some() && range.start > lines.len() {
        return Err(FileReadError::Validation(format!(
            "{} '{}' has {} lines, so there is no line {} to read",
            kind,
            file,
            lines.len(),
            range.start
        )));
    }
    let start = range.start;
    let end = range.end.unwrap_or(lines.len()).min(lines.len());
    // Only an empty file reaches this, and only through the default range: an
    // explicit one was bounded above.
    let selected: &[&str] = if start > end {
        &[]
    } else {
        &lines[start - 1..end]
    };

    let view = match &options.grep {
        None => FileView::Range {
            content: selected.join("\n"),
            start_line: start,
            end_line: end,
        },
        Some(pattern) => {
            let regex = compile_grep(pattern)?;
            let mut matches = Vec::new();
            let mut truncated = false;
            for (offset, line) in selected.iter().enumerate() {
                if !regex.is_match(line) {
                    continue;
                }
                if matches.len() >= MAX_GREP_MATCHES {
                    truncated = true;
                    break;
                }
                let (text, line_truncated) = truncate_chars(line, MAX_GREP_LINE_CHARS);
                matches.push(GrepMatch {
                    line: start + offset,
                    text,
                    truncated: line_truncated,
                });
            }
            FileView::Matches { matches, truncated }
        }
    };

    Ok((view, lines.len()))
}

#[derive(Debug)]
pub enum AssetWriteError {
    /// The write was to create a file and the file is already there. Its own
    /// variant rather than a validation message, because the caller's next
    /// move is different: this one is told what it asked to be told.
    Exists(String),
    /// Carries why, for the same reason [`PatchError::AccessDenied`] does: a
    /// write covering a script's root and its assets can be refused by either
    /// rule, and "you do not own this script" is not the message an asset
    /// write would have given.
    AccessDenied(String),
    Validation(String),
    Storage(String),
}

/// The shape checks an asset path has to pass before it can be stored.
pub(super) fn validate_asset_uri(asset_uri: &str) -> Result<(), AssetWriteError> {
    if asset_uri.is_empty() || asset_uri.len() > 255 {
        return Err(AssetWriteError::Validation(format!(
            "Invalid asset URI '{}': must be 1-255 characters",
            asset_uri
        )));
    }
    if asset_uri.contains("..") || asset_uri.contains('\\') {
        return Err(AssetWriteError::Validation(format!(
            "Invalid asset URI '{}': path traversal not allowed",
            asset_uri
        )));
    }
    Ok(())
}

/// Lowercase hex SHA-256 of an asset's decoded bytes — the digest the batch
/// write echoes back so a caller can verify what was stored without reading it
/// again.
pub(super) fn sha256_hex(content: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(content);
    hex::encode(hasher.finalize())
}

/// MIME type inferred from an asset's extension, for batch callers that push a
/// directory of source files and have nothing to say about each one's type.
///
/// Deliberately narrow: it covers what scripts are actually made of, and
/// anything else falls back to a type that will be served as a download rather
/// than guessed at.
pub(crate) fn mimetype_for(asset_uri: &str) -> &'static str {
    let extension = asset_uri
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "js" | "mjs" | "cjs" | "jsx" => "text/javascript",
        "ts" | "tsx" | "mts" | "cts" => "text/typescript",
        "json" => "application/json",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "md" => "text/markdown",
        "txt" => "text/plain",
        "csv" => "text/csv",
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// Create or update an asset from base64 content.
pub fn upsert_asset_authorized(
    user: &UserContext,
    script_uri: &str,
    asset_uri: &str,
    mimetype: &str,
    content_b64: &str,
) -> Result<Option<i32>, AssetWriteError> {
    write_asset_authorized(user, script_uri, asset_uri, mimetype, content_b64, false)
}

/// [`upsert_asset_authorized`], with the option of refusing to overwrite.
///
/// `create_file` could say "this is a new script, fail if it is not" and the
/// asset write could not, so creating a module and overwriting somebody's was
/// the same request. `if_absent` is that distinction: it is a precondition on
/// the write rather than a check before it, since a caller who reads first and
/// writes second has a window in which the answer changes.
pub fn write_asset_authorized(
    user: &UserContext,
    script_uri: &str,
    asset_uri: &str,
    mimetype: &str,
    content_b64: &str,
    if_absent: bool,
) -> Result<Option<i32>, AssetWriteError> {
    let content = base64::engine::general_purpose::STANDARD
        .decode(content_b64)
        .map_err(|e| {
            AssetWriteError::Validation(format!("Error decoding base64 content: {}", e))
        })?;
    write_file_bytes_authorized(user, script_uri, asset_uri, mimetype, content, if_absent)
}

/// Write one file of a script's tree, whole, given the bytes rather than a
/// spelling of them.
///
/// The transfer encoding is the request's business, and a module arrives as
/// text: `write_file` and `create_file` take `text`, and so does the batch.
///
/// The entrypoint differs from the rest in three ways, and all three are
/// here rather than in a second copy of this function:
///
/// - It takes `WriteScripts` and ownership rather than `WriteAssets`, the
///   line the patch and the delete also draw.
/// - Writing it can *create* the script, because a script is brought into
///   being by its program. Every file references the `scripts` row, so that
///   row has to exist first; a write of any other file to a script that is
///   not there is a write with nothing to belong to.
/// - It meets the 1MB source ceiling rather than the 10MB file one.
pub fn write_file_bytes_authorized(
    user: &UserContext,
    script_uri: &str,
    path: &str,
    mimetype: &str,
    content: Vec<u8>,
    if_absent: bool,
) -> Result<Option<i32>, AssetWriteError> {
    let is_root = crate::module_loader::is_root_module_name(path);

    validate_asset_uri(path)?;
    if if_absent && repository::fetch_asset(script_uri, path).is_some() {
        return Err(AssetWriteError::Exists(path.to_string()));
    }
    let ceiling = if is_root {
        repository::MAX_SCRIPT_CONTENT_BYTES
    } else {
        MAX_ASSET_BYTES
    };
    if content.len() > ceiling {
        return Err(AssetWriteError::Validation(format!(
            "'{}' too large: {} bytes (max {})",
            path,
            content.len(),
            ceiling
        )));
    }

    // Writing the entrypoint *is* writing the script, so it goes the way
    // writing a script goes: the same permission, the same ownership on
    // creation, the same revision, the same broadcast to `/engine/script_updates`
    // and the same `init()`. Delegating rather than repeating that is what
    // the merge is for — there is one way to replace a script's program,
    // whichever name the caller reached for.
    if is_root {
        let text = String::from_utf8(content).map_err(|_| {
            AssetWriteError::Validation(format!(
                "'{}' is a script's program, so it has to be text",
                path
            ))
        })?;
        return upsert_root_authorized(user, script_uri, Some(path), &text, Some("file"))
            .map(|(_, revision)| revision)
            .map_err(|message| {
                let message = message
                    .strip_prefix("Error: ")
                    .unwrap_or(&message)
                    .to_string();
                // Its two refusals are not the same answer: a caller who may
                // not write this script is forbidden, and one who sent no
                // content asked for something impossible.
                if message.starts_with("Script name and content cannot be empty") {
                    AssetWriteError::Validation(message)
                } else {
                    AssetWriteError::AccessDenied(message)
                }
            });
    }

    if !can_access_assets(user, script_uri, &Capability::WriteAssets) {
        return Err(AssetWriteError::AccessDenied("Access denied".to_string()));
    }

    let auditor = auditor();
    let user_id = user.user_id.clone();
    let script_uri_owned = script_uri.to_string();
    let path_owned = path.to_string();
    let content_len = content.len();
    let mimetype_owned = mimetype.to_string();
    tokio::task::spawn(async move {
        let _ = auditor
            .log_event(
                SecurityEvent::new(
                    SecurityEventType::SystemSecurityEvent,
                    SecuritySeverity::Medium,
                    user_id,
                )
                .with_resource("file".to_string())
                .with_action("upsert_for_uri".to_string())
                .with_detail("uri", &path_owned)
                .with_detail("script_uri", &script_uri_owned)
                .with_detail("content_size", content_len.to_string())
                .with_detail("mimetype", &mimetype_owned),
            )
            .await;
    });

    let now = std::time::SystemTime::now();
    let asset = repository::Asset {
        uri: path.to_string(),
        name: Some(path.to_string()),
        mimetype: mimetype.to_string(),
        content,
        created_at: now,
        updated_at: now,
        script_uri: script_uri.to_string(),
    };
    repository::upsert_asset(asset)
        .map_err(|e| AssetWriteError::Storage(format!("Error upserting asset: {}", e)))?;

    Ok(revisions::record_blocking(
        script_uri,
        revisions::Origin::Post,
        user.user_id.as_deref(),
    ))
}

/// One file of a batch asset write.
pub struct AssetWrite {
    /// Path of the asset within the script, e.g. `/lib/util.ts`.
    pub name: String,
    /// MIME type; inferred from the extension when the caller omits it.
    pub mimetype: Option<String>,
    /// The file's bytes, already decoded.
    ///
    /// How they arrived is the request's business and not this type's: a
    /// module comes as text and an image as base64, and by here both are
    /// bytes. [`asset_content_from_request`] is where that choice is made,
    /// once, for the endpoint and the tool alike.
    pub content: Vec<u8>,
    /// Digest the caller believes the bytes have, lowercase hex. When present
    /// it is checked before anything is written.
    pub expected_sha256: Option<String>,
}

/// The bytes of one file of a batch, from whichever field carried them.
///
/// A batch required base64 for every file, including the modules — which are
/// text, are written as text, and are checked as text: `/engine/check_script` takes
/// candidate modules as plain source and says why ("a module the bundler can
/// read has to be UTF-8 anyway"). So the request that *described* a change and
/// the request that *applied* it, which are meant to be the same shape,
/// disagreed about the one field that carries the code. Encoding it cost a
/// third of the bytes and a step no agent can do reliably in its head, which
/// is what drove callers back to writing one file per request — and back to
/// the partial deployments a batch exists to prevent.
///
/// Exactly one of the two fields, because a file whose two spellings disagree
/// has no right answer and guessing which was meant is worse than refusing.
pub(super) fn asset_content_from_request(
    label: &str,
    base64_field: &str,
    text: Option<String>,
    encoded: Option<String>,
) -> Result<Vec<u8>, String> {
    match (text, encoded) {
        (Some(_), Some(_)) => Err(format!(
            "{}: give either 'text' or '{}', not both",
            label, base64_field
        )),
        (Some(text), None) => Ok(text.into_bytes()),
        (None, Some(encoded)) => base64::engine::general_purpose::STANDARD
            .decode(&encoded)
            .map_err(|e| format!("{}: error decoding base64 content: {}", label, e)),
        (None, None) => Err(format!(
            "{}: missing required field: text (or {} for a file that is not text)",
            label, base64_field
        )),
    }
}

/// What a batch write did with one file.
pub struct AssetWriteOutcome {
    pub name: String,
    pub sha256: String,
    pub bytes: usize,
    /// `created`, `updated`, or `unchanged`.
    pub status: &'static str,
}

impl AssetWriteOutcome {
    pub(super) fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "sha256": self.sha256,
            "bytes": self.bytes,
            "status": self.status,
        })
    }
}

/// What a batch write did overall.
pub struct BatchWriteOutcome {
    pub results: Vec<AssetWriteOutcome>,
    /// The revision this write produced, or `None` when it changed nothing.
    /// The number a caller reverts to when the change turns out to be wrong.
    pub revision: Option<i32>,
    /// How many files reached the database. Zero when every file already held
    /// the content it was sent with, which is also when re-initializing the
    /// script afterwards would be pure cost.
    pub written: usize,
    /// How many of the requested removals actually removed something. A sync
    /// naming a file the script no longer has is not an error; it is a sync
    /// that has already happened.
    pub deleted: usize,
}

/// What a sync wants from the asset write that a plain batch does not.
pub struct AssetSyncOptions<'a> {
    /// Asset paths that must not survive this write.
    ///
    /// A pull is a *sync* rather than an append: a module deleted upstream has
    /// to go here too, or the script keeps building against a file its source
    /// of truth no longer holds.
    pub delete: &'a [String],
    /// How the resulting revision describes where it came from.
    pub origin: revisions::Origin,
    /// Ceiling on this write's total content.
    ///
    /// A parameter because [`MAX_BATCH_BYTES`] bounds an HTTP *request body*,
    /// and a sync the engine started on its own has no request body to bound.
    pub max_total_bytes: usize,
    /// Ceiling on how many files this write may carry, for the same reason.
    pub max_files: usize,
    /// Whether this write records a revision of its own.
    ///
    /// False for a caller whose write is one part of a larger change. A pull
    /// writes a script's root and then its assets, and both belong to one
    /// revision: letting the asset write record its own would describe the
    /// change as two, and would miss a pull that only altered the root — a
    /// script consisting of nothing but `main.ts` writes no assets at all.
    pub record_revision: bool,
}

impl Default for AssetSyncOptions<'_> {
    /// What an HTTP batch write asks for: no removals, and the ceilings that
    /// bound a request.
    fn default() -> Self {
        Self {
            delete: &[],
            origin: revisions::Origin::Batch,
            max_total_bytes: MAX_BATCH_BYTES,
            max_files: MAX_BATCH_FILES,
            record_revision: true,
        }
    }
}

/// Write several of a script's assets as one unit.
///
/// Every file is decoded and checked before any of them is stored, so a batch
/// with one bad entry writes nothing: the caller's tree never lands in the
/// engine half-applied. Files whose stored content already matches are
/// reported as `unchanged` and skipped, because rewriting one would invalidate
/// the script's prepared program for no change.
///
/// The authorization is the single-write rule applied once, not per file:
/// WriteAssets capability, ownership of the script, or admin.
pub fn upsert_assets_authorized(
    user: &UserContext,
    script_uri: &str,
    files: &[AssetWrite],
) -> Result<BatchWriteOutcome, AssetWriteError> {
    upsert_assets_synced(user, script_uri, files, AssetSyncOptions::default())
}

/// [`upsert_assets_authorized`], for a caller replacing a script's tree rather
/// than adding to it.
///
/// One implementation rather than two, because the difference between a batch
/// write and a sync is three parameters and not a different set of rules. A
/// second write path would have to reimplement the ownership check, the digest
/// comparison that drops unchanged files, and the audit event — and one arm of
/// that would drift.
pub fn upsert_assets_synced(
    user: &UserContext,
    script_uri: &str,
    files: &[AssetWrite],
    options: AssetSyncOptions<'_>,
) -> Result<BatchWriteOutcome, AssetWriteError> {
    if !can_access_assets(user, script_uri, &Capability::WriteAssets) {
        return Err(AssetWriteError::AccessDenied("Access denied".to_string()));
    }
    // A removal that takes the entrypoint with it takes what removing a script
    // takes, the same line [`delete_asset_authorized`] draws. A change that
    // also writes a root is a rename rather than a removal — `main.js` out,
    // `main.ts` in — and the caller writing the new one has already been
    // asked for script-write rights by the time this runs.
    if options.delete.iter().any(|path| {
        crate::module_loader::is_root_module_name(path)
            && !files
                .iter()
                .any(|file| crate::module_loader::is_root_module_name(&file.name))
    }) && !can_access_assets(user, script_uri, &Capability::DeleteScripts)
    {
        return Err(AssetWriteError::AccessDenied(
            "Removing a script's entrypoint takes the right to delete the script".to_string(),
        ));
    }
    if files.is_empty() && options.delete.is_empty() {
        return Err(AssetWriteError::Validation(
            "No files to write: 'files' must contain at least one entry".to_string(),
        ));
    }
    if files.len() > options.max_files {
        return Err(AssetWriteError::Validation(format!(
            "Too many files in one batch: {} (max {})",
            files.len(),
            options.max_files
        )));
    }

    let mut prepared: Vec<(String, String, Vec<u8>, String)> = Vec::with_capacity(files.len());
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let mut total_bytes: usize = 0;

    for file in files {
        validate_asset_uri(&file.name)?;
        if !seen.insert(file.name.as_str()) {
            return Err(AssetWriteError::Validation(format!(
                "Duplicate file '{}' in batch",
                file.name
            )));
        }

        // Decoded at the request boundary, where the caller's spelling of it
        // is still visible; by here a file is bytes however it arrived.
        let content = file.content.clone();

        if content.len() > MAX_ASSET_BYTES {
            return Err(AssetWriteError::Validation(format!(
                "Asset '{}' too large (max 10MB)",
                file.name
            )));
        }
        total_bytes = total_bytes.saturating_add(content.len());
        if total_bytes > options.max_total_bytes {
            return Err(AssetWriteError::Validation(format!(
                "Batch too large: over {} bytes of content in one request",
                options.max_total_bytes
            )));
        }

        let digest = sha256_hex(&content);
        if let Some(expected) = &file.expected_sha256
            && !expected.eq_ignore_ascii_case(&digest)
        {
            return Err(AssetWriteError::Validation(format!(
                "Content of '{}' does not match the sha256 supplied for it (expected {}, got {})",
                file.name, expected, digest
            )));
        }

        let mimetype = match &file.mimetype {
            Some(mimetype) if !mimetype.trim().is_empty() => mimetype.clone(),
            _ => mimetype_for(&file.name).to_string(),
        };

        prepared.push((file.name.clone(), mimetype, content, digest));
    }

    // One read of what is stored, rather than one per file, to classify each
    // write and to drop the files that would rewrite identical bytes.
    let existing = repository::fetch_assets(script_uri);

    let now = std::time::SystemTime::now();
    let mut results = Vec::with_capacity(prepared.len());
    let mut to_write = Vec::new();

    for (name, mimetype, content, digest) in prepared {
        let current = existing.get(&name);
        let status = match current {
            Some(asset) if asset.content == content && asset.mimetype == mimetype => "unchanged",
            Some(_) => "updated",
            None => "created",
        };
        results.push(AssetWriteOutcome {
            name: name.clone(),
            sha256: digest,
            bytes: content.len(),
            status,
        });
        if status != "unchanged" {
            to_write.push(repository::Asset {
                uri: name.clone(),
                name: Some(name),
                mimetype,
                content,
                created_at: current.map(|asset| asset.created_at).unwrap_or(now),
                updated_at: now,
                script_uri: script_uri.to_string(),
            });
        }
    }

    // Writes and removals in one transaction, so the tree either becomes what
    // the caller described or stays exactly as it was. Doing the two in
    // sequence left a window where a process dying between them kept files the
    // caller had asked to drop.
    let written = to_write.len();
    let deleted = if written > 0 || !options.delete.is_empty() {
        repository::sync_assets(script_uri, to_write, options.delete.to_vec())
            .map_err(|e| AssetWriteError::Storage(format!("Error writing assets: {}", e)))?
    } else {
        0
    };

    // One audit event for the batch, not one per file: the batch is the act.
    let auditor = auditor();
    let user_id = user.user_id.clone();
    let script_uri_owned = script_uri.to_string();
    let names = results
        .iter()
        .map(|result| result.name.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let file_count = results.len();
    tokio::task::spawn(async move {
        let _ = auditor
            .log_event(
                SecurityEvent::new(
                    SecurityEventType::SystemSecurityEvent,
                    SecuritySeverity::Medium,
                    user_id,
                )
                .with_resource("asset".to_string())
                .with_action("batch_upsert_for_uri".to_string())
                .with_detail("script_uri", &script_uri_owned)
                .with_detail("file_count", file_count.to_string())
                .with_detail("written", written.to_string())
                .with_detail("content_size", total_bytes.to_string())
                .with_detail("uris", &names),
            )
            .await;
    });

    // A write that changed nothing left the script exactly as the previous
    // revision already describes, so there is no new state to record.
    let revision = (options.record_revision && (written > 0 || deleted > 0))
        .then(|| revisions::record_blocking(script_uri, options.origin, user.user_id.as_deref()))
        .flatten();

    Ok(BatchWriteOutcome {
        results,
        revision,
        written,
        deleted,
    })
}

/// One change to a script's files, and whatever the change removes.
///
/// There is no separate root here, and that is the point. A script's
/// entrypoint is the file named `main.*` in its tree, so writing it is one
/// entry of `writes` like any other — which is what lets a change that
/// rewrites the entrypoint and two of its modules be one request, one
/// transaction and one revision, rather than a batch with a special case
/// bolted to the front of it.
pub struct ScriptFilesChange<'a> {
    pub writes: &'a [AssetWrite],
    /// Asset paths that must not survive this change.
    pub delete: &'a [String],
}

/// What a change to a script's files needs beyond the files themselves.
///
/// Deliberately not [`AssetSyncOptions`], which also carries the removals and
/// whether to record a revision: a change already names its own removals, and
/// recording the revision is what [`write_script_files_authorized`] is for, so
/// taking those here would be taking two answers to each question.
pub struct ScriptWriteOptions {
    /// How the resulting revision describes where the change came from.
    pub origin: revisions::Origin,
    /// Ceiling on this change's total asset content.
    pub max_total_bytes: usize,
    /// Ceiling on how many assets it may carry.
    pub max_files: usize,
}

impl Default for ScriptWriteOptions {
    /// What a change arriving over HTTP or MCP asks for: the ceilings that
    /// bound a request, and a history that calls it a batch.
    fn default() -> Self {
        Self {
            origin: revisions::Origin::Batch,
            max_total_bytes: MAX_BATCH_BYTES,
            max_files: MAX_BATCH_FILES,
        }
    }
}

/// What a change to a script's files did.
pub struct ScriptFilesOutcome {
    /// Whether the change stored a new entrypoint.
    ///
    /// Derived from the per-file results rather than tracked separately: the
    /// root is one of the files, so this is a question about the list.
    pub root_changed: bool,
    /// `inserted` when the change created the script, `updated` when it did
    /// not, and `None` when the change carried no root and so could not have
    /// created one.
    pub action: Option<UpsertAction>,
    pub assets: BatchWriteOutcome,
    /// The one revision describing everything this change did, or `None` when
    /// it changed nothing.
    pub revision: Option<i32>,
}

impl ScriptFilesOutcome {
    /// Whether anything reached storage — which is also whether re-initializing
    /// the script afterwards would be anything but cost.
    pub fn changed(&self) -> bool {
        self.assets.written > 0 || self.assets.deleted > 0
    }
}

/// Fold a caller's `content` argument into `writes` as the script's root file.
///
/// `content` is the older spelling of "and also the entrypoint", from when the
/// root was a column and a batch could not carry it as a file. It is kept
/// because it is what callers send, and it is sugar now rather than a second
/// write path: the text becomes one more entry of the batch, named after
/// whichever root file the script already has, or after the name a first write
/// would give it.
///
/// A caller who sends `content` *and* names a root file in `files` has
/// described the entrypoint twice, and the batch refuses the duplicate rather
/// than picking one.
pub(super) fn with_root_content(
    writes: &mut Vec<AssetWrite>,
    script_uri: &str,
    content: Option<&str>,
) {
    let Some(content) = content else {
        return;
    };
    let name = crate::module_loader::root_module_path_in(
        script_uri,
        &crate::source_view::SourceView::Live,
    )
    .unwrap_or_else(|_| crate::module_loader::default_root_module_name(script_uri).to_string());
    writes.push(AssetWrite {
        name,
        mimetype: None,
        content: content.as_bytes().to_vec(),
        expected_sha256: None,
    });
}

/// Which of `writes` is the change's root module, if any.
///
/// The first by [`crate::module_loader::ROOT_MODULE_NAMES`] order, which is
/// the rule the tree itself is read by, so a change carrying both a `main.ts`
/// and a `main.js` resolves the same way the resulting tree would.
pub(super) fn root_write(writes: &[AssetWrite]) -> Option<&AssetWrite> {
    crate::module_loader::ROOT_MODULE_NAMES
        .iter()
        .find_map(|name| writes.iter().find(|write| write.name == *name))
}

/// Write a script's files as one change: one transaction per store, one
/// revision, one `init()`.
///
/// A script's modules were already one unit of change; its root source was
/// not. A change that touches both had to be two writes — two revisions, two
/// cluster notifications, two `init()` runs, and a window in which the
/// deployment is half-changed — even though `/engine/check_script` would check
/// exactly such a change in one request. That asymmetry is what this removes:
/// the request that describes a change is now the request that applies it.
///
/// What remains asymmetric is the script *row*, and only the row. Every file
/// references it, so a change carrying the first file of a script that does
/// not exist yet has to create it first — and creating a script is a
/// different permission from writing to one that is already there, which is
/// why that step is authorized separately and why a change with no root
/// cannot bring a script into being.
///
/// `record_revision: false` on the file write is what keeps the change one
/// revision rather than two, and is why a caller cannot simply call the
/// pieces in sequence: the revision is recorded here, once, after everything
/// has landed. A change that stored nothing records none at all, since the
/// previous revision already describes exactly this content.
pub fn write_script_files_authorized(
    user: &UserContext,
    script_uri: &str,
    change: ScriptFilesChange<'_>,
    options: ScriptWriteOptions,
) -> Result<ScriptFilesOutcome, AssetWriteError> {
    // The script row has to exist before any of its files can, since every
    // file references it. `authorize_script_write` is what decides whether
    // this caller may create or replace this script at all — a stricter
    // question than "may they write its assets", and the one writing a root
    // has always been asked.
    let action = match root_write(change.writes) {
        None => None,
        Some(root) => {
            if root.content.is_empty() {
                return Err(AssetWriteError::Validation(format!(
                    "Script '{}' has no content: '{}' is empty",
                    script_uri, root.name
                )));
            }
            let existed = authorize_script_write(user, script_uri).map_err(|message| {
                let message = message
                    .strip_prefix("Error: ")
                    .unwrap_or(&message)
                    .to_string();
                AssetWriteError::AccessDenied(message)
            })?;
            repository::ensure_script(script_uri, user.user_id.as_deref())
                .map_err(|e| AssetWriteError::Storage(format!("Error storing script: {}", e)))?;
            Some(if existed {
                UpsertAction::Updated
            } else {
                UpsertAction::Inserted
            })
        }
    };

    // The file write refuses an empty batch — right for a caller who sent an
    // empty request, and there is nothing else for this to be now that a root
    // is one of the files.
    let assets = if change.writes.is_empty() && change.delete.is_empty() {
        BatchWriteOutcome {
            results: Vec::new(),
            revision: None,
            written: 0,
            deleted: 0,
        }
    } else {
        upsert_assets_synced(
            user,
            script_uri,
            change.writes,
            AssetSyncOptions {
                delete: change.delete,
                origin: options.origin,
                max_total_bytes: options.max_total_bytes,
                max_files: options.max_files,
                // One revision covers the whole change; it is recorded below,
                // once every file has landed.
                record_revision: false,
            },
        )?
    };

    let root_changed = assets.results.iter().any(|result| {
        crate::module_loader::is_root_module_name(&result.name) && result.status != "unchanged"
    });

    let mut outcome = ScriptFilesOutcome {
        root_changed,
        action,
        assets,
        revision: None,
    };
    if outcome.changed() {
        outcome.revision =
            revisions::record_blocking(script_uri, options.origin, user.user_id.as_deref());
    }
    Ok(outcome)
}

/// One string replacement of a patch — of one of a script's assets, or of its
/// root source. Editing either is the same act on the same kind of content, so
/// both take the same request shape and the same checks.
pub struct StringEdit {
    /// Text to find. It must be present, and unique unless `replace_all`.
    pub old_string: String,
    /// Text to put in its place.
    pub new_string: String,
    /// Replace every occurrence rather than requiring exactly one.
    pub replace_all: bool,
}

/// What a patch did to the file it edited.
pub struct PatchOutcome {
    /// Digest of the content as it now stands, which the next patch can send
    /// back as `base_sha256`.
    pub sha256: String,
    /// The revision this patch produced, or `None` when the edits cancelled
    /// each other out and nothing was stored.
    pub revision: Option<i32>,
    pub bytes: usize,
    /// How many occurrences the edits replaced in total.
    pub replacements: usize,
    /// `updated`, or `unchanged` when the edits cancelled each other out.
    pub status: &'static str,
}

impl PatchOutcome {
    pub(super) fn to_json(&self) -> Value {
        json!({
            "sha256": self.sha256,
            "bytes": self.bytes,
            "replacements": self.replacements,
            "status": self.status,
            "revision": self.revision,
        })
    }
}

pub enum PatchError {
    /// Carries why, because the two ways to be refused a write are not the
    /// same thing to be told: lacking the capability is a different problem
    /// from holding it and not owning this script.
    AccessDenied(String),
    NotFound,
    Validation(String),
    /// The stored file is not the one the caller edited: `base_sha256` names
    /// content it no longer has.
    Conflict {
        expected: String,
        actual: String,
    },
    Storage(String),
}

/// Apply a patch's edits to `text`, reporting how many occurrences they
/// replaced in total.
///
/// This is the half of a patch that is the same whether the file being edited
/// is one of a script's assets or its root source, so it lives in one place:
/// the arithmetic of finding and replacing, and the rule that keeps an edit
/// aimed by content alone from being a guess. An `old_string` that appears
/// more than once is refused unless the caller said `replace_all`, because an
/// edit meant for one of three identical lines cannot be aimed by content.
///
/// `file` is only ever quoted back in the refusals. It is what a caller needs
/// to know which of the files it sent is the one that did not match.
///
/// The edits are applied to a copy the caller owns, so a patch whose third
/// edit does not match has written nothing anywhere.
pub(super) fn apply_string_edits(
    text: &mut String,
    edits: &[StringEdit],
    file: &str,
) -> Result<usize, String> {
    if edits.is_empty() {
        return Err("No edits to apply: 'edits' must contain at least one entry".to_string());
    }
    if edits.len() > MAX_PATCH_EDITS {
        return Err(format!(
            "Too many edits in one patch: {} (max {})",
            edits.len(),
            MAX_PATCH_EDITS
        ));
    }

    let mut replacements = 0usize;
    for (index, edit) in edits.iter().enumerate() {
        if edit.old_string.is_empty() {
            return Err(format!("edits[{}]: old_string must not be empty", index));
        }
        if edit.old_string == edit.new_string {
            return Err(format!(
                "edits[{}]: old_string and new_string are identical, so the edit would do nothing",
                index
            ));
        }

        let occurrences = text.matches(edit.old_string.as_str()).count();
        match (occurrences, edit.replace_all) {
            (0, _) => {
                return Err(format!(
                    "edits[{}]: old_string was not found in '{}'{}",
                    index,
                    file,
                    if index > 0 {
                        " as the earlier edits left it"
                    } else {
                        ""
                    }
                ));
            }
            (count, false) if count > 1 => {
                return Err(format!(
                    "edits[{}]: old_string appears {} times in '{}'; include enough surrounding \
                     text to make it unique, or pass replace_all",
                    index, count, file
                ));
            }
            (count, true) => {
                *text = text.replace(edit.old_string.as_str(), &edit.new_string);
                replacements += count;
            }
            (_, false) => {
                *text = text.replacen(edit.old_string.as_str(), &edit.new_string, 1);
                replacements += 1;
            }
        }
    }

    Ok(replacements)
}

/// Delete one file of a script.
pub fn delete_asset_authorized(
    user: &UserContext,
    script_uri: &str,
    asset_uri: &str,
) -> Result<(bool, Option<i32>), AssetFetchError> {
    // Removing a script's entrypoint is removing its source, so it takes what
    // removing a script takes. The write side already draws this line —
    // `WriteAssets` is not a way to write a root — and leaving the delete side
    // at `DeleteAssets` would make the tree merge a way around it: what you
    // could not overwrite you could delete.
    let required = if crate::module_loader::is_root_module_name(asset_uri) {
        Capability::DeleteScripts
    } else {
        Capability::DeleteAssets
    };
    if !can_access_assets(user, script_uri, &required) {
        let auditor = auditor();
        let user_id = user.user_id.clone();
        tokio::task::spawn(async move {
            let _ = auditor
                .log_authz_failure(
                    user_id,
                    "asset".to_string(),
                    "delete_for_uri".to_string(),
                    format!("{:?}", required),
                )
                .await;
        });
        return Err(AssetFetchError::AccessDenied);
    }

    let auditor = auditor();
    let user_id = user.user_id.clone();
    let script_uri_owned = script_uri.to_string();
    let asset_uri_owned = asset_uri.to_string();
    tokio::task::spawn(async move {
        let _ = auditor
            .log_event(
                SecurityEvent::new(
                    SecurityEventType::SystemSecurityEvent,
                    SecuritySeverity::High,
                    user_id,
                )
                .with_resource("asset".to_string())
                .with_action("delete_for_uri".to_string())
                .with_detail("uri", &asset_uri_owned)
                .with_detail("script_uri", &script_uri_owned),
            )
            .await;
    });

    let deleted = repository::delete_asset(script_uri, asset_uri);
    // Removing a file is a change like any other, and the one most worth being
    // able to undo: nothing else in the engine still holds the content.
    let revision = deleted
        .then(|| {
            revisions::record_blocking(
                script_uri,
                revisions::Origin::Delete,
                user.user_id.as_deref(),
            )
        })
        .flatten();

    Ok((deleted, revision))
}

/// One edit of a patch request.
#[derive(Deserialize, Default)]
pub struct StringEditBody {
    pub(super) old_string: Option<String>,
    pub(super) new_string: Option<String>,
    #[serde(default)]
    pub(super) replace_all: bool,
}

/// Turn a request's `edits` into the patch's, or name the entry that is not
/// one.
///
/// Both fields are required rather than defaulted to empty, so a misspelled
/// field name cannot quietly turn a replacement into a deletion — which is the
/// one mistake here that destroys content rather than being refused.
pub(super) fn prepare_edits(edits: Vec<StringEditBody>) -> Result<Vec<StringEdit>, String> {
    edits
        .into_iter()
        .enumerate()
        .map(|(index, edit)| {
            let old_string = edit
                .old_string
                .ok_or_else(|| format!("edits[{}]: missing required field: old_string", index))?;
            let new_string = edit.new_string.ok_or_else(|| {
                format!(
                    "edits[{}]: missing required field: new_string (pass \"\" to delete the text)",
                    index
                )
            })?;
            Ok(StringEdit {
                old_string,
                new_string,
                replace_all: edit.replace_all,
            })
        })
        .collect()
}

/// The scoping a read asked for, or why it does not parse.
pub(super) fn read_options(
    lines: Option<&str>,
    grep: Option<String>,
) -> Result<FileReadOptions, String> {
    let lines = match lines {
        Some(raw) => Some(LineRange::parse(raw)?),
        None => None,
    };
    Ok(FileReadOptions { lines, grep })
}

/// The answer a change to a script's files gives, shared by the endpoint and
/// the tool so the two cannot drift.
pub(super) fn batch_outcome_json(script: &str, outcome: &ScriptFilesOutcome, init: Value) -> Value {
    let mut body = json!({
        "script": script,
        "results": outcome.assets.results.iter().map(AssetWriteOutcome::to_json).collect::<Vec<Value>>(),
        "written": outcome.assets.written,
        "revision": outcome.revision,
        "init": init,
        "timestamp": iso_timestamp(),
    });
    if let Some(object) = body.as_object_mut() {
        // Only reported when the change carried them, so a caller written
        // against the asset-only batch sees exactly the body it always saw.
        if outcome.action.is_some() {
            object.insert(
                "root".to_string(),
                json!(if outcome.root_changed {
                    "updated"
                } else {
                    "unchanged"
                }),
            );
        }
        if outcome.assets.deleted > 0 {
            object.insert("deleted".to_string(), json!(outcome.assets.deleted));
        }
        if let Some(note) = deployment_note(script) {
            object.insert("deployment".to_string(), note);
        }
    }
    body
}

pub(super) fn tool_list_files(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let files = list_assets_authorized(user, script);
    json!({
        "script": script,
        "files": files,
        "count": files.len(),
        "timestamp": iso_timestamp(),
    })
}

pub(super) fn tool_read_file(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let Some(path) = arg_str(args, "path") else {
        return missing_arg("path");
    };
    let options = match read_options(
        arg_str(args, "lines"),
        arg_str(args, "grep").map(str::to_string),
    ) {
        Ok(options) => options,
        Err(message) => return refuse_text(message),
    };
    match read_file_authorized(user, script, path, &options) {
        Ok(read) => {
            let mut body = read.to_json();
            if let Some(object) = body.as_object_mut() {
                object.insert("script".to_string(), json!(script));
                object.insert("path".to_string(), json!(path));
                // What a pinned script serves is not what this read answered
                // with, and that is worth saying wherever it is true rather
                // than only for the entrypoint, since a module is pinned
                // with it.
                if let Some(note) = deployment_note(script) {
                    object.insert("deployment".to_string(), note);
                }
                object.insert("timestamp".to_string(), json!(iso_timestamp()));
            }
            body
        }
        Err(FileReadError::AccessDenied) => refuse(Refusal::Forbidden, "Error: Access denied"),
        Err(FileReadError::NotFound) => {
            refuse(Refusal::NotFound, format!("File not found: {}", path))
        }
        Err(FileReadError::Validation(message)) => refuse(Refusal::BadRequest, message),
    }
}

pub(super) fn tool_write_file(args: &Value, user: &UserContext) -> Value {
    write_one_file(args, user, false)
}

/// `write_file` and `create_file`, which differ only in whether an existing
/// path is an error. That was the whole of `create_asset`'s body, repeated.
pub(super) fn write_one_file(args: &Value, user: &UserContext, if_absent: bool) -> Value {
    let verb = if if_absent { "create" } else { "write" };
    let done = if if_absent { "created" } else { "written" };
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let Some(path) = arg_str(args, "path") else {
        return missing_arg("path");
    };
    // Inferred rather than required: a caller writing `lib/util.ts` should
    // not have to know what the engine calls a TypeScript file.
    let mimetype = arg_str(args, "mimetype")
        .map(str::to_string)
        .unwrap_or_else(|| mimetype_for(path).to_string());
    let content = match asset_content_from_request(
        &format!("file '{}'", path),
        "content",
        arg_str(args, "text").map(str::to_string),
        arg_str(args, "content").map(str::to_string),
    ) {
        Ok(content) => content,
        Err(message) => return refuse_text(message),
    };

    match write_file_bytes_authorized(user, script, path, &mimetype, content, if_absent) {
        Ok(revision) => json!({
            "success": true,
            "message": format!("File '{}' {} successfully", path, done),
            "script": script,
            "path": path,
            "revision": revision,
            "timestamp": iso_timestamp(),
        }),
        Err(AssetWriteError::Exists(path)) => {
            refuse(Refusal::Conflict, format!("File already exists: {}", path))
        }
        Err(AssetWriteError::AccessDenied(message)) => refuse(
            Refusal::Forbidden,
            format!("Failed to {} file: {}", verb, message),
        ),
        Err(AssetWriteError::Validation(msg)) => refuse(
            Refusal::BadRequest,
            format!("Failed to {} file: {}", verb, msg),
        ),
        Err(AssetWriteError::Storage(msg)) => {
            refuse(Refusal::Failed, format!("Failed to {} file: {}", verb, msg))
        }
    }
}

/// Write several of a script's assets as one unit — the MCP face of
/// [`assets_batch_route`], down to the shape of its answer.
pub(super) fn tool_write_files(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let root = arg_str(args, "content");
    let remove: Vec<String> = match args.get("remove") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(paths)) => {
            let mut collected = Vec::with_capacity(paths.len());
            for (index, path) in paths.iter().enumerate() {
                let Some(path) = path.as_str() else {
                    return refuse(
                        Refusal::BadRequest,
                        format!("remove[{}]: expected an asset path", index),
                    );
                };
                collected.push(path.to_string());
            }
            collected
        }
        Some(_) => {
            return refuse(
                Refusal::BadRequest,
                "'remove' must be an array of asset paths",
            );
        }
    };
    let empty: Vec<Value> = Vec::new();
    let files = match args.get("files") {
        Some(Value::Array(files)) => files,
        None | Some(Value::Null) if root.is_some() || !remove.is_empty() => &empty,
        _ => return missing_arg("files"),
    };
    let reinit = match ReinitMode::parse(arg_str(args, "reinit")) {
        Ok(reinit) => reinit,
        Err(message) => return refuse_text(message),
    };

    let mut writes = Vec::with_capacity(files.len());
    for (index, file) in files.iter().enumerate() {
        let Some(name) = arg_str(file, "name").or_else(|| arg_str(file, "asset")) else {
            return refuse(
                Refusal::BadRequest,
                format!("files[{}]: missing required field: name", index),
            );
        };
        let content = match asset_content_from_request(
            &format!("files[{}] ('{}')", index, name),
            "content_base64",
            arg_str(file, "text")
                .or_else(|| arg_str(file, "source"))
                .map(str::to_string),
            arg_str(file, "content_base64")
                .or_else(|| arg_str(file, "content"))
                .map(str::to_string),
        ) {
            Ok(content) => content,
            Err(message) => return refuse_text(message),
        };
        writes.push(AssetWrite {
            name: name.to_string(),
            mimetype: arg_str(file, "mimetype").map(str::to_string),
            content,
            expected_sha256: arg_str(file, "sha256").map(str::to_string),
        });
    }

    with_root_content(&mut writes, script, root);

    match write_script_files_authorized(
        user,
        script,
        ScriptFilesChange {
            writes: &writes,
            delete: &remove,
        },
        ScriptWriteOptions::default(),
    ) {
        Ok(outcome) => {
            // Bridging back to async, as `check_script` does, so the caller is
            // told what init() did rather than that it was started.
            let init = match (reinit, outcome.changed()) {
                (ReinitMode::Never, _) => json!({ "ran": false, "reason": "reinit=never" }),
                (ReinitMode::After, false) => {
                    json!({ "ran": false, "reason": "no files changed" })
                }
                (ReinitMode::After, true) => {
                    crate::database::run_blocking(reinitialize_after_write(script))
                }
            };
            let check = outcome
                .changed()
                .then(|| check_after_write(script, user, args))
                .flatten();
            let mut body = batch_outcome_json(script, &outcome, init);
            if let Some(object) = body.as_object_mut() {
                object.insert("success".to_string(), json!(true));
                if let Some(check) = check {
                    object.insert("check".to_string(), check);
                }
            }
            body
        }
        Err(AssetWriteError::Exists(asset)) => refuse(
            Refusal::Conflict,
            format!("Asset already exists: {}", asset),
        ),
        Err(AssetWriteError::AccessDenied(message)) => refuse(
            Refusal::Forbidden,
            format!("Failed to write assets: {}", message),
        ),
        Err(AssetWriteError::Validation(msg)) => refuse(
            Refusal::BadRequest,
            format!("Failed to write assets: {}", msg),
        ),
        Err(AssetWriteError::Storage(msg)) => {
            refuse(Refusal::Failed, format!("Failed to write assets: {}", msg))
        }
    }
}

pub(super) fn tool_create_file(args: &Value, user: &UserContext) -> Value {
    write_one_file(args, user, true)
}

pub(super) fn tool_edit_file(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let Some(path) = arg_str(args, "path") else {
        return missing_arg("path");
    };
    let Some(edits) = args.get("edits").and_then(Value::as_array) else {
        return missing_arg("edits");
    };
    let reinit = match ReinitMode::parse(arg_str(args, "reinit")) {
        Ok(reinit) => reinit,
        Err(message) => return refuse_text(message),
    };

    let prepared = match parse_tool_edits(edits) {
        Ok(prepared) => prepared,
        Err(message) => return refuse_text(message),
    };

    match patch_file_authorized(
        user,
        script,
        path,
        &prepared,
        arg_str(args, "base_sha256").or_else(|| arg_str(args, "sha256")),
        Some("mcp"),
    ) {
        Ok(outcome) => {
            // Bridging back to async, as write_files does, so the caller is
            // told what init() did rather than that it was started.
            let init = match (reinit, outcome.status) {
                (ReinitMode::Never, _) => json!({ "ran": false, "reason": "reinit=never" }),
                (_, "unchanged") => json!({ "ran": false, "reason": "no change" }),
                (ReinitMode::After, _) => {
                    crate::database::run_blocking(reinitialize_after_write(script))
                }
            };
            let check = (outcome.status != "unchanged")
                .then(|| check_after_write(script, user, args))
                .flatten();
            let mut body = outcome.to_json();
            if let Some(object) = body.as_object_mut() {
                object.insert("success".to_string(), json!(true));
                object.insert("script".to_string(), json!(script));
                object.insert("path".to_string(), json!(path));
                object.insert("init".to_string(), init);
                if let Some(check) = check {
                    object.insert("check".to_string(), check);
                }
                if let Some(note) = deployment_note(script) {
                    object.insert("deployment".to_string(), note);
                }
                object.insert("timestamp".to_string(), json!(iso_timestamp()));
            }
            body
        }
        Err(PatchError::AccessDenied(message)) => refuse(
            Refusal::Forbidden,
            format!("Failed to edit file: {}", message),
        ),
        Err(PatchError::NotFound) => refuse(Refusal::NotFound, format!("File not found: {}", path)),
        Err(PatchError::Conflict { expected, actual }) => json!({
            "status": Refusal::Conflict.status(),
            "error": format!(
                "'{}' has changed since it was read (expected {}, stored {})",
                path, expected, actual
            ),
            "script": script,
            "path": path,
            "expected_sha256": expected,
            "sha256": actual,
        }),
        Err(PatchError::Validation(message)) => refuse(
            Refusal::BadRequest,
            format!("Failed to edit file: {}", message),
        ),
        Err(PatchError::Storage(message)) => {
            refuse(Refusal::Failed, format!("Failed to edit file: {}", message))
        }
    }
}

pub(super) fn tool_delete_file(args: &Value, user: &UserContext) -> Value {
    let Some(script) = arg_str(args, "script") else {
        return missing_arg("script");
    };
    let Some(path) = arg_str(args, "path") else {
        return missing_arg("path");
    };
    match delete_asset_authorized(user, script, path) {
        Ok((true, revision)) => json!({
            "success": true,
            "message": format!("File '{}' deleted successfully", path),
            "script": script,
            "path": path,
            "revision": revision,
            "timestamp": iso_timestamp(),
        }),
        Ok((false, _)) => refuse(Refusal::NotFound, format!("File '{}' not found", path)),
        Err(_) => refuse(Refusal::Forbidden, "Failed to delete file: Access denied"),
    }
}
