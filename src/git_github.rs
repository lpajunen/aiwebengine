//! Reading a GitHub repository over HTTPS, with no working copy.
//!
//! The engine stores a script's files in Postgres and has nowhere to put a
//! checkout — a desktop install is one process and a data directory, and a
//! cluster instance is one of several that would each need their own. That
//! rules out `libgit2` and `gix`, which both want an on-disk object store, and
//! it points at the thing the engine actually wants: a *tree at a ref*, never
//! history. GitHub serves exactly that as one gzipped tar.
//!
//! Deliberately GitHub-only, and deliberately not a trait with one
//! implementation behind it. GitLab and Gitea expose equivalent APIs in
//! different shapes, and choosing the seam before a second host exists produces
//! an abstraction fitted to a single example. When a second host is actually
//! wanted, the seam will be obvious; until then this is a concrete client.
//!
//! Every request goes through [`crate::http_client::HttpClient`] rather than a
//! `reqwest` of its own. That client validates the URL, re-resolves DNS, and
//! re-validates each redirect hop — and a repository URL is caller-supplied, so
//! a client without those checks is a request forger pointed at whatever
//! network the engine is running in.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::http_client::{HttpClient, HttpError};

/// Where the JSON API lives. The archive redirect leaves this host for
/// `codeload.github.com`, which the HTTP client re-validates like any other
/// hop.
const API_BASE: &str = "https://api.github.com";

/// GitHub rejects requests that do not identify themselves.
const USER_AGENT: &str = "aiwebengine";

/// Pinned so a future default does not change what this code receives.
const API_VERSION: &str = "2022-11-28";

/// The host this client speaks to, and the key a credential is stored under.
pub const HOST: &str = "github.com";

/// Ceiling on a repository archive, compressed.
///
/// Generous next to what the tree is allowed to expand to, because this bounds
/// a download and the expansion is bounded separately where it can be measured
/// honestly — a small archive can still be a decompression bomb.
pub const MAX_ARCHIVE_BYTES: usize = 24 * 1024 * 1024;

/// One repository on GitHub.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoRef {
    pub owner: String,
    pub repo: String,
}

impl std::fmt::Display for RepoRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.owner, self.repo)
    }
}

impl RepoRef {
    /// Parse the ways a person naturally names a repository.
    ///
    /// All four forms below reach the same place, and a caller who copied a URL
    /// out of a browser should not have to know that `owner/repo` was wanted:
    ///
    /// - `owner/repo`
    /// - `https://github.com/owner/repo`
    /// - `https://github.com/owner/repo.git`
    /// - `git@github.com:owner/repo.git`
    ///
    /// A host that is not GitHub is refused here rather than later, so the
    /// message names the real reason instead of surfacing as a 404 from an API
    /// that was never going to answer.
    pub fn parse(spec: &str) -> Result<Self, GitHubError> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err(GitHubError::InvalidRepo(
                "Repository is empty; expected 'owner/repo'".to_string(),
            ));
        }

        let path = if let Some(rest) = spec.strip_prefix("git@github.com:") {
            rest
        } else if let Some(rest) = spec
            .strip_prefix("https://github.com/")
            .or_else(|| spec.strip_prefix("http://github.com/"))
            .or_else(|| spec.strip_prefix("https://www.github.com/"))
        {
            rest
        } else if spec.contains("://") || spec.contains('@') {
            return Err(GitHubError::InvalidRepo(format!(
                "Only GitHub repositories are supported, and '{}' is not one",
                spec
            )));
        } else {
            spec
        };

        let path = path.trim_end_matches('/');
        let path = path.strip_suffix(".git").unwrap_or(path);

        let mut parts = path.split('/');
        let (Some(owner), Some(repo), None) = (parts.next(), parts.next(), parts.next()) else {
            return Err(GitHubError::InvalidRepo(format!(
                "Expected 'owner/repo', got '{}'",
                spec
            )));
        };

        if owner.is_empty() || repo.is_empty() {
            return Err(GitHubError::InvalidRepo(format!(
                "Expected 'owner/repo', got '{}'",
                spec
            )));
        }
        if !is_safe_segment(owner) || !is_safe_segment(repo) {
            return Err(GitHubError::InvalidRepo(format!(
                "'{}' contains characters a GitHub owner or repository name cannot have",
                spec
            )));
        }

        Ok(Self {
            owner: owner.to_string(),
            repo: repo.to_string(),
        })
    }
}

/// Whether a path segment can be dropped into a URL as-is.
///
/// This is a whitelist rather than an escape, because everything GitHub permits
/// in an owner, repository, or branch name is already URL-safe. Anything else
/// is a caller mistake worth naming, not something to quietly encode.
fn is_safe_segment(segment: &str) -> bool {
    !segment.is_empty()
        && segment.len() <= 128
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '/'))
        && !segment.contains("..")
}

/// A branch resolved to the commit it points at.
#[derive(Debug, Clone)]
pub struct ResolvedRef {
    pub branch: String,
    pub commit: String,
}

#[derive(Debug, Deserialize)]
struct RepoInfo {
    default_branch: String,
}

#[derive(Debug, Deserialize)]
struct GitRef {
    object: GitRefObject,
}

#[derive(Debug, Deserialize)]
struct GitRefObject {
    sha: String,
}

/// A GitHub endpoint this engine reads from.
///
/// The API base is a field rather than a constant so the whole client can be
/// pointed at a stand-in. That is not only a testing convenience: a pull is the
/// one engine operation whose behaviour depends on a third party's HTTP
/// responses, and a suite that cannot exercise those responses is a suite that
/// only tests the half of the feature that was never in doubt.
pub struct GitHubClient {
    http: HttpClient,
    api_base: String,
    /// The caller's personal access token, when they have one stored.
    ///
    /// Held rather than passed per call because every request this client makes
    /// on a caller's behalf should carry it: a token that reaches the archive
    /// but not the ref lookup produces a 404 on a private repository that reads
    /// exactly like "no such repository".
    token: Option<String>,
}

impl GitHubClient {
    /// A client pointed at GitHub.
    pub fn new() -> Result<Self, GitHubError> {
        Ok(Self {
            http: HttpClient::new()
                .map_err(|e| GitHubError::Transport(format!("Could not build a client: {}", e)))?,
            api_base: API_BASE.to_string(),
            token: None,
        })
    }

    /// The same client, authenticating as whoever owns `token`.
    pub fn with_token(mut self, token: Option<String>) -> Self {
        self.token = token;
        self
    }

    /// Whether this client is carrying a credential, which decides how honestly
    /// it can explain a 404.
    pub fn is_authenticated(&self) -> bool {
        self.token.is_some()
    }

    /// A client pointed at a stand-in on loopback.
    ///
    /// Uses the HTTP client's test mode, which is what permits a private
    /// address at all — the production path blocks loopback precisely so that a
    /// caller-supplied repository URL cannot reach inside the network.
    #[doc(hidden)]
    pub fn for_tests(api_base: impl Into<String>) -> Result<Self, GitHubError> {
        Ok(Self {
            http: HttpClient::new_for_tests()
                .map_err(|e| GitHubError::Transport(format!("Could not build a client: {}", e)))?,
            api_base: api_base.into(),
            token: None,
        })
    }

    /// The repository's default branch, for a caller who did not name one.
    ///
    /// Worth one extra request: the alternative is guessing `main` and falling
    /// back to `master`, which is wrong for every repository that uses neither
    /// and turns a clear answer into two confusing 404s.
    pub fn default_branch(&self, repo: &RepoRef) -> Result<String, GitHubError> {
        let url = format!("{}/repos/{}/{}", self.api_base, repo.owner, repo.repo);
        let info: RepoInfo = self.get_json(&url, repo)?;
        Ok(info.default_branch)
    }

    /// Resolve a branch to the commit sha it currently points at.
    ///
    /// Uses the Git Data ref endpoint rather than the commits endpoint, whose
    /// response carries the commit's full file list and can run to hundreds of
    /// kilobytes for a large merge. All that is wanted here is one sha.
    pub fn resolve_branch(&self, repo: &RepoRef, branch: &str) -> Result<ResolvedRef, GitHubError> {
        if !is_safe_segment(branch) {
            return Err(GitHubError::InvalidRef(format!(
                "'{}' is not a usable branch name",
                branch
            )));
        }

        let url = format!(
            "{}/repos/{}/{}/git/ref/heads/{}",
            self.api_base, repo.owner, repo.repo, branch
        );

        let git_ref: GitRef = self.get_json(&url, repo).map_err(|e| match e {
            // The ref endpoint answers 404 both for "no such repository" and for
            // "no such branch", and the caller can only act on the difference.
            GitHubError::NotFound(_) => GitHubError::NotFound(format!(
                "Branch '{}' does not exist in {} (or the repository is private)",
                branch, repo
            )),
            other => other,
        })?;

        Ok(ResolvedRef {
            branch: branch.to_string(),
            commit: git_ref.object.sha,
        })
    }

    /// Download the repository's tree at `commit` as a gzipped tar.
    ///
    /// The API answers with a redirect to `codeload.github.com`; the HTTP
    /// client follows it and validates that hop like any other, which is why
    /// this is one call and not two.
    pub fn fetch_archive(&self, repo: &RepoRef, commit: &str) -> Result<Vec<u8>, GitHubError> {
        if !is_safe_segment(commit) {
            return Err(GitHubError::InvalidRef(format!(
                "'{}' is not a usable commit reference",
                commit
            )));
        }

        let url = format!(
            "{}/repos/{}/{}/tarball/{}",
            self.api_base, repo.owner, repo.repo, commit
        );

        let response = self
            .http
            .fetch_bytes(&url, Some(self.headers()), MAX_ARCHIVE_BYTES)
            .map_err(|e| classify(e, repo))?;

        check_status(
            response.status,
            response.ok,
            repo,
            &url,
            self.is_authenticated(),
        )?;
        Ok(response.body)
    }

    fn get_json<T: for<'de> Deserialize<'de>>(
        &self,
        url: &str,
        repo: &RepoRef,
    ) -> Result<T, GitHubError> {
        let response = self
            .http
            .fetch_bytes(url, Some(self.headers()), MAX_JSON_BYTES)
            .map_err(|e| classify(e, repo))?;

        check_status(
            response.status,
            response.ok,
            repo,
            url,
            self.is_authenticated(),
        )?;

        serde_json::from_slice(&response.body).map_err(|e| {
            GitHubError::UnexpectedResponse(format!(
                "Could not read GitHub's answer from {}: {}",
                url, e
            ))
        })
    }
}

impl GitHubClient {
    fn headers(&self) -> HashMap<String, String> {
        let mut headers = HashMap::from([
            ("User-Agent".to_string(), USER_AGENT.to_string()),
            (
                "Accept".to_string(),
                "application/vnd.github+json".to_string(),
            ),
            ("X-GitHub-Api-Version".to_string(), API_VERSION.to_string()),
        ]);
        if let Some(token) = &self.token {
            headers.insert("Authorization".to_string(), format!("Bearer {}", token));
        }
        headers
    }

    /// Who this token belongs to, and whether the host still accepts it.
    ///
    /// Checked before a credential is stored, so a token that was mistyped or
    /// has already been revoked is refused at the point somebody can do
    /// something about it rather than at the next pull.
    pub fn verify_token(&self) -> Result<String, GitHubError> {
        #[derive(Deserialize)]
        struct Account {
            login: String,
        }

        let url = format!("{}/user", self.api_base);
        let response = self
            .http
            .fetch_bytes(&url, Some(self.headers()), MAX_JSON_BYTES)
            .map_err(|e| GitHubError::Transport(e.to_string()))?;

        if response.status == 401 || response.status == 403 {
            return Err(GitHubError::Forbidden(
                "GitHub did not accept this token. Check that it is correct and has not \
                 expired or been revoked."
                    .to_string(),
            ));
        }
        if !response.ok {
            return Err(GitHubError::UnexpectedResponse(format!(
                "GitHub answered {} when asked whose token this is",
                response.status
            )));
        }

        let account: Account = serde_json::from_slice(&response.body).map_err(|e| {
            GitHubError::UnexpectedResponse(format!("Could not read GitHub's answer: {}", e))
        })?;
        Ok(account.login)
    }
}

/// A JSON response bounded well below the archive ceiling — these endpoints
/// answer with a few kilobytes, and anything far larger is not what was asked
/// for.
const MAX_JSON_BYTES: usize = 1024 * 1024;

/// Turn a status code into the thing the caller can act on.
///
/// The distinction that matters is 404 against everything else: for a public
/// repository, a 404 is as likely to mean "this repository is private" as "no
/// such repository", and until credentials exist those are the same problem
/// wearing different words.
fn check_status(
    status: u16,
    ok: bool,
    repo: &RepoRef,
    url: &str,
    authenticated: bool,
) -> Result<(), GitHubError> {
    if ok {
        return Ok(());
    }
    match status {
        // GitHub answers 404 rather than 403 for a repository the caller may
        // not see, so what this can honestly say depends on whether a
        // credential was presented at all.
        404 if authenticated => Err(GitHubError::NotFound(format!(
            "{} was not found. It does not exist, or the token in use cannot see it.",
            repo
        ))),
        404 => Err(GitHubError::NotFound(format!(
            "{} was not found. It may not exist, or it may be private — no credential was \
             used for this pull. Store a token for github.com and try again.",
            repo
        ))),
        401 | 403 if authenticated => Err(GitHubError::Forbidden(format!(
            "GitHub refused the request for {}. The token may have expired, been revoked, \
             or lack access to this repository.",
            repo
        ))),
        401 | 403 => Err(GitHubError::Forbidden(format!(
            "GitHub refused the request for {}. Unauthenticated requests are rate limited \
             to 60 per hour per address.",
            repo
        ))),
        429 => Err(GitHubError::Forbidden(format!(
            "GitHub is rate limiting requests for {}. Try again shortly.",
            repo
        ))),
        _ => Err(GitHubError::UnexpectedResponse(format!(
            "GitHub answered {} for {}",
            status, url
        ))),
    }
}

fn classify(error: HttpError, repo: &RepoRef) -> GitHubError {
    match error {
        HttpError::ResponseTooLarge(bytes) => GitHubError::TooLarge(format!(
            "{} is larger than this engine will download ({} bytes, limit {})",
            repo, bytes, MAX_ARCHIVE_BYTES
        )),
        other => GitHubError::Transport(other.to_string()),
    }
}

// ============================================================================
// Writing
// ============================================================================

/// One entry of a git tree, as GitHub reports and accepts it.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TreeEntry {
    pub path: String,
    /// `100644` for a file, `100755` executable, `120000` symlink, `160000`
    /// submodule, `040000` a directory. Preserved verbatim for anything this
    /// engine did not write, so a push does not quietly change a file's mode.
    pub mode: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub sha: String,
}

#[derive(Debug, Deserialize)]
struct TreeResponse {
    sha: String,
    #[serde(default)]
    tree: Vec<TreeEntry>,
    /// GitHub sets this when the tree was too large to return whole.
    #[serde(default)]
    truncated: bool,
}

#[derive(Debug, Deserialize)]
struct ShaResponse {
    sha: String,
}

/// A commit's own sha and the sha of the tree it points at.
#[derive(Debug, Clone)]
pub struct CommitTree {
    pub commit: String,
    pub tree: String,
    pub entries: Vec<TreeEntry>,
}

impl GitHubClient {
    /// Every file a commit's tree holds, flattened.
    ///
    /// Refuses a truncated answer rather than working with part of one. A push
    /// builds the next tree out of this, so a tree missing entries GitHub did
    /// not send would delete every file it could not see.
    pub fn tree_at(&self, repo: &RepoRef, commit: &str) -> Result<CommitTree, GitHubError> {
        if !is_safe_segment(commit) {
            return Err(GitHubError::InvalidRef(format!(
                "'{}' is not a usable commit reference",
                commit
            )));
        }
        let url = format!(
            "{}/repos/{}/{}/git/trees/{}?recursive=1",
            self.api_base, repo.owner, repo.repo, commit
        );
        let tree: TreeResponse = self.get_json(&url, repo)?;

        if tree.truncated {
            return Err(GitHubError::TooLarge(format!(
                "{} has more files than GitHub will list in one response, so this engine \
                 cannot safely rebuild its tree",
                repo
            )));
        }

        Ok(CommitTree {
            commit: commit.to_string(),
            tree: tree.sha,
            entries: tree.tree,
        })
    }

    /// Upload one file's content and get the blob sha back.
    pub fn create_blob(&self, repo: &RepoRef, content: &[u8]) -> Result<String, GitHubError> {
        let encoded = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, content);
        let url = format!(
            "{}/repos/{}/{}/git/blobs",
            self.api_base, repo.owner, repo.repo
        );
        let response: ShaResponse = self.send_json(
            "POST",
            &url,
            repo,
            &serde_json::json!({ "content": encoded, "encoding": "base64" }),
        )?;
        Ok(response.sha)
    }

    /// Assemble a tree from a complete list of entries.
    ///
    /// The whole tree rather than a delta against `base_tree`: a delta
    /// expresses a removal as an explicit null, and getting that wrong leaves
    /// a file behind rather than failing, which is the kind of mistake nobody
    /// notices. Directories are inferred from the paths, so only leaves are
    /// sent.
    pub fn create_tree(
        &self,
        repo: &RepoRef,
        entries: &[TreeEntry],
    ) -> Result<String, GitHubError> {
        let url = format!(
            "{}/repos/{}/{}/git/trees",
            self.api_base, repo.owner, repo.repo
        );
        let response: ShaResponse =
            self.send_json("POST", &url, repo, &serde_json::json!({ "tree": entries }))?;
        Ok(response.sha)
    }

    /// Commit a tree.
    ///
    /// `parents` empty makes a root commit, which is what a push into an empty
    /// repository needs.
    pub fn create_commit(
        &self,
        repo: &RepoRef,
        message: &str,
        tree: &str,
        parents: &[String],
    ) -> Result<String, GitHubError> {
        let url = format!(
            "{}/repos/{}/{}/git/commits",
            self.api_base, repo.owner, repo.repo
        );
        let response: ShaResponse = self.send_json(
            "POST",
            &url,
            repo,
            &serde_json::json!({ "message": message, "tree": tree, "parents": parents }),
        )?;
        Ok(response.sha)
    }

    /// Move a branch to `commit`.
    ///
    /// Never forced. GitHub refuses a non-fast-forward update without `force`,
    /// which is a second wall behind this engine's own divergence check — and
    /// the right one to keep, because it is the only check that sees a push
    /// somebody else landed between our check and our write.
    pub fn update_ref(
        &self,
        repo: &RepoRef,
        branch: &str,
        commit: &str,
    ) -> Result<(), GitHubError> {
        if !is_safe_segment(branch) {
            return Err(GitHubError::InvalidRef(format!(
                "'{}' is not a usable branch name",
                branch
            )));
        }
        let url = format!(
            "{}/repos/{}/{}/git/refs/heads/{}",
            self.api_base, repo.owner, repo.repo, branch
        );
        let _: serde_json::Value = self.send_json(
            "PATCH",
            &url,
            repo,
            &serde_json::json!({ "sha": commit, "force": false }),
        )?;
        Ok(())
    }

    /// Create a branch pointing at `commit`, for a repository that has none.
    pub fn create_ref(
        &self,
        repo: &RepoRef,
        branch: &str,
        commit: &str,
    ) -> Result<(), GitHubError> {
        if !is_safe_segment(branch) {
            return Err(GitHubError::InvalidRef(format!(
                "'{}' is not a usable branch name",
                branch
            )));
        }
        let url = format!(
            "{}/repos/{}/{}/git/refs",
            self.api_base, repo.owner, repo.repo
        );
        let _: serde_json::Value = self.send_json(
            "POST",
            &url,
            repo,
            &serde_json::json!({ "ref": format!("refs/heads/{}", branch), "sha": commit }),
        )?;
        Ok(())
    }

    /// One blob's bytes, by sha.
    ///
    /// Used to read a repository's ignore file out of the commit a push is
    /// landing on, so the rules applied are the ones that repository ships
    /// rather than whatever this engine last saw.
    pub fn blob(&self, repo: &RepoRef, sha: &str) -> Result<Vec<u8>, GitHubError> {
        if !is_safe_segment(sha) {
            return Err(GitHubError::InvalidRef(format!(
                "'{}' is not a blob id",
                sha
            )));
        }
        let url = format!(
            "{}/repos/{}/{}/git/blobs/{}",
            self.api_base, repo.owner, repo.repo, sha
        );

        #[derive(Deserialize)]
        struct Blob {
            content: String,
            encoding: String,
        }

        let blob: Blob = self.get_json(&url, repo)?;
        if blob.encoding != "base64" {
            return Err(GitHubError::UnexpectedResponse(format!(
                "GitHub returned a blob encoded as '{}', which this engine cannot read",
                blob.encoding
            )));
        }
        // GitHub wraps base64 blob content at 60 columns.
        let cleaned: String = blob
            .content
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect();
        base64::Engine::decode(&base64::engine::general_purpose::STANDARD, cleaned)
            .map_err(|e| GitHubError::UnexpectedResponse(format!("Unreadable blob: {}", e)))
    }

    fn send_json<T: for<'de> Deserialize<'de>>(
        &self,
        method: &str,
        url: &str,
        repo: &RepoRef,
        body: &serde_json::Value,
    ) -> Result<T, GitHubError> {
        let mut headers = self.headers();
        // GitHub infers this, and a stricter server does not — a JSON body
        // deserves to say what it is either way.
        headers.insert("Content-Type".to_string(), "application/json".to_string());

        let options = crate::http_client::FetchOptions {
            method: method.to_string(),
            headers: Some(headers),
            body: Some(body.to_string()),
            timeout_ms: None,
        };

        let response = self
            .http
            .fetch(url.to_string(), options, None, None)
            .map_err(|e| classify(e, repo))?;

        check_write_status(response.status, response.ok, repo, url, &response.body)?;

        serde_json::from_str(&response.body).map_err(|e| {
            GitHubError::UnexpectedResponse(format!("Could not read GitHub's answer: {}", e))
        })
    }
}

/// A write refused is worth reporting in GitHub's own words.
///
/// The read path can say what a status means because there are only a few ways
/// to fail a read. A write fails for reasons that are specific and actionable —
/// a protected branch, a missing scope, a ref that moved — and GitHub says
/// which in the body, where paraphrasing it would lose the part that helps.
fn check_write_status(
    status: u16,
    ok: bool,
    repo: &RepoRef,
    url: &str,
    body: &str,
) -> Result<(), GitHubError> {
    if ok {
        return Ok(());
    }

    let detail = serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .and_then(|parsed| {
            parsed
                .get("message")
                .and_then(|m| m.as_str())
                .map(str::to_string)
        })
        .unwrap_or_else(|| format!("HTTP {}", status));

    match status {
        401 | 403 => Err(GitHubError::Forbidden(format!(
            "GitHub refused to write to {}: {}. The token needs write access to this \
             repository's contents.",
            repo, detail
        ))),
        404 => Err(GitHubError::NotFound(format!(
            "{} was not found, or the token cannot write to it: {}",
            repo, detail
        ))),
        409 | 422 => Err(GitHubError::Conflict(format!(
            "GitHub would not apply the change to {}: {}",
            repo, detail
        ))),
        _ => Err(GitHubError::UnexpectedResponse(format!(
            "GitHub answered {} for {}: {}",
            status, url, detail
        ))),
    }
}

#[derive(Debug, Error)]
pub enum GitHubError {
    #[error("{0}")]
    InvalidRepo(String),

    #[error("{0}")]
    InvalidRef(String),

    #[error("{0}")]
    NotFound(String),

    #[error("{0}")]
    Forbidden(String),

    #[error("{0}")]
    TooLarge(String),

    /// The host would not apply the change — a ref that moved under us, a
    /// protected branch, a tree it rejected.
    #[error("{0}")]
    Conflict(String),

    #[error("Could not reach GitHub: {0}")]
    Transport(String),

    #[error("{0}")]
    UnexpectedResponse(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_owner_slash_repo() {
        let parsed = RepoRef::parse("lpajunen/solution").expect("should parse");
        assert_eq!(parsed.owner, "lpajunen");
        assert_eq!(parsed.repo, "solution");
    }

    #[test]
    fn parses_browser_url() {
        let parsed = RepoRef::parse("https://github.com/lpajunen/solution").expect("should parse");
        assert_eq!(parsed.to_string(), "lpajunen/solution");
    }

    #[test]
    fn parses_clone_urls() {
        for spec in [
            "https://github.com/lpajunen/solution.git",
            "git@github.com:lpajunen/solution.git",
            "https://github.com/lpajunen/solution/",
        ] {
            let parsed = RepoRef::parse(spec).unwrap_or_else(|e| panic!("{} -> {}", spec, e));
            assert_eq!(parsed.to_string(), "lpajunen/solution", "for {}", spec);
        }
    }

    #[test]
    fn rejects_other_hosts() {
        let error = RepoRef::parse("https://gitlab.com/owner/repo").expect_err("should refuse");
        assert!(
            error.to_string().contains("Only GitHub"),
            "unexpected: {}",
            error
        );
    }

    #[test]
    fn rejects_incomplete_specs() {
        for spec in ["owner", "owner/repo/extra", "", "/repo", "owner/"] {
            assert!(
                RepoRef::parse(spec).is_err(),
                "'{}' should be refused",
                spec
            );
        }
    }

    #[test]
    fn rejects_traversal_in_segments() {
        assert!(RepoRef::parse("owner/..").is_err());
        assert!(RepoRef::parse("../etc/passwd").is_err());
    }

    #[test]
    fn rejects_unusable_branch_names() {
        assert!(!is_safe_segment("feature branch"));
        assert!(!is_safe_segment("../main"));
        assert!(is_safe_segment("release/2.0"));
        assert!(is_safe_segment("main"));
    }
}
