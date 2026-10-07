//! Searching the files of the scripts a caller may read.

use super::*;
use crate::repository;
use crate::security::{Capability, UserContext};
use serde_json::{Value, json};

/// Which of a script's files a search reads.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SearchScope {
    /// Root sources and assets alike — what "search my code" means, since a
    /// solution's code is mostly its modules.
    All,
    /// Entrypoints only.
    Scripts,
    /// Assets only.
    Assets,
}

impl SearchScope {
    pub(super) fn parse(value: Option<&str>) -> Result<Self, String> {
        match value {
            None | Some("all") => Ok(SearchScope::All),
            Some("scripts") => Ok(SearchScope::Scripts),
            Some("assets") => Ok(SearchScope::Assets),
            Some(other) => Err(format!(
                "Invalid scope '{}': expected 'all', 'scripts' or 'assets'",
                other
            )),
        }
    }

    pub(super) fn reads_scripts(self) -> bool {
        self != SearchScope::Assets
    }

    pub(super) fn reads_assets(self) -> bool {
        self != SearchScope::Scripts
    }
}

/// How a search across a deployment's files is narrowed.
pub struct SearchOptions {
    pub case_insensitive: bool,
    pub scope: SearchScope,
    /// One script's files rather than every script's.
    pub script: Option<String>,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            // The default this search has always had. A caller looking for a
            // symbol usually does not know how it is capitalised.
            case_insensitive: true,
            scope: SearchScope::All,
            script: None,
        }
    }
}

/// How many files one search reports before it stops looking.
pub const MAX_SEARCH_FILES: usize = 200;

/// How many matching lines a search reports per file.
pub const MAX_SEARCH_MATCHES_PER_FILE: usize = 50;

/// Search a deployment's files for a pattern.
///
/// The counterpart of `read_asset`'s `grep`, which searches one named file: this
/// is for the caller that does not yet know which file to name. It read only
/// root sources, which is the smaller half of a solution — the modules are
/// where the code is — so "which file mentions `movePlayer`" meant listing every
/// script's assets and fetching each one.
///
/// A binary asset is skipped rather than refused: a search across a tree that
/// happens to contain a PNG is a reasonable thing to ask for, and failing the
/// whole search because of it would not be.
pub fn search_files_authorized(
    user: &UserContext,
    pattern: &str,
    options: &SearchOptions,
) -> Result<Value, String> {
    if pattern.is_empty() {
        return Err("Empty search pattern: there is nothing to search for".to_string());
    }
    if pattern.chars().count() > MAX_GREP_PATTERN_CHARS {
        return Err(format!(
            "Search pattern too long (max {} characters)",
            MAX_GREP_PATTERN_CHARS
        ));
    }
    let regex = regex::RegexBuilder::new(pattern)
        .case_insensitive(options.case_insensitive)
        .size_limit(1 << 20)
        .dfa_size_limit(1 << 20)
        .build()
        .map_err(|e| format!("Invalid search pattern: {}", e))?;

    let matches_in = |text: &str| -> Vec<Value> {
        text.lines()
            .enumerate()
            .filter(|(_, line)| regex.is_match(line))
            .take(MAX_SEARCH_MATCHES_PER_FILE)
            .map(|(index, line)| {
                json!({
                    "line": index + 1,
                    "content": line.trim(),
                    "preview": line.chars().take(200).collect::<String>(),
                })
            })
            .collect()
    };

    let mut results: Vec<Value> = Vec::new();
    let mut truncated = false;
    for meta in list_scripts_authorized(user) {
        if options
            .script
            .as_deref()
            .is_some_and(|script| script != meta.uri)
        {
            continue;
        }
        if truncated {
            break;
        }

        // Reading a script's assets through `/engine/*` is a permission of its
        // own, so a caller who may list the scripts is still asked for it
        // before their modules are searched. The entrypoint is not one of
        // those: it is the script's source, and reading it is what listing the
        // script already granted.
        let may_read_assets = options.scope.reads_assets()
            && can_access_assets(user, &meta.uri, &Capability::ReadAssets);

        // One pass over the tree, the entrypoint included, so it is reported
        // once.
        let mut files: Vec<(String, Vec<u8>)> = repository::fetch_assets(&meta.uri)
            .into_iter()
            .map(|(name, asset)| (name, asset.content))
            .collect();
        // A stable order, so the same search reports the same list twice:
        // `fetch_assets` hands back a map.
        files.sort_by(|(left, _), (right, _)| {
            let rank = |name: &str| u8::from(!crate::module_loader::is_root_module_name(name));
            rank(left).cmp(&rank(right)).then_with(|| left.cmp(right))
        });

        for (name, content) in files {
            if results.len() >= MAX_SEARCH_FILES {
                truncated = true;
                break;
            }
            let allowed = if crate::module_loader::is_root_module_name(&name) {
                options.scope.reads_scripts()
            } else {
                may_read_assets
            };
            if !allowed {
                continue;
            }
            let Ok(text) = std::str::from_utf8(&content) else {
                continue;
            };
            let matches = matches_in(text);
            if !matches.is_empty() {
                results.push(json!({
                    "uri": meta.uri,
                    "asset": name,
                    "matchCount": matches.len(),
                    "matches": matches,
                }));
            }
        }
    }

    Ok(json!({
        "query": pattern,
        "caseInsensitive": options.case_insensitive,
        "filesMatched": results.len(),
        "truncated": truncated,
        "results": results,
        "timestamp": iso_timestamp(),
    }))
}

pub(super) fn tool_search_files(args: &Value, user: &UserContext) -> Value {
    let Some(query) = arg_str(args, "query") else {
        return missing_arg("query");
    };
    let scope = match SearchScope::parse(arg_str(args, "scope")) {
        Ok(scope) => scope,
        Err(message) => return refuse_text(message),
    };
    let options = SearchOptions {
        case_insensitive: args
            .get("caseInsensitive")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        scope,
        script: arg_str(args, "script").map(str::to_string),
    };

    match search_files_authorized(user, query, &options) {
        Ok(body) => body,
        Err(message) => refuse(
            Refusal::from_message(&message),
            format!("Failed to search files: {}", message),
        ),
    }
}
