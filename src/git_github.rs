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

use serde::Deserialize;
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
}

impl GitHubClient {
    /// A client pointed at GitHub.
    pub fn new() -> Result<Self, GitHubError> {
        Ok(Self {
            http: HttpClient::new()
                .map_err(|e| GitHubError::Transport(format!("Could not build a client: {}", e)))?,
            api_base: API_BASE.to_string(),
        })
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
            .fetch_bytes(&url, Some(api_headers()), MAX_ARCHIVE_BYTES)
            .map_err(|e| classify(e, repo))?;

        check_status(response.status, response.ok, repo, &url)?;
        Ok(response.body)
    }

    fn get_json<T: for<'de> Deserialize<'de>>(
        &self,
        url: &str,
        repo: &RepoRef,
    ) -> Result<T, GitHubError> {
        let response = self
            .http
            .fetch_bytes(url, Some(api_headers()), MAX_JSON_BYTES)
            .map_err(|e| classify(e, repo))?;

        check_status(response.status, response.ok, repo, url)?;

        serde_json::from_slice(&response.body).map_err(|e| {
            GitHubError::UnexpectedResponse(format!(
                "Could not read GitHub's answer from {}: {}",
                url, e
            ))
        })
    }
}

fn api_headers() -> HashMap<String, String> {
    HashMap::from([
        ("User-Agent".to_string(), USER_AGENT.to_string()),
        (
            "Accept".to_string(),
            "application/vnd.github+json".to_string(),
        ),
        ("X-GitHub-Api-Version".to_string(), API_VERSION.to_string()),
    ])
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
fn check_status(status: u16, ok: bool, repo: &RepoRef, url: &str) -> Result<(), GitHubError> {
    if ok {
        return Ok(());
    }
    match status {
        404 => Err(GitHubError::NotFound(format!(
            "{} was not found. It may not exist, or it may be private — this engine can \
             only read public repositories.",
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
