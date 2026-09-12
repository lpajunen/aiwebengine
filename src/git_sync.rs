//! Turning a repository's tree into scripts the engine can run.
//!
//! The mapping is inferred from the directory structure rather than declared in
//! a manifest, and that is a correctness decision before it is a convenience
//! one. A manifest's plausible payload is two things, and this feature rules
//! out both:
//!
//! - **The script URI cannot travel.** One engine composes
//!   `https://her-host/shop/main.ts`; the person pulling the same repository on
//!   their own install has a different `server.base_url` entirely. A repository
//!   that records the full URI records a value correct on exactly one machine.
//!   What travels is the script's *name*, and the pulling engine composes the
//!   local URI from its own host.
//! - **The authorization fields cannot travel either.** Owners, host bindings
//!   and deployment pins arriving from a repository would mean push access to
//!   that repository confers ownership and host binding inside somebody's
//!   engine.
//!
//! Strip both out and nothing is left for a manifest to carry — which is
//! convenient, because the directory structure already *is* the mapping. Asset
//! names are relative paths and import specifiers are relative
//! ([`crate::module_loader`]), so a repository subtree is byte-identical to what
//! the engine stores, in both directions, with no specifier rewriting.

use std::collections::{BTreeMap, BTreeSet};

use thiserror::Error;

use crate::security::capabilities::UserContext;

/// Ceiling on a repository's tree once expanded.
///
/// Separate from the ceiling on the download, and necessarily so: a small
/// archive can expand without bound, and the only honest place to measure that
/// is while expanding it.
pub const MAX_TREE_BYTES: usize = 48 * 1024 * 1024;

/// Ceiling on one file, matching what the asset write enforces. A repository
/// carrying something larger has a file the engine could not store anyway, and
/// saying so while reading is clearer than failing at the write.
pub const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;

/// Ceiling on how many files one repository may map to.
///
/// Generous next to the 256 a single batch write accepts, because a repository
/// holding several scripts is spread over several batches — one per script.
pub const MAX_TREE_FILES: usize = 4096;

/// Basenames that mark a script's entry point, in preference order.
///
/// A fixed name rather than "the only source file at the top level", because a
/// script legitimately keeps a top-level `util.ts` as an asset. This is also
/// what a future push writes, so both directions agree without negotiation.
const ENTRY_NAMES: [&str; 4] = ["main.ts", "main.js", "main.tsx", "main.jsx"];

/// Version of the rules that turn a repository into script URIs.
///
/// Recorded on every sync row so that a pull can tell whether the row in front
/// of it was written by the same rules it is about to apply. Bump it whenever
/// [`compose_script_uri`] changes: an unmoved commit under unchanged rules is
/// genuinely nothing to do, and under changed rules it is everything to do.
///
/// 1. `{base}/{entry file name}` — every script called `main.js`.
/// 2. `{base}/{directory}{ext}`, or `{base}{ext}` for a repository that is one
///    script, so a script is named after where it came from.
pub const MAPPING_VERSION: i32 = 2;

/// Files skipped at a script's top level.
///
/// Only at the top level: a `README.md` beside the entry is repository
/// furniture, while one inside `docs/` may well be content the solution serves.
const TOP_LEVEL_SKIP: [&str; 5] = [
    "readme.md",
    "license",
    "license.md",
    "license.txt",
    "aiwebengine.json",
];

// ============================================================================
// What not to take
// ============================================================================

/// The file a repository uses to say which of its own files are not content.
///
/// Deliberately not `.gitignore`. Everything that file lists is by definition
/// uncommitted, so it never reaches the archive at all and honouring it would
/// be close to a no-op. The useful question is the other one — of the files git
/// *does* track, which should become engine assets — and it deserves its own
/// answer rather than a borrowed one.
pub const IGNORE_FILE: &str = ".aiwebengineignore";

/// One line of [`IGNORE_FILE`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct IgnorePattern {
    /// `!pattern` — puts a file back that an earlier pattern took out.
    negated: bool,
    /// `pattern/` — matches directories, and so their contents, but not a file
    /// of that name.
    dir_only: bool,
    /// Matches at any depth rather than from the repository root. True for a
    /// pattern with no slash in it, and for one written `**/name`, which is the
    /// same thing said explicitly.
    floating: bool,
    segments: Vec<String>,
}

/// The rules a repository ships, in the order it wrote them.
#[derive(Debug, Clone, Default)]
pub struct IgnoreRules {
    patterns: Vec<IgnorePattern>,
}

impl IgnoreRules {
    /// Read the rules out of the file's text.
    ///
    /// The syntax is the one people already know from `.gitignore`, because a
    /// file that looks like that one and behaves subtly unlike it is worse than
    /// no file at all: `#` comments, `*` and `?` within a segment, `**` across
    /// segments, a trailing `/` for directories, a leading `!` to re-include,
    /// and last match wins. A pattern with no slash matches at any depth; a
    /// pattern with one is anchored at the repository root.
    pub fn parse(text: &str) -> Self {
        Self {
            patterns: text.lines().filter_map(parse_pattern).collect(),
        }
    }

    /// What the rules say about a path, or `None` when none of them mention it.
    ///
    /// Three-valued because "no rule matched" and "a rule put this back" are
    /// different answers: the second overrides the built-in exclusions and the
    /// first leaves them to decide.
    fn verdict(&self, path: &str) -> Option<bool> {
        let segments: Vec<&str> = path.split('/').collect();
        let mut verdict = None;
        for pattern in &self.patterns {
            if pattern.matches(&segments) {
                verdict = Some(!pattern.negated);
            }
        }
        verdict
    }

    /// Whether the rules take this path out.
    fn hides(&self, path: &str) -> bool {
        self.verdict(path) == Some(true)
    }
}

fn parse_pattern(line: &str) -> Option<IgnorePattern> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return None;
    }

    let (negated, rest) = match line.strip_prefix('!') {
        Some(rest) => (true, rest.trim()),
        None => (false, line),
    };

    // `dir/` and `dir/**` say the same thing: the directory's contents.
    let (dir_only, rest) = match rest.strip_suffix("/**") {
        Some(rest) => (true, rest),
        None => match rest.strip_suffix('/') {
            Some(rest) => (true, rest),
            None => (false, rest),
        },
    };

    let rest = rest.strip_prefix('/').unwrap_or(rest);
    let (floating, rest) = match rest.strip_prefix("**/") {
        Some(rest) => (true, rest),
        None => (!rest.contains('/'), rest),
    };

    if rest.is_empty() {
        return None;
    }

    Some(IgnorePattern {
        negated,
        dir_only,
        floating,
        segments: rest.split('/').map(str::to_string).collect(),
    })
}

impl IgnorePattern {
    /// Whether this pattern covers `segments`.
    ///
    /// A file is covered when the pattern names the file itself *or any
    /// directory above it* — which is what makes `docs/` exclude everything
    /// underneath without the caller walking directories. `dir_only` is exactly
    /// the restriction that the match has to land on an ancestor rather than on
    /// the file.
    fn matches(&self, segments: &[&str]) -> bool {
        let limit = if self.dir_only {
            segments.len().saturating_sub(1)
        } else {
            segments.len()
        };

        (1..=limit).any(|depth| {
            let prefix = &segments[..depth];
            if self.floating {
                (0..depth).any(|start| glob_path(&self.segments, &prefix[start..]))
            } else {
                glob_path(&self.segments, prefix)
            }
        })
    }
}

/// Match pattern segments against path segments, `**` standing for any number
/// of segments including none.
fn glob_path(pattern: &[String], path: &[&str]) -> bool {
    match pattern.split_first() {
        None => path.is_empty(),
        Some((head, rest)) if head == "**" => {
            (0..=path.len()).any(|skip| glob_path(rest, &path[skip..]))
        }
        Some((head, rest)) => match path.split_first() {
            Some((segment, tail)) if glob_segment(head, segment) => glob_path(rest, tail),
            _ => false,
        },
    }
}

/// Match one path segment, where `*` stands for any run of characters within
/// the segment and `?` for exactly one.
fn glob_segment(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();

    let (mut p, mut t) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut resume = 0usize;

    while t < text.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            resume = t;
            p += 1;
        } else if let Some(position) = star {
            // Backtrack: let the last `*` swallow one more character.
            p = position + 1;
            resume += 1;
            t = resume;
        } else {
            return false;
        }
    }

    pattern[p..].iter().all(|c| *c == '*')
}

/// One script as a repository describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptLayout {
    /// The directory this script came from, or `None` when the repository is
    /// itself one script. The caller turns this into a local URI; it is
    /// deliberately not a URI here, because this type describes the repository
    /// and a URI describes an engine.
    pub name: Option<String>,
    /// Path of the entry within the repository, for error messages.
    pub entry_path: String,
    /// The entry's bytes, which become the script row rather than an asset.
    pub entry: Vec<u8>,
    /// Every other file, keyed by its path relative to the script's root —
    /// which is exactly the asset name the engine stores.
    pub assets: BTreeMap<String, Vec<u8>>,
}

impl ScriptLayout {
    /// The entry's file name within the repository.
    pub fn entry_file_name(&self) -> &str {
        self.entry_path
            .rsplit('/')
            .next()
            .unwrap_or(&self.entry_path)
    }

    /// The entry's extension, including the dot, or empty when it has none.
    ///
    /// This is the part of the file name a URI actually has to keep.
    /// [`crate::transpiler::transpile_if_needed`] decides whether a module is
    /// TypeScript or JSX from the extension alone and never reads the name in
    /// front of it, so a `.ts` entry served under a URI ending `.js` would be
    /// handed to the runtime untranspiled.
    pub fn entry_extension(&self) -> &str {
        let name = self.entry_file_name();
        match name.rfind('.') {
            Some(dot) => &name[dot..],
            None => "",
        }
    }
}

/// Expand a gzipped tar into the files it holds.
///
/// GitHub wraps every archive in a single `{owner}-{repo}-{sha}` directory.
/// That prefix is stripped rather than reconstructed from the repository name:
/// the archive states it, and requiring the two to agree would break on a
/// repository that has since been renamed.
///
/// Entries that are not regular files are dropped rather than refused —
/// directories carry no content, and a symlink in an archive is a traversal
/// primitive with no legitimate use in a script tree.
pub fn extract_tree(archive: &[u8]) -> Result<BTreeMap<String, Vec<u8>>, SyncError> {
    use std::io::Read;

    let decoder = flate2::read::GzDecoder::new(archive);
    let mut tar = tar::Archive::new(decoder);

    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let mut prefix: Option<String> = None;
    let mut total: usize = 0;

    let entries = tar
        .entries()
        .map_err(|e| SyncError::Archive(format!("Could not read the archive: {}", e)))?;

    for entry in entries {
        let mut entry =
            entry.map_err(|e| SyncError::Archive(format!("Could not read an entry: {}", e)))?;

        if entry.header().entry_type() != tar::EntryType::Regular {
            continue;
        }

        let path = entry
            .path()
            .map_err(|e| SyncError::Archive(format!("Entry has an unreadable path: {}", e)))?
            .to_string_lossy()
            .into_owned();

        let Some((first, rest)) = path.split_once('/') else {
            // A file at the archive's own root, outside the wrapper directory.
            // GitHub does not produce these, so something else did.
            return Err(SyncError::Archive(format!(
                "Unexpected file '{}' outside the archive's root directory",
                path
            )));
        };

        match &prefix {
            None => prefix = Some(first.to_string()),
            Some(known) if known == first => {}
            Some(known) => {
                return Err(SyncError::Archive(format!(
                    "Archive holds more than one root directory ('{}' and '{}')",
                    known, first
                )));
            }
        }

        if rest.is_empty() {
            continue;
        }
        validate_repo_path(rest)?;

        if files.len() >= MAX_TREE_FILES {
            return Err(SyncError::TooLarge(format!(
                "Repository holds more than {} files",
                MAX_TREE_FILES
            )));
        }

        let mut content = Vec::new();
        entry
            .by_ref()
            .take(MAX_FILE_BYTES as u64 + 1)
            .read_to_end(&mut content)
            .map_err(|e| SyncError::Archive(format!("Could not read '{}': {}", rest, e)))?;

        if content.len() > MAX_FILE_BYTES {
            return Err(SyncError::TooLarge(format!(
                "'{}' is larger than {} bytes",
                rest, MAX_FILE_BYTES
            )));
        }

        total = total.saturating_add(content.len());
        if total > MAX_TREE_BYTES {
            return Err(SyncError::TooLarge(format!(
                "Repository expands to more than {} bytes",
                MAX_TREE_BYTES
            )));
        }

        files.insert(rest.to_string(), content);
    }

    if files.is_empty() {
        return Err(SyncError::Layout(
            "The repository holds no files this engine can read".to_string(),
        ));
    }

    Ok(files)
}

/// The shape checks a repository path has to pass before it can name an asset.
///
/// Stricter than it needs to be for a well-behaved archive, and deliberately
/// so: these bytes came off the network, and the asset write's own
/// `validate_asset_uri` is the last line rather than the first.
fn validate_repo_path(path: &str) -> Result<(), SyncError> {
    if path.len() > 255 {
        return Err(SyncError::Layout(format!(
            "Path '{}' is longer than the 255 characters an asset name allows",
            path
        )));
    }
    if path.starts_with('/') || path.contains('\\') || path.contains("..") {
        return Err(SyncError::Layout(format!(
            "Path '{}' is not a usable asset name",
            path
        )));
    }
    if path.split('/').any(|segment| segment.is_empty()) {
        return Err(SyncError::Layout(format!(
            "Path '{}' has an empty segment",
            path
        )));
    }
    Ok(())
}

/// Whether a file is carried into a script's assets at all.
///
/// Three layers, and the order between them is the whole design:
///
/// 1. **The floor.** A dotfile is never taken, and nothing can put one back.
///    `.git` and `.github` are machinery, and `.env` is the one that matters:
///    assets are servable over HTTP, so a re-included credential file is a
///    credential leak. The mistake would not be deliberate — it would be one
///    broad `!` pattern written by somebody who did not think about what else
///    it caught.
/// 2. **The repository's own rules.** [`IGNORE_FILE`] both takes files out and,
///    with `!`, puts back ones the defaults below would have dropped — a
///    `README.md` a solution actually serves, say.
/// 3. **The defaults.** `node_modules`, because a script's imports resolve
///    through the engine's module loader and never through a package tree; and
///    repository furniture at a script's top level, where a `README.md` beside
///    the entry is documentation while one inside `docs/` may be content.
///
/// `repo_path` is relative to the repository root and `script_relative` to the
/// script's own root. They differ for a repository holding several scripts, and
/// both are needed: the rules are written against the repository, while "at the
/// top level" means the script's top level.
fn is_excluded(repo_path: &str, script_relative: &str, ignore: &IgnoreRules) -> bool {
    if repo_path.split('/').any(|segment| segment.starts_with('.')) {
        return true;
    }

    if let Some(ignored) = ignore.verdict(repo_path) {
        return ignored;
    }

    if repo_path
        .split('/')
        .any(|segment| segment == "node_modules")
    {
        return true;
    }

    let mut segments = script_relative.split('/');
    match (segments.next(), segments.next()) {
        (Some(only), None) => TOP_LEVEL_SKIP.contains(&only.to_ascii_lowercase().as_str()),
        _ => false,
    }
}

/// Find the entry point directly inside `dir` (or at the tree root when `dir`
/// is empty), refusing an ambiguous answer rather than picking one.
fn find_entry(files: &BTreeMap<String, Vec<u8>>, dir: &str) -> Result<Option<String>, SyncError> {
    // The full path, not the bare name: the caller uses it both to look the
    // entry's content up and to keep it out of the script's assets, and inside
    // a directory those two are only correct when the directory is included.
    let found: Vec<String> = ENTRY_NAMES
        .iter()
        .map(|name| {
            if dir.is_empty() {
                (*name).to_string()
            } else {
                format!("{}/{}", dir, name)
            }
        })
        .filter(|candidate| files.contains_key(candidate))
        .collect();

    match found.as_slice() {
        [] => Ok(None),
        [one] => Ok(Some(one.clone())),
        several => Err(SyncError::Layout(format!(
            "{} holds more than one entry point ({}). Keep one.",
            if dir.is_empty() {
                "The repository root".to_string()
            } else {
                format!("'{}'", dir)
            },
            several.join(", ")
        ))),
    }
}

/// Read the scripts a repository's tree describes.
///
/// Two shapes, checked in this order:
///
/// 1. An entry at the tree root means the repository *is* one script, and every
///    other file is one of its assets — including files inside directories.
/// 2. Otherwise each top-level directory holding its own entry is one script.
///
/// The first rule takes precedence deliberately. A repository with both a root
/// entry and entry-bearing subdirectories is one script that happens to have
/// subdirectories, which is the reading that keeps a single-script repository
/// from changing meaning the day somebody adds a folder.
pub fn infer_scripts(files: &BTreeMap<String, Vec<u8>>) -> Result<Vec<ScriptLayout>, SyncError> {
    // The rules are read from the tree rather than passed in, because they are
    // a property of the repository and travel with it — the same repository
    // pulled onto a different install has to take the same files.
    let ignore = files
        .get(IGNORE_FILE)
        .map(|content| IgnoreRules::parse(&String::from_utf8_lossy(content)))
        .unwrap_or_default();

    // A script whose entry the repository has excluded is not a script this
    // pull takes. Ignoring a directory should mean the whole of it, rather than
    // leaving behind an entry with none of the modules it imports.
    if let Some(entry_path) = find_entry(files, "")?.filter(|path| !ignore.hides(path)) {
        let mut assets = BTreeMap::new();
        for (path, content) in files {
            // The repository root is this script's root, so the two paths the
            // exclusion asks about are the same one.
            if path == &entry_path || is_excluded(path, path, &ignore) {
                continue;
            }
            assets.insert(path.clone(), content.clone());
        }
        let entry = files.get(&entry_path).cloned().unwrap_or_default();
        return Ok(vec![ScriptLayout {
            name: None,
            entry_path,
            entry,
            assets,
        }]);
    }

    let dirs: BTreeSet<&str> = files
        .keys()
        .filter_map(|path| path.split_once('/').map(|(dir, _)| dir))
        .filter(|dir| !dir.starts_with('.') && *dir != "node_modules")
        .collect();

    let mut layouts = Vec::new();
    for dir in dirs {
        let Some(entry_path) = find_entry(files, dir)?.filter(|path| !ignore.hides(path)) else {
            continue;
        };
        let prefix = format!("{}/", dir);
        let mut assets = BTreeMap::new();
        for (path, content) in files {
            let Some(relative) = path.strip_prefix(&prefix) else {
                continue;
            };
            if path == &entry_path || is_excluded(path, relative, &ignore) {
                continue;
            }
            assets.insert(relative.to_string(), content.clone());
        }
        let entry = files.get(&entry_path).cloned().unwrap_or_default();
        layouts.push(ScriptLayout {
            name: Some(dir.to_string()),
            entry_path,
            entry,
            assets,
        });
    }

    if layouts.is_empty() {
        return Err(SyncError::Layout(format!(
            "No script found. Expected one of {} at the repository root, or inside a \
             top-level directory.",
            ENTRY_NAMES.join(", ")
        )));
    }

    Ok(layouts)
}

// ============================================================================
// What a sync costs, and what it records
// ============================================================================

/// Spend one unit of this account's git budget.
///
/// A pull downloads an archive, expands it and rewrites a script's whole tree;
/// a push uploads blobs and moves a ref. Every other operation of that weight
/// in the engine is bounded and these were not. Keyed by account rather than
/// address because both are authenticated, and the budget should follow the
/// account across whatever network it is on.
async fn spend_budget(user: &UserContext) -> Result<(), SyncError> {
    let Some(user_id) = &user.user_id else {
        return Ok(());
    };
    let Some(limiter) = crate::security::rate_limiting::shared() else {
        // Startup has not built one — a unit test. Carrying on is right:
        // refusing work for want of a budget to check would fail closed on
        // something that is not a security decision.
        return Ok(());
    };

    let allowed = limiter
        .check_rate_limit(
            crate::security::rate_limiting::RateLimitKey::GitSync(user_id.clone()),
            1,
        )
        .await
        .allowed;

    if allowed {
        Ok(())
    } else {
        Err(SyncError::RateLimited(
            "Too many git operations. This account's budget refills over the next few minutes."
                .to_string(),
        ))
    }
}

/// Record that content crossed the boundary between this engine and a git host.
///
/// A push publishes a script's contents to a third party, and a pull replaces a
/// script's contents from one. Both are worth a line in the audit log for the
/// same reason the asset write already keeps one: somebody asking later what
/// happened to a script needs to see that the answer was "somewhere else".
fn audit_sync(
    user: &UserContext,
    action: &'static str,
    severity: crate::security::audit::SecuritySeverity,
    script: &str,
    remote: &str,
    detail: String,
) {
    use crate::security::audit::{SecurityEvent, SecurityEventType};

    let auditor = crate::security::audit::SecurityAuditor::new(
        crate::database::get_global_database().map(|db| db.pool().clone()),
    );
    let user_id = user.user_id.clone();
    let script = script.to_string();
    let remote = remote.to_string();

    tokio::task::spawn(async move {
        let _ = auditor
            .log_event(
                SecurityEvent::new(SecurityEventType::SystemSecurityEvent, severity, user_id)
                    .with_resource("git".to_string())
                    .with_action(action.to_string())
                    .with_detail("script_uri", &script)
                    .with_detail("remote", &remote)
                    .with_detail("outcome", &detail),
            )
            .await;
    });
}

// ============================================================================
// Pulling
// ============================================================================

/// What a caller asks for when pulling.
#[derive(Debug, Clone)]
pub struct PullRequest {
    /// `owner/repo`, or any GitHub URL naming it.
    pub repo: String,
    /// Branch to read. `None` asks GitHub for the repository's default.
    pub branch: Option<String>,
    /// URI prefix the pulled scripts land under.
    ///
    /// Explicit rather than derived from the local host, and defaulted from the
    /// repository name. On a shared server the prefix is what keeps two people
    /// pulling the same public repository from colliding on one URI — and it is
    /// a decision belonging to whoever is deploying the solution rather than to
    /// whoever named the repository.
    pub prefix: Option<String>,
    /// Download and re-apply even when nothing appears to have moved.
    ///
    /// The up-to-date check is an optimization over the questions this engine
    /// knows to ask. It cannot cover the ones it does not — a repository
    /// rewritten to the same commit, a script edited here since the pull, a
    /// mapping this version composes differently in a way the recorded base
    /// does not capture — so there has to be a way to say "do it anyway" that
    /// does not involve deleting rows to clear a cache.
    pub force: bool,
}

/// What one script's pull did.
#[derive(Debug, Clone)]
pub struct PulledScript {
    pub script_uri: String,
    /// The repository directory it came from, or `None` for a single-script
    /// repository.
    pub source: Option<String>,
    /// `created` or `updated`.
    pub action: &'static str,
    /// Whether this pull altered the script at all — its root or its files.
    /// False means the engine already held exactly what the repository has.
    pub changed: bool,
    pub written: usize,
    pub deleted: usize,
    pub unchanged: usize,
    pub revision: Option<i32>,
    pub init: serde_json::Value,
}

/// What a pull did, in full.
#[derive(Debug, Clone)]
pub struct PullReport {
    pub repo: String,
    pub branch: String,
    pub commit: String,
    pub scripts: Vec<PulledScript>,
    /// Whether every script this repository owns already stood at `commit`, in
    /// which case nothing was downloaded.
    pub up_to_date: bool,
}

/// The scripts written by a pull, before any of them has been initialised.
struct WrittenScripts {
    branch: String,
    commit: String,
    scripts: Vec<PulledScript>,
    up_to_date: bool,
}

/// Pull a repository into this engine.
///
/// Split across the runtime boundary on purpose. Everything up to and including
/// the writes is blocking — [`crate::http_client`] is built on blocking reqwest,
/// and the repository's write path is blocking too — so it runs on a blocking
/// thread. Re-initialising the scripts afterwards is async and runs here, once
/// per script rather than once per file.
pub async fn pull(user: &UserContext, request: PullRequest) -> Result<PullReport, SyncError> {
    let host = crate::git_github::HOST;

    // Asked before anything is fetched, so a deployment that has turned git
    // sync off answers with the reason rather than with a network error.
    if !crate::config::git_config().allows(host) {
        return Err(SyncError::AccessDenied(format!(
            "This engine is not configured to read from {}",
            host
        )));
    }

    // The caller's own token, never anybody else's: the account that reaches
    // GitHub is the account that asked, which is what keeps the attribution on
    // the far side matching the attribution here.
    let token = match &user.user_id {
        Some(user_id) => crate::git_credentials::token_for(user_id, host).await,
        None => None,
    };
    let authenticated = token.is_some();

    spend_budget(user).await?;

    let repo_label = request.repo.clone();
    let client = crate::git_github::GitHubClient::new()?.with_token(token);
    let report = pull_with(client, user, request).await;

    match &report {
        Ok(done) => audit_sync(
            user,
            "pull",
            crate::security::audit::SecuritySeverity::Medium,
            &done
                .scripts
                .iter()
                .map(|script| script.script_uri.as_str())
                .collect::<Vec<_>>()
                .join(","),
            &done.repo,
            format!("commit={} scripts={}", done.commit, done.scripts.len()),
        ),
        Err(e) => audit_sync(
            user,
            "pull_refused",
            crate::security::audit::SecuritySeverity::Low,
            "",
            &repo_label,
            e.to_string(),
        ),
    }

    // Recorded only when the pull got somewhere, so "last used" answers about
    // the credential working rather than about somebody having typed a
    // repository name.
    if authenticated
        && report.is_ok()
        && let Some(user_id) = &user.user_id
    {
        crate::git_credentials::mark_used(user_id, host).await;
    }

    report
}

/// [`pull`], against a GitHub client the caller supplies.
///
/// Exists so the suite can point a pull at a stand-in for GitHub. The whole of
/// this feature past the layout inference is a conversation with a third party,
/// and testing only the half that never leaves the process would leave the
/// interesting half unexercised.
pub async fn pull_with(
    client: crate::git_github::GitHubClient,
    user: &UserContext,
    request: PullRequest,
) -> Result<PullReport, SyncError> {
    let repo = crate::git_github::RepoRef::parse(&request.repo)?;
    let repo_label = repo.to_string();

    let user = user.clone();
    let written =
        tokio::task::spawn_blocking(move || fetch_and_write(&client, &user, &repo, &request))
            .await
            .map_err(|e| SyncError::Storage(format!("Pull did not finish: {}", e)))??;

    let mut scripts = written.scripts;
    if !written.up_to_date {
        for script in &mut scripts {
            // A script whose files did not change is already running exactly
            // this code, and re-initialising it would drop and rebuild
            // registrations that are correct.
            script.init = if script.changed {
                crate::engine_api::reinitialize_after_write(&script.script_uri).await
            } else {
                serde_json::json!({ "ran": false, "reason": "no files changed" })
            };
        }
    }

    Ok(PullReport {
        repo: repo_label,
        branch: written.branch,
        commit: written.commit,
        scripts,
        up_to_date: written.up_to_date,
    })
}

/// The blocking half of [`pull`]: resolve, download, map, and write.
fn fetch_and_write(
    client: &crate::git_github::GitHubClient,
    user: &UserContext,
    repo: &crate::git_github::RepoRef,
    request: &PullRequest,
) -> Result<WrittenScripts, SyncError> {
    let branch = match &request.branch {
        Some(branch) => branch.clone(),
        None => client.default_branch(repo)?,
    };
    let resolved = client.resolve_branch(repo, &branch)?;

    let prefix = request.prefix.clone().unwrap_or_else(|| repo.repo.clone());
    let prefix = prefix.trim_matches('/').to_string();
    if prefix.is_empty() || prefix.contains("..") || prefix.contains('\\') {
        return Err(SyncError::Layout(format!(
            "'{}' is not a usable URI prefix",
            prefix
        )));
    }
    let uri_base = resolve_uri_base(&prefix);

    // Ask before downloading: a repository that has not moved since the last
    // pull costs two small API calls rather than an archive.
    //
    // Two things have to hold, not one. That the remote has not moved is what
    // this check is usually about — but the base on record has to be the one
    // this pull would compose too, or the shortcut is answering about a mapping
    // that no longer applies. That is not hypothetical: it is what made a pull
    // silently do nothing after the URI composition changed underneath it. A
    // row predating the column carries `None`, which matches nothing, so the
    // first pull after that migration always does the work.
    if !request.force {
        let known = crate::database::run_blocking(last_synced(&repo.to_string(), &branch))
            .unwrap_or_default();
        let settled = !known.is_empty()
            && known.iter().all(|row| {
                row.commit == resolved.commit
                    && row.uri_base.as_deref() == Some(uri_base.as_str())
                    && row.mapping_version == Some(MAPPING_VERSION)
            });
        if settled {
            return Ok(WrittenScripts {
                branch,
                commit: resolved.commit,
                scripts: Vec::new(),
                up_to_date: true,
            });
        }
    }

    let archive = client.fetch_archive(repo, &resolved.commit)?;
    let files = extract_tree(&archive)?;
    let layouts = infer_scripts(&files)?;

    // Compose every URI and check every one of them before writing any, so a
    // repository whose fourth script belongs to somebody else is refused whole
    // rather than applied in part.
    let planned: Vec<(String, ScriptLayout)> = layouts
        .into_iter()
        .map(|layout| (compose_script_uri(&uri_base, &layout), layout))
        .collect();

    let refused: Vec<&str> = planned
        .iter()
        .map(|(uri, _)| uri.as_str())
        .filter(|uri| !crate::engine_api::can_write_script(user, uri))
        .collect();
    if !refused.is_empty() {
        return Err(SyncError::AccessDenied(format!(
            "Access denied. This pull would write {}, which you do not own. \
             Pull under a different prefix, or ask an administrator.",
            refused.join(", ")
        )));
    }

    // Owning a script is not the same as having agreed that a repository may
    // replace it. The prefix defaults to the repository's name, so a repository
    // named like an existing script's prefix is all it takes to overwrite work
    // that was written here and never pulled from anywhere.
    if !request.force {
        let occupied: Vec<String> = planned
            .iter()
            .filter(|(uri, _)| crate::repository::fetch_script(uri).is_some())
            .filter(|(uri, _)| {
                match crate::database::run_blocking(sync_row(uri)).unwrap_or_default() {
                    // Already this repository's script: replacing it is the
                    // whole point of a pull.
                    Some(row) => row.remote != repo.to_string(),
                    // Written here and bound to nothing.
                    None => true,
                }
            })
            .map(|(uri, _)| uri.clone())
            .collect();

        if !occupied.is_empty() {
            return Err(SyncError::WouldOverwrite(format!(
                "This pull would overwrite {}, which {} did not put there. Pull under a \
                 different prefix, or pass force to replace it.",
                occupied.join(", "),
                repo
            )));
        }
    }

    let mut scripts = Vec::with_capacity(planned.len());
    for (script_uri, layout) in planned {
        scripts.push(write_script(
            user,
            repo,
            &branch,
            &resolved.commit,
            &uri_base,
            &script_uri,
            &layout,
        )?);
    }

    Ok(WrittenScripts {
        branch,
        commit: resolved.commit,
        scripts,
        up_to_date: false,
    })
}

/// Write one inferred script: its root, then its assets, then whatever the
/// repository no longer holds.
/// The local URI a pulled script lands on.
///
/// Absolute, against this engine's own origin. A script URI in this engine is
/// an absolute URL — every script written through the editor or the MCP tools
/// has one — and things that consume a URI resolve it as such: the editor
/// builds its `read_script` link straight from it, so a relative URI silently
/// resolves against whatever origin the browser happens to be on and 404s.
///
/// That the URI is machine-specific is the point rather than a cost. It is
/// exactly why a repository cannot carry one: the engine composes its own at
/// pull time, and the same repository lands correctly on every install.
///
/// A `prefix` that is already absolute is left alone, which is how a caller
/// aims a pull at one host of a multi-host deployment.
fn resolve_uri_base(prefix: &str) -> String {
    if is_absolute(prefix) {
        prefix.to_string()
    } else if crate::hosts::is_configured() {
        format!(
            "{}/{}",
            crate::hosts::origin(&crate::hosts::default_host()),
            prefix
        )
    } else {
        // Nothing has told this process what it is serving — a unit test, or a
        // deployment with no usable base URL. A relative URI is the honest
        // answer there; inventing a host would be worse than not having one.
        prefix.to_string()
    }
}

fn is_absolute(prefix: &str) -> bool {
    prefix.starts_with("http://") || prefix.starts_with("https://")
}

/// Put the pieces together, given a resolved base.
///
/// A script is named after where it came from — the directory that held it, or
/// the repository when the repository is itself one script. Naming every script
/// `main.js` after its entry file is accurate and useless: a list of them in an
/// editor is a column of identical names, and the one thing a person needs to
/// read off a script URI is which script it is.
///
/// The extension is carried over from the entry and is the part that is *not*
/// free. [`crate::transpiler`] decides whether to transpile from the extension
/// alone — the stem is never read — so a `.ts` entry under a URI ending `.js`
/// would reach the runtime as TypeScript nobody transpiled.
fn compose_script_uri(base: &str, layout: &ScriptLayout) -> String {
    let base = base.trim_end_matches('/');
    let extension = layout.entry_extension();
    match &layout.name {
        Some(name) => format!("{}/{}{}", base, name, extension),
        // The base already ends with the repository's own name, so appending it
        // again would read `.../shop/shop.js` for a repository holding one
        // script.
        None => format!("{}{}", base, extension),
    }
}

#[allow(clippy::too_many_arguments)]
fn write_script(
    user: &UserContext,
    repo: &crate::git_github::RepoRef,
    branch: &str,
    commit: &str,
    uri_base: &str,
    script_uri: &str,
    layout: &ScriptLayout,
) -> Result<PulledScript, SyncError> {
    let entry = String::from_utf8(layout.entry.clone()).map_err(|_| {
        SyncError::Layout(format!(
            "'{}' is not text, and a script's entry point has to be",
            layout.entry_path
        ))
    })?;

    let writes: Vec<crate::engine_api::AssetWrite> = layout
        .assets
        .iter()
        .map(|(name, content)| crate::engine_api::AssetWrite {
            name: name.clone(),
            mimetype: None,
            content_base64: base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                content,
            ),
            expected_sha256: None,
        })
        .collect();

    // Anything the script holds that the repository does not is a file removed
    // upstream. Leaving it would let the script keep building against a module
    // its source of truth no longer has.
    let removed: Vec<String> = crate::repository::fetch_assets(script_uri)
        .into_keys()
        .filter(|existing| !layout.assets.contains_key(existing))
        .collect();

    // The root and the assets as one change, under one revision — the unit a
    // person reverts, and the unit the repository actually changed. A pull is
    // the same shape of write as the batch endpoint's, differing in its
    // ceilings and in what the revision says it came from.
    let outcome = crate::engine_api::write_script_files_authorized(
        user,
        script_uri,
        crate::engine_api::ScriptFilesChange {
            root: Some(&entry),
            writes: &writes,
            delete: &removed,
        },
        crate::engine_api::ScriptWriteOptions {
            origin: crate::revisions::Origin::GitPull,
            max_total_bytes: MAX_TREE_BYTES,
            max_files: MAX_TREE_FILES,
        },
    )
    .map_err(|e| match e {
        crate::engine_api::AssetWriteError::AccessDenied(message) => {
            SyncError::AccessDenied(format!("{} writing '{}'", message, script_uri))
        }
        crate::engine_api::AssetWriteError::Exists(file) => {
            SyncError::Storage(format!("'{}' already exists", file))
        }
        crate::engine_api::AssetWriteError::Validation(message)
        | crate::engine_api::AssetWriteError::Storage(message) => SyncError::Storage(message),
    })?;

    let unchanged = outcome
        .assets
        .results
        .iter()
        .filter(|result| result.status == "unchanged")
        .count();

    let changed = outcome.changed();
    let revision = outcome.revision;
    let action = outcome
        .action
        .unwrap_or(crate::engine_api::UpsertAction::Updated);

    crate::database::run_blocking(record_sync(
        script_uri,
        &repo.to_string(),
        branch,
        commit,
        uri_base,
        revision,
        user.user_id.as_deref(),
    ))
    .map_err(|e| SyncError::Storage(format!("Could not record the sync: {}", e)))?;

    Ok(PulledScript {
        script_uri: script_uri.to_string(),
        source: layout.name.clone(),
        action: match action {
            crate::engine_api::UpsertAction::Inserted => "created",
            crate::engine_api::UpsertAction::Updated => "updated",
        },
        changed,
        written: outcome.assets.written,
        deleted: outcome.assets.deleted,
        unchanged,
        revision,
        init: serde_json::json!({ "ran": false, "reason": "not yet run" }),
    })
}

// ============================================================================
// Pushing
// ============================================================================

/// What a caller asks for when pushing.
#[derive(Debug, Clone)]
pub struct PushRequest {
    /// The script to publish.
    pub script_uri: String,
    /// Where to publish it. Optional once the script has been pulled, since the
    /// sync row already names the repository it belongs to.
    pub repo: Option<String>,
    pub branch: Option<String>,
    /// Commit message. Defaulted to something that says what happened and
    /// where it came from.
    pub message: Option<String>,
    /// Push even when the engine believes the remote has moved.
    ///
    /// The check this skips is this engine's; GitHub's own refusal to
    /// fast-forward a ref stands regardless, which is what stops `force` from
    /// meaning "overwrite whatever is there".
    pub force: bool,
}

/// What a push did.
#[derive(Debug, Clone)]
pub struct PushReport {
    pub script_uri: String,
    pub repo: String,
    pub branch: String,
    /// The commit this push created, or the one already there when there was
    /// nothing to do.
    pub commit: String,
    /// Repository paths written and removed by this push.
    pub written: Vec<String>,
    pub removed: Vec<String>,
    /// True when the repository already held exactly these files.
    pub up_to_date: bool,
}

/// Publish a script's files to its repository.
pub async fn push(user: &UserContext, request: PushRequest) -> Result<PushReport, SyncError> {
    let host = crate::git_github::HOST;
    if !crate::config::git_config().allows(host) {
        return Err(SyncError::AccessDenied(format!(
            "This engine is not configured to write to {}",
            host
        )));
    }

    // Pushing publishes a script's content outward, so it takes what changing
    // the script takes. An editor who does not own it could already read it and
    // copy it out by hand — but making this ownership-gated keeps the sync row
    // coherent and leaves a record of content leaving the deployment.
    if !crate::engine_api::can_write_script(user, &request.script_uri) {
        return Err(SyncError::AccessDenied(format!(
            "Access denied. You must own '{}' or be an administrator to publish it.",
            request.script_uri
        )));
    }

    let token = match &user.user_id {
        Some(user_id) => crate::git_credentials::token_for(user_id, host).await,
        None => None,
    };
    if token.is_none() {
        return Err(SyncError::AccessDenied(
            "Pushing needs a credential. Store a token for github.com first — a public \
             repository can be read without one, but nothing can be written."
                .to_string(),
        ));
    }

    spend_budget(user).await?;

    let script_label = request.script_uri.clone();
    let client = crate::git_github::GitHubClient::new()?.with_token(token);
    let report = match push_with(client, user, request).await {
        Ok(report) => report,
        Err(e) => {
            // A refused push is worth recording too: a divergence somebody hit
            // is the beginning of a story, and the log is where it starts.
            audit_sync(
                user,
                "push_refused",
                crate::security::audit::SecuritySeverity::Low,
                &script_label,
                "",
                e.to_string(),
            );
            return Err(e);
        }
    };

    // High, unlike a pull: this is content leaving the deployment for a third
    // party, which is the direction that cannot be undone from here.
    audit_sync(
        user,
        "push",
        crate::security::audit::SecuritySeverity::High,
        &report.script_uri,
        &report.repo,
        format!(
            "commit={} written={} removed={}",
            report.commit,
            report.written.len(),
            report.removed.len()
        ),
    );

    if let Some(user_id) = &user.user_id {
        crate::git_credentials::mark_used(user_id, host).await;
    }

    Ok(report)
}

/// [`push`], against a GitHub client the caller supplies.
///
/// The credential lookup and the allowlist stay in [`push`]: what this takes is
/// an already-authenticated client, which is what lets the suite drive a whole
/// publish against a stand-in for GitHub.
pub async fn push_with(
    client: crate::git_github::GitHubClient,
    user: &UserContext,
    request: PushRequest,
) -> Result<PushReport, SyncError> {
    if !crate::engine_api::can_write_script(user, &request.script_uri) {
        return Err(SyncError::AccessDenied(format!(
            "Access denied. You must own '{}' or be an administrator to publish it.",
            request.script_uri
        )));
    }

    let known = crate::database::run_blocking(sync_row(&request.script_uri)).unwrap_or_default();
    let repo_spec = request
        .repo
        .clone()
        .or_else(|| known.as_ref().map(|row| row.remote.clone()))
        .ok_or_else(|| {
            SyncError::Layout(format!(
                "'{}' has not been pulled from anywhere, so there is nowhere to push it. \
                 Name a repository.",
                request.script_uri
            ))
        })?;
    let repo = crate::git_github::RepoRef::parse(&repo_spec)?;

    let acting = user.clone();
    tokio::task::spawn_blocking(move || {
        gather_and_push(&client, &acting, &repo, &request, known.as_ref())
    })
    .await
    .map_err(|e| SyncError::Storage(format!("Push did not finish: {}", e)))?
}

/// The blocking half of [`push`].
fn gather_and_push(
    client: &crate::git_github::GitHubClient,
    user: &UserContext,
    repo: &crate::git_github::RepoRef,
    request: &PushRequest,
    known: Option<&SyncRow>,
) -> Result<PushReport, SyncError> {
    let branch = match &request.branch {
        Some(branch) => branch.clone(),
        None => match known {
            Some(row) => row.branch.clone(),
            None => client.default_branch(repo)?,
        },
    };

    // A repository with no commits on this branch yet is a first publish rather
    // than an error: there is simply no parent and no tree to preserve.
    let head = match client.resolve_branch(repo, &branch) {
        Ok(resolved) => Some(client.tree_at(repo, &resolved.commit)?),
        Err(crate::git_github::GitHubError::NotFound(_)) => None,
        Err(e) => return Err(SyncError::Remote(e)),
    };

    if let Some(row) = known {
        check_divergence(row, head.as_ref(), &request.script_uri, request.force)?;
    }

    let files = engine_files(&request.script_uri, known)?;
    let ignore = head
        .as_ref()
        .and_then(|tree| ignore_rules_from(client, repo, tree))
        .unwrap_or_default();

    let entry_prefix = script_prefix(&request.script_uri, known);
    let existing = head
        .as_ref()
        .map(|tree| tree.entries.clone())
        .unwrap_or_default();

    // Everything the script does not own passes through untouched. That is what
    // keeps a push from deleting the repository's README, its CI configuration
    // and everything its ignore file kept out — files this engine never held
    // and which, from here, are indistinguishable from files deleted upstream.
    let mut next: Vec<crate::git_github::TreeEntry> = Vec::new();
    let mut removed = Vec::new();
    for entry in existing {
        if entry.kind == "tree" {
            // Directories are inferred from the paths of their contents.
            continue;
        }
        if owns_path(&entry.path, &entry_prefix, &ignore) && !files.contains_key(&entry.path) {
            removed.push(entry.path.clone());
            continue;
        }
        if files.contains_key(&entry.path) {
            continue;
        }
        next.push(entry);
    }

    let mut written = Vec::new();
    let mut unchanged = 0usize;
    let by_path: std::collections::HashMap<&str, &crate::git_github::TreeEntry> = head
        .as_ref()
        .map(|tree| {
            tree.entries
                .iter()
                .map(|entry| (entry.path.as_str(), entry))
                .collect()
        })
        .unwrap_or_default();

    for (path, content) in &files {
        // Both stores address content by digest, so a file whose blob is
        // already there needs no upload — only its sha repeated.
        let existing_sha = by_path.get(path.as_str()).map(|entry| entry.sha.as_str());
        let sha = match existing_sha {
            Some(sha) if git_blob_sha(content) == sha => {
                unchanged += 1;
                sha.to_string()
            }
            _ => {
                written.push(path.clone());
                client.create_blob(repo, content)?
            }
        };
        next.push(crate::git_github::TreeEntry {
            path: path.clone(),
            mode: "100644".to_string(),
            kind: "blob".to_string(),
            sha,
        });
    }

    if written.is_empty() && removed.is_empty() {
        return Ok(PushReport {
            script_uri: request.script_uri.clone(),
            repo: repo.to_string(),
            branch,
            commit: head.map(|tree| tree.commit).unwrap_or_default(),
            written,
            removed,
            up_to_date: true,
        });
    }
    let _ = unchanged;

    let tree = client.create_tree(repo, &next)?;
    let parents: Vec<String> = head.iter().map(|tree| tree.commit.clone()).collect();
    let message = request.message.clone().unwrap_or_else(|| {
        format!(
            "Update {} from aiwebengine",
            entry_prefix.as_deref().unwrap_or("the solution")
        )
    });
    let commit = client.create_commit(repo, &message, &tree, &parents)?;

    if parents.is_empty() {
        client.create_ref(repo, &branch, &commit)?;
    } else {
        client.update_ref(repo, &branch, &commit)?;
    }

    // The script now agrees with the commit that carries it.
    let revision = crate::revisions::current(&request.script_uri);
    let uri_base = known
        .and_then(|row| row.uri_base.clone())
        .unwrap_or_else(|| default_base_for(&request.script_uri));
    crate::database::run_blocking(record_sync(
        &request.script_uri,
        &repo.to_string(),
        &branch,
        &commit,
        &uri_base,
        revision,
        user.user_id.as_deref(),
    ))
    .map_err(|e| SyncError::Storage(format!("Could not record the sync: {}", e)))?;

    Ok(PushReport {
        script_uri: request.script_uri.clone(),
        repo: repo.to_string(),
        branch,
        commit,
        written,
        removed,
        up_to_date: false,
    })
}

/// Refuse a push whose two sides have both moved.
///
/// The engine implements no merge. When the repository has changed since this
/// script was last synced *and* the script has changed here, reconciling the
/// two is a judgement about code — which is what the agent asking for this push
/// is for, and what `/engine/revisions/diff` exists to feed it.
fn check_divergence(
    row: &SyncRow,
    head: Option<&crate::git_github::CommitTree>,
    script_uri: &str,
    force: bool,
) -> Result<(), SyncError> {
    if force {
        return Ok(());
    }
    let Some(head) = head else {
        return Ok(());
    };
    if head.commit == row.last_commit {
        return Ok(());
    }

    let local = crate::revisions::current(script_uri);
    if local == row.revision_at_sync {
        return Err(SyncError::Diverged(format!(
            "{} has moved since '{}' was last synced, and this engine has not. Pull first.",
            row.remote, script_uri
        )));
    }

    Err(SyncError::Diverged(format!(
        "Both sides have moved: {} is at {} rather than {}, and '{}' is at revision {} rather \
         than {}. Nothing was pushed. Reconcile the two — /engine/revisions/diff shows what \
         changed here — then push again, or pass force to publish this engine's copy over the \
         repository's.",
        row.remote,
        &head.commit[..7.min(head.commit.len())],
        &row.last_commit[..7.min(row.last_commit.len())],
        script_uri,
        local
            .map(|r| r.to_string())
            .unwrap_or_else(|| "none".into()),
        row.revision_at_sync
            .map(|r| r.to_string())
            .unwrap_or_else(|| "none".into()),
    )))
}

/// The repository paths a script's files occupy, with their content.
fn engine_files(
    script_uri: &str,
    known: Option<&SyncRow>,
) -> Result<BTreeMap<String, Vec<u8>>, SyncError> {
    let Some(root) = crate::repository::fetch_script(script_uri) else {
        return Err(SyncError::Layout(format!(
            "No script '{}' to push",
            script_uri
        )));
    };

    let prefix = script_prefix(script_uri, known);
    let extension = script_uri
        .rfind('.')
        .map(|dot| &script_uri[dot..])
        .filter(|ext| !ext.contains('/'))
        .unwrap_or(".js");

    let mut files = BTreeMap::new();
    let entry = match &prefix {
        Some(dir) => format!("{}/main{}", dir, extension),
        None => format!("main{}", extension),
    };
    files.insert(entry, root.into_bytes());

    for (name, asset) in crate::repository::fetch_assets(script_uri) {
        let path = match &prefix {
            Some(dir) => format!("{}/{}", dir, name),
            None => name,
        };
        files.insert(path, asset.content);
    }

    Ok(files)
}

/// The repository directory a script came from, or `None` when the repository
/// is itself that one script.
///
/// The mapping a pull applies is invertible, which is why nothing has to be
/// stored for this: a URI is the base plus either `/{directory}{ext}` or just
/// `{ext}`, so what follows the base says which shape it is.
fn script_prefix(script_uri: &str, known: Option<&SyncRow>) -> Option<String> {
    let base = known.and_then(|row| row.uri_base.clone())?;
    let rest = script_uri.strip_prefix(&base)?;
    let rest = rest.strip_prefix('/')?;
    let stem = rest.rfind('.').map(|dot| &rest[..dot]).unwrap_or(rest);
    (!stem.is_empty()).then(|| stem.to_string())
}

/// Whether `path` is one this script is responsible for.
///
/// Owned means "a pull would have taken it": inside the script's directory, and
/// not excluded. Everything else belongs to the repository and is preserved.
fn owns_path(path: &str, prefix: &Option<String>, ignore: &IgnoreRules) -> bool {
    let relative = match prefix {
        Some(dir) => match path.strip_prefix(&format!("{}/", dir)) {
            Some(relative) => relative,
            None => return false,
        },
        None => path,
    };
    !is_excluded(path, relative, ignore)
}

/// The repository's ignore rules, read from the commit being pushed onto.
fn ignore_rules_from(
    client: &crate::git_github::GitHubClient,
    repo: &crate::git_github::RepoRef,
    tree: &crate::git_github::CommitTree,
) -> Option<IgnoreRules> {
    let entry = tree
        .entries
        .iter()
        .find(|entry| entry.path == IGNORE_FILE)?;
    let content = client.blob(repo, &entry.sha).ok()?;
    Some(IgnoreRules::parse(&String::from_utf8_lossy(&content)))
}

/// A base for a script that has never been synced, so a first push can record
/// one.
fn default_base_for(script_uri: &str) -> String {
    match script_uri.rfind('/') {
        Some(slash) => script_uri[..slash].to_string(),
        None => script_uri.to_string(),
    }
}

/// Git's own object id for a blob: `sha1("blob {len}\0" + content)`.
///
/// Computed here so a file whose content the repository already holds is
/// recognised without uploading it. GitHub would deduplicate the blob anyway,
/// but the request is the cost worth avoiding, and this is also what makes a
/// push of an unchanged script report honestly that it did nothing.
fn git_blob_sha(content: &[u8]) -> String {
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(format!("blob {}\0", content.len()).as_bytes());
    hasher.update(content);
    hex::encode(hasher.finalize())
}

/// One script's sync row.
#[derive(Debug, Clone)]
pub struct SyncRow {
    pub remote: String,
    pub branch: String,
    pub last_commit: String,
    pub uri_base: Option<String>,
    pub revision_at_sync: Option<i32>,
}

/// One script's binding, for a listing.
#[derive(Debug, Clone)]
pub struct Binding {
    pub script_uri: String,
    pub remote: String,
    pub branch: String,
    pub last_commit: String,
    pub synced_at: chrono::DateTime<chrono::Utc>,
    pub synced_by: Option<String>,
}

/// Every binding this engine holds.
///
/// Filtered by the caller afterwards rather than here, because who may see a
/// script is a question about capabilities and ownership that this module has
/// no business answering.
pub async fn bindings() -> crate::error::AppResult<Vec<Binding>> {
    use sqlx::Row;
    let rows = sqlx::query(
        "SELECT script_uri, remote, branch, last_commit, synced_at, synced_by \
         FROM script_git_sync ORDER BY remote, script_uri",
    )
    .fetch_all(&pool()?)
    .await
    .map_err(|e| crate::error::AppError::Database {
        message: format!("Database error listing git bindings: {}", e),
        source: None,
    })?;

    Ok(rows
        .into_iter()
        .map(|row| Binding {
            script_uri: row.get::<String, _>(0),
            remote: row.get::<String, _>(1),
            branch: row.get::<String, _>(2),
            last_commit: row.get::<String, _>(3),
            synced_at: row.get::<chrono::DateTime<chrono::Utc>, _>(4),
            synced_by: row.get::<Option<String>, _>(5),
        })
        .collect())
}

/// Stop a script tracking a repository. Returns whether it was tracking one.
///
/// The script and its files stay exactly as they are; what goes is the record
/// of where they came from. There is deliberately no counterpart that sets a
/// binding without writing anything: a push names its repository and records
/// the binding when it lands, and a binding that has never been either pushed
/// or pulled describes an agreement neither side has made.
pub async fn unbind(script_uri: &str) -> crate::error::AppResult<bool> {
    let result = sqlx::query("DELETE FROM script_git_sync WHERE script_uri = $1")
        .bind(script_uri)
        .execute(&pool()?)
        .await
        .map_err(|e| crate::error::AppError::Database {
            message: format!("Database error clearing a git binding: {}", e),
            source: None,
        })?;
    Ok(result.rows_affected() > 0)
}

pub async fn sync_row(script_uri: &str) -> crate::error::AppResult<Option<SyncRow>> {
    use sqlx::Row;
    let row = sqlx::query(
        "SELECT remote, branch, last_commit, uri_base, revision_at_sync \
         FROM script_git_sync WHERE script_uri = $1",
    )
    .bind(script_uri)
    .fetch_optional(&pool()?)
    .await
    .map_err(|e| crate::error::AppError::Database {
        message: format!("Database error reading git sync state: {}", e),
        source: None,
    })?;

    Ok(row.map(|row| SyncRow {
        remote: row.get::<String, _>(0),
        branch: row.get::<String, _>(1),
        last_commit: row.get::<String, _>(2),
        uri_base: row.get::<Option<String>, _>(3),
        revision_at_sync: row.get::<Option<i32>, _>(4),
    }))
}

// ============================================================================
// Where a script stands
// ============================================================================

/// Which cell of the sync matrix a script sits in.
///
/// The four states are the whole decision, and naming them is the point of this
/// type: an agent holding one of these knows whether to push, to pull, or to
/// stop and reconcile, without having to attempt an operation and read the
/// refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncState {
    /// Never pulled from or pushed to anywhere.
    Unbound,
    /// Neither side has moved since they last agreed.
    InSync,
    /// The repository moved and this engine did not — a pull.
    Behind,
    /// This engine moved and the repository did not — a push.
    Ahead,
    /// Both moved. Nothing to do automatically; somebody has to reconcile them.
    Diverged,
    /// The repository could not be reached, so only the local half is known.
    Unreachable,
}

impl SyncState {
    pub fn as_str(self) -> &'static str {
        match self {
            SyncState::Unbound => "unbound",
            SyncState::InSync => "in_sync",
            SyncState::Behind => "behind",
            SyncState::Ahead => "ahead",
            SyncState::Diverged => "diverged",
            SyncState::Unreachable => "unreachable",
        }
    }

    /// What to do about it, in the words the caller would use.
    pub fn advice(self) -> &'static str {
        match self {
            SyncState::Unbound => {
                "This script came from nowhere. Push it, naming a repository, to publish it."
            }
            SyncState::InSync => "Nothing to do.",
            SyncState::Behind => "Pull. The repository has changes this engine does not.",
            SyncState::Ahead => "Push. This engine has changes the repository does not.",
            SyncState::Diverged => {
                "Both sides have changed. Compare them — /engine/revisions/diff shows what \
                 changed here — and reconcile before pushing, or pull to take the \
                 repository's copy."
            }
            SyncState::Unreachable => {
                "The repository could not be reached, so only this engine's half is known."
            }
        }
    }
}

/// What a script's relationship with its repository looks like right now.
#[derive(Debug, Clone)]
pub struct SyncStatus {
    pub script_uri: String,
    pub state: SyncState,
    /// Where it came from, when it came from anywhere.
    pub remote: Option<String>,
    pub branch: Option<String>,
    /// The commit the two sides last agreed on.
    pub commit_at_sync: Option<String>,
    /// What the branch points at now, when the host could be asked.
    pub remote_commit: Option<String>,
    /// The revision they last agreed on, and the newest one here.
    pub revision_at_sync: Option<i32>,
    pub revision: Option<i32>,
    /// The revision this script is pinned to, if any.
    ///
    /// Reported beside the sync state because a pull into a pinned script
    /// advances head without changing what answers requests — correct, and
    /// surprising if nothing says so at the point somebody is deciding what to
    /// do next.
    pub pinned: Option<i32>,
    /// Why the remote half is missing, when it is.
    pub unreachable: Option<String>,
}

/// Report where `script_uri` stands relative to its repository.
pub async fn status(user: &UserContext, script_uri: &str) -> Result<SyncStatus, SyncError> {
    let host = crate::git_github::HOST;
    let token = match &user.user_id {
        Some(user_id) => crate::git_credentials::token_for(user_id, host).await,
        None => None,
    };
    let client = crate::git_github::GitHubClient::new()?.with_token(token);
    status_with(client, script_uri).await
}

/// [`status`], against a GitHub client the caller supplies.
pub async fn status_with(
    client: crate::git_github::GitHubClient,
    script_uri: &str,
) -> Result<SyncStatus, SyncError> {
    if crate::repository::fetch_script(script_uri).is_none() {
        return Err(SyncError::Layout(format!("No script '{}'", script_uri)));
    }

    let revision = crate::revisions::current(script_uri);
    let pinned = crate::deployments::pinned(script_uri);
    let known = crate::database::run_blocking(sync_row(script_uri)).unwrap_or_default();

    let Some(row) = known else {
        return Ok(SyncStatus {
            script_uri: script_uri.to_string(),
            state: SyncState::Unbound,
            remote: None,
            branch: None,
            commit_at_sync: None,
            remote_commit: None,
            revision_at_sync: None,
            revision,
            pinned,
            unreachable: None,
        });
    };

    let repo = crate::git_github::RepoRef::parse(&row.remote)?;
    let branch = row.branch.clone();

    // Asking the host is what makes this worth calling, and it is also the one
    // part that can fail for reasons that have nothing to do with the script.
    // A host that cannot be reached degrades to the local half rather than
    // failing the whole answer: "your copy has changed and I could not check
    // theirs" is useful, and an error is not.
    let probe = tokio::task::spawn_blocking(move || client.resolve_branch(&repo, &branch))
        .await
        .map_err(|e| SyncError::Storage(format!("Status did not finish: {}", e)))?;

    let (remote_commit, unreachable) = match probe {
        Ok(resolved) => (Some(resolved.commit), None),
        Err(e) => (None, Some(e.to_string())),
    };

    let local_moved = revision != row.revision_at_sync;
    let state = match &remote_commit {
        None => SyncState::Unreachable,
        Some(commit) => match (commit != &row.last_commit, local_moved) {
            (false, false) => SyncState::InSync,
            (true, false) => SyncState::Behind,
            (false, true) => SyncState::Ahead,
            (true, true) => SyncState::Diverged,
        },
    };

    Ok(SyncStatus {
        script_uri: script_uri.to_string(),
        state,
        remote: Some(row.remote),
        branch: Some(row.branch),
        commit_at_sync: Some(row.last_commit),
        remote_commit,
        revision_at_sync: row.revision_at_sync,
        revision,
        pinned,
        unreachable,
    })
}

// ============================================================================
// Where a script was last synced from
// ============================================================================

fn pool() -> crate::error::AppResult<sqlx::PgPool> {
    crate::repository::get_db_pool()
        .map(|db| db.pool().clone())
        .ok_or_else(|| crate::error::AppError::Database {
            message: "No database configured".to_string(),
            source: None,
        })
}

/// One script's standing relative to the repository it came from.
#[derive(Debug, Clone)]
pub struct SyncedScript {
    pub script_uri: String,
    pub commit: String,
    /// The base the pull composed this script's URI against, or `None` for a
    /// row written before that was recorded.
    pub uri_base: Option<String>,
    /// Which composition rules wrote it, or `None` for a row predating them.
    pub mapping_version: Option<i32>,
}

/// The scripts this remote and branch have written here.
///
/// Read for one question today — is there anything to do — but it is the same
/// row a push and a divergence check will read, which is why it carries the
/// revision alongside the commit rather than the commit alone.
pub async fn last_synced(remote: &str, branch: &str) -> crate::error::AppResult<Vec<SyncedScript>> {
    use sqlx::Row;
    let rows = sqlx::query(
        "SELECT script_uri, last_commit, uri_base, mapping_version FROM script_git_sync \
         WHERE remote = $1 AND branch = $2",
    )
    .bind(remote)
    .bind(branch)
    .fetch_all(&pool()?)
    .await
    .map_err(|e| crate::error::AppError::Database {
        message: format!("Database error reading git sync state: {}", e),
        source: None,
    })?;

    Ok(rows
        .into_iter()
        .map(|row| SyncedScript {
            script_uri: row.get::<String, _>(0),
            commit: row.get::<String, _>(1),
            uri_base: row.get::<Option<String>, _>(2),
            mapping_version: row.get::<Option<i32>, _>(3),
        })
        .collect())
}

/// Record where a script now stands relative to its repository.
#[allow(clippy::too_many_arguments)]
pub async fn record_sync(
    script_uri: &str,
    remote: &str,
    branch: &str,
    commit: &str,
    uri_base: &str,
    revision: Option<i32>,
    user_id: Option<&str>,
) -> crate::error::AppResult<()> {
    sqlx::query(
        r#"
        INSERT INTO script_git_sync
            (script_uri, remote, branch, last_commit, uri_base, mapping_version,
             revision_at_sync, synced_at, synced_by)
        VALUES ($1, $2, $3, $4, $5, $6, $7, NOW(), $8)
        ON CONFLICT (script_uri) DO UPDATE SET
            remote = EXCLUDED.remote,
            branch = EXCLUDED.branch,
            last_commit = EXCLUDED.last_commit,
            uri_base = EXCLUDED.uri_base,
            mapping_version = EXCLUDED.mapping_version,
            revision_at_sync = COALESCE(EXCLUDED.revision_at_sync, script_git_sync.revision_at_sync),
            synced_at = EXCLUDED.synced_at,
            synced_by = EXCLUDED.synced_by
        "#,
    )
    .bind(script_uri)
    .bind(remote)
    .bind(branch)
    .bind(commit)
    .bind(uri_base)
    .bind(MAPPING_VERSION)
    .bind(revision)
    .bind(user_id)
    .execute(&pool()?)
    .await
    .map_err(|e| crate::error::AppError::Database {
        message: format!("Database error recording git sync state: {}", e),
        source: None,
    })?;
    Ok(())
}

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("{0}")]
    Archive(String),

    #[error("{0}")]
    Layout(String),

    #[error("{0}")]
    TooLarge(String),

    #[error("{0}")]
    Remote(#[from] crate::git_github::GitHubError),

    #[error("{0}")]
    AccessDenied(String),

    #[error("{0}")]
    Storage(String),

    /// Both sides moved. The engine does not merge; it says so and stops.
    #[error("{0}")]
    Diverged(String),

    /// The target already holds a script this repository did not write.
    #[error("{0}")]
    WouldOverwrite(String),

    /// This account has spent its git budget.
    #[error("{0}")]
    RateLimited(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(entries: &[(&str, &str)]) -> BTreeMap<String, Vec<u8>> {
        entries
            .iter()
            .map(|(path, content)| (path.to_string(), content.as_bytes().to_vec()))
            .collect()
    }

    fn gzipped_tar(entries: &[(&str, &str)]) -> Vec<u8> {
        use std::io::Write;
        let mut builder = tar::Builder::new(Vec::new());
        for (path, content) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_entry_type(tar::EntryType::Regular);
            header.set_cksum();
            builder
                .append_data(&mut header, path, content.as_bytes())
                .expect("append");
        }
        let tar_bytes = builder.into_inner().expect("finish tar");
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        encoder.write_all(&tar_bytes).expect("gzip");
        encoder.finish().expect("finish gzip")
    }

    // ---------------------------------------------------------------- ignore

    fn ignored(rules: &str, path: &str) -> bool {
        IgnoreRules::parse(rules).hides(path)
    }

    #[test]
    fn a_bare_name_matches_at_any_depth() {
        assert!(ignored("tsconfig.json", "tsconfig.json"));
        assert!(ignored("tsconfig.json", "shop/tsconfig.json"));
        assert!(ignored("tsconfig.json", "shop/deep/tsconfig.json"));
        assert!(!ignored("tsconfig.json", "shop/tsconfig.json.ts"));
    }

    #[test]
    fn a_pattern_with_a_slash_is_anchored_at_the_repository_root() {
        assert!(ignored("shop/docs", "shop/docs/guide.md"));
        assert!(
            !ignored("shop/docs", "admin/shop/docs/guide.md"),
            "an anchored pattern does not float"
        );
    }

    #[test]
    fn a_directory_pattern_takes_its_contents_with_it() {
        assert!(ignored("docs/", "docs/guide.md"));
        assert!(ignored("docs/", "shop/docs/img/logo.png"));
        assert!(
            !ignored("docs/", "docs"),
            "a trailing slash names a directory, so a file called 'docs' survives"
        );
        // `dir/**` is the same statement written the other way.
        assert!(ignored("docs/**", "docs/guide.md"));
    }

    #[test]
    fn wildcards_stay_inside_one_segment() {
        assert!(ignored("*.md", "readme.md"));
        assert!(ignored("*.md", "docs/guide.md"));
        assert!(!ignored("*.md", "docs/guide.mdx"));
        assert!(
            !ignored("shop/*.ts", "shop/lib/cart.ts"),
            "a single star does not cross a slash"
        );
        assert!(ignored("shop/**/*.ts", "shop/lib/deep/cart.ts"));
    }

    #[test]
    fn the_last_matching_pattern_wins() {
        let rules = "*.md\n!shop/templates/help.md";
        assert!(ignored(rules, "README.md"));
        assert!(ignored(rules, "docs/guide.md"));
        assert!(!ignored(rules, "shop/templates/help.md"));

        // Order matters, and the other order says the other thing.
        let reversed = "!shop/templates/help.md\n*.md";
        assert!(ignored(reversed, "shop/templates/help.md"));
    }

    #[test]
    fn comments_and_blank_lines_are_not_patterns() {
        let rules = "# a comment\n\n   \ndocs/\n";
        assert!(ignored(rules, "docs/guide.md"));
        assert!(!ignored(rules, "# a comment"));
    }

    #[test]
    fn negation_can_restore_repository_furniture() {
        let files = tree(&[
            ("main.ts", "x"),
            ("README.md", "served on purpose"),
            (IGNORE_FILE, "!README.md"),
        ]);
        let scripts = infer_scripts(&files).expect("should infer");
        assert_eq!(
            scripts[0].assets.keys().collect::<Vec<_>>(),
            vec!["README.md"],
            "a solution that serves its README may say so"
        );
    }

    /// The floor. `!` is allowed to put back documentation; it is not allowed to
    /// put back a credential, because assets are servable and the mistake would
    /// be one broad pattern rather than a deliberate act.
    #[test]
    fn negation_cannot_restore_a_dotfile() {
        let files = tree(&[
            ("main.ts", "x"),
            (".env", "TOKEN=hunter2"),
            (".github/workflows/ci.yml", "on: push"),
            (IGNORE_FILE, "!.env\n!**\n!.github/**"),
        ]);
        let scripts = infer_scripts(&files).expect("should infer");
        assert!(
            scripts[0].assets.is_empty(),
            "no negation reaches a dotfile, got {:?}",
            scripts[0].assets.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_ignore_file_never_becomes_an_asset() {
        let files = tree(&[("main.ts", "x"), (IGNORE_FILE, "docs/")]);
        let scripts = infer_scripts(&files).expect("should infer");
        assert!(scripts[0].assets.is_empty());
    }

    #[test]
    fn rules_are_written_against_the_repository_not_the_script() {
        let files = tree(&[
            ("shop/main.ts", "shop"),
            ("shop/lib/cart.ts", "cart"),
            ("shop/docs/guide.md", "docs"),
            ("admin/main.ts", "admin"),
            ("admin/docs/guide.md", "docs"),
            (IGNORE_FILE, "shop/docs/\n"),
        ]);
        let scripts = infer_scripts(&files).expect("should infer");

        let shop = scripts
            .iter()
            .find(|s| s.name.as_deref() == Some("shop"))
            .expect("shop");
        assert_eq!(
            shop.assets.keys().collect::<Vec<_>>(),
            vec!["lib/cart.ts"],
            "the rule named shop/docs and took it"
        );

        let admin = scripts
            .iter()
            .find(|s| s.name.as_deref() == Some("admin"))
            .expect("admin");
        assert_eq!(
            admin.assets.keys().collect::<Vec<_>>(),
            vec!["docs/guide.md"],
            "and left the other script's docs alone, though its relative path is the same"
        );
    }

    #[test]
    fn ignoring_a_directory_drops_the_script_inside_it() {
        let files = tree(&[
            ("shop/main.ts", "shop"),
            ("experiment/main.ts", "not ready"),
            ("experiment/lib/thing.ts", "not ready"),
            (IGNORE_FILE, "experiment/\n"),
        ]);
        let scripts = infer_scripts(&files).expect("should infer");
        assert_eq!(scripts.len(), 1, "the ignored directory is not a script");
        assert_eq!(scripts[0].name.as_deref(), Some("shop"));
    }

    // ------------------------------------------------------------------ uri

    fn layout(name: Option<&str>, entry: &str) -> ScriptLayout {
        ScriptLayout {
            name: name.map(str::to_string),
            entry_path: entry.to_string(),
            entry: Vec::new(),
            assets: BTreeMap::new(),
        }
    }

    #[test]
    fn a_uri_is_named_after_its_source_and_keeps_the_entry_extension() {
        // A repository that is one script takes the repository's name, which
        // the base already ends with — appending it again would read
        // `.../examples/examples.ts`.
        let single = layout(None, "main.ts");
        assert_eq!(
            compose_script_uri("https://engine.example/examples", &single),
            "https://engine.example/examples.ts"
        );

        // One inside a directory takes the directory's name.
        let named = layout(Some("shop"), "shop/main.js");
        assert_eq!(
            compose_script_uri("https://engine.example/examples", &named),
            "https://engine.example/examples/shop.js"
        );
    }

    /// The extension is the part of the entry's name a URI has to keep:
    /// `transpiler::needs_transpilation` reads it and never reads the stem, so
    /// a `.ts` entry under a `.js` URI would reach the runtime untranspiled.
    #[test]
    fn the_extension_follows_the_entry() {
        assert_eq!(layout(Some("a"), "a/main.ts").entry_extension(), ".ts");
        assert_eq!(layout(Some("a"), "a/main.tsx").entry_extension(), ".tsx");
        assert_eq!(layout(None, "main.js").entry_extension(), ".js");
        assert_eq!(layout(None, "Makefile").entry_extension(), "");
    }

    #[test]
    fn a_trailing_slash_on_the_base_does_not_double_up() {
        assert_eq!(
            compose_script_uri(
                "https://engine.example/examples/",
                &layout(Some("shop"), "shop/main.ts")
            ),
            "https://engine.example/examples/shop.ts"
        );
        assert_eq!(
            compose_script_uri("https://engine.example/examples/", &layout(None, "main.ts")),
            "https://engine.example/examples.ts"
        );
    }

    /// A prefix that already names an origin is left alone, which is how a
    /// caller aims a pull at one host of a multi-host deployment.
    #[test]
    fn an_absolute_prefix_is_taken_as_given() {
        assert!(is_absolute("https://shop.example.com/solutions"));
        assert!(is_absolute("http://localhost:3000/x"));
        assert!(!is_absolute("examples"));
        assert!(!is_absolute("/examples"));
    }

    #[test]
    fn root_entry_makes_the_repository_one_script() {
        let files = tree(&[
            ("main.ts", "export function init() {}"),
            ("server/handler.ts", "export const x = 1;"),
            ("templates/page.html", "<p>hi</p>"),
        ]);
        let scripts = infer_scripts(&files).expect("should infer");
        assert_eq!(scripts.len(), 1);
        assert_eq!(scripts[0].name, None);
        assert_eq!(scripts[0].entry_path, "main.ts");
        assert_eq!(scripts[0].entry_file_name(), "main.ts");
        assert_eq!(
            scripts[0].assets.keys().collect::<Vec<_>>(),
            vec!["server/handler.ts", "templates/page.html"]
        );
    }

    #[test]
    fn top_level_directories_become_separate_scripts() {
        let files = tree(&[
            ("shop/main.ts", "shop"),
            ("shop/server/api.ts", "api"),
            ("admin/main.ts", "admin"),
        ]);
        let scripts = infer_scripts(&files).expect("should infer");
        assert_eq!(scripts.len(), 2);
        let names: Vec<_> = scripts.iter().map(|s| s.name.clone()).collect();
        assert_eq!(
            names,
            vec![Some("admin".to_string()), Some("shop".to_string())]
        );
        let shop = scripts
            .iter()
            .find(|s| s.name.as_deref() == Some("shop"))
            .expect("shop");
        // Asset names are relative to the script's own root, which is what
        // makes the repository subtree and the stored tree identical.
        assert_eq!(
            shop.assets.keys().collect::<Vec<_>>(),
            vec!["server/api.ts"]
        );
    }

    #[test]
    fn a_root_entry_wins_over_subdirectory_entries() {
        let files = tree(&[("main.ts", "root"), ("shop/main.ts", "shop")]);
        let scripts = infer_scripts(&files).expect("should infer");
        assert_eq!(scripts.len(), 1);
        assert_eq!(scripts[0].name, None);
        assert!(scripts[0].assets.contains_key("shop/main.ts"));
    }

    #[test]
    fn dotfiles_and_node_modules_are_dropped() {
        let files = tree(&[
            ("main.ts", "x"),
            (".env", "SECRET=1"),
            (".github/workflows/ci.yml", "on: push"),
            ("node_modules/left-pad/index.js", "module.exports = 1"),
            ("src/.hidden/thing.ts", "x"),
            ("keep.ts", "x"),
        ]);
        let scripts = infer_scripts(&files).expect("should infer");
        assert_eq!(
            scripts[0].assets.keys().collect::<Vec<_>>(),
            vec!["keep.ts"]
        );
    }

    #[test]
    fn repository_furniture_is_dropped_only_at_the_top_level() {
        let files = tree(&[
            ("main.ts", "x"),
            ("README.md", "repo readme"),
            ("LICENSE", "MIT"),
            ("docs/README.md", "served content"),
        ]);
        let scripts = infer_scripts(&files).expect("should infer");
        assert_eq!(
            scripts[0].assets.keys().collect::<Vec<_>>(),
            vec!["docs/README.md"]
        );
    }

    #[test]
    fn two_entry_points_are_refused_rather_than_guessed() {
        let files = tree(&[("main.ts", "a"), ("main.js", "b")]);
        let error = infer_scripts(&files).expect_err("should refuse");
        assert!(
            error.to_string().contains("more than one entry point"),
            "unexpected: {}",
            error
        );
    }

    #[test]
    fn a_repository_with_no_entry_says_what_it_looked_for() {
        let files = tree(&[("lib/util.ts", "x"), ("README.md", "y")]);
        let error = infer_scripts(&files).expect_err("should refuse");
        assert!(
            error.to_string().contains("main.ts"),
            "unexpected: {}",
            error
        );
    }

    #[test]
    fn extracts_a_github_style_archive_and_strips_the_wrapper() {
        let archive = gzipped_tar(&[
            ("owner-repo-abc1234/main.ts", "entry"),
            ("owner-repo-abc1234/server/api.ts", "api"),
        ]);
        let files = extract_tree(&archive).expect("should extract");
        assert_eq!(
            files.keys().collect::<Vec<_>>(),
            vec!["main.ts", "server/api.ts"]
        );
        assert_eq!(files.get("main.ts").map(Vec::as_slice), Some(&b"entry"[..]));
    }

    #[test]
    fn refuses_an_archive_with_two_root_directories() {
        let archive = gzipped_tar(&[("one/main.ts", "a"), ("two/main.ts", "b")]);
        let error = extract_tree(&archive).expect_err("should refuse");
        assert!(
            error.to_string().contains("more than one root directory"),
            "unexpected: {}",
            error
        );
    }

    /// Checked against the validator rather than through a built archive:
    /// `tar::Builder` refuses to write a `..` path at all, so an archive
    /// carrying one cannot be produced by the crate that reads it. An attacker
    /// writing tar headers by hand is under no such restriction, which is
    /// exactly why this check exists on the reading side.
    #[test]
    fn refuses_traversal_and_other_unusable_paths() {
        for path in [
            "../../etc/passwd",
            "server/../../escape.ts",
            "/absolute.ts",
            "windows\\style.ts",
            "server//empty.ts",
            "",
        ] {
            assert!(
                validate_repo_path(path).is_err(),
                "'{}' should be refused",
                path
            );
        }
        assert!(validate_repo_path("server/routes/index.ts").is_ok());
    }

    #[test]
    fn refuses_a_path_longer_than_an_asset_name_allows() {
        let long = format!("server/{}.ts", "a".repeat(300));
        assert!(validate_repo_path(&long).is_err());
    }

    #[test]
    fn round_trips_an_archive_into_scripts() {
        let archive = gzipped_tar(&[
            ("lpajunen-solution-9f8e7d6/shop/main.ts", "shop entry"),
            ("lpajunen-solution-9f8e7d6/shop/lib/cart.ts", "cart"),
            ("lpajunen-solution-9f8e7d6/README.md", "docs"),
            ("lpajunen-solution-9f8e7d6/.gitignore", "target"),
        ]);
        let files = extract_tree(&archive).expect("should extract");
        let scripts = infer_scripts(&files).expect("should infer");
        assert_eq!(scripts.len(), 1);
        assert_eq!(scripts[0].name.as_deref(), Some("shop"));
        assert_eq!(scripts[0].entry, b"shop entry".to_vec());
        assert_eq!(
            scripts[0].assets.keys().collect::<Vec<_>>(),
            vec!["lib/cart.ts"]
        );
    }
}
