//! `POST /engine/git/pull`: a repository read into the engine as scripts.
//!
//! These run against a stand-in for GitHub rather than GitHub itself, for the
//! reason the rest of the suite avoids the network: a test that depends on a
//! third party's availability fails for reasons that have nothing to do with
//! the code. The stand-in serves the three shapes the client actually reads —
//! repository metadata, a git ref, and a tarball — so what is under test is the
//! whole path from an HTTP response to a stored script.
//!
//! The mapping is the thing worth pinning down. Nothing in a repository says
//! where its files land; the engine works it out from the directory structure,
//! and these tests are what say that reading is stable.

mod common;

use aiwebengine::git_github::GitHubClient;
use aiwebengine::git_sync::{PullRequest, pull_with};
use aiwebengine::repository;
use aiwebengine::security::UserContext;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use common::{setup_env, test_mutex};
use serde_json::json;
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

// ============================================================================
// A stand-in for GitHub
// ============================================================================

#[derive(Clone)]
struct FakeState {
    default_branch: String,
    /// The commit the branch points at. Changing it is this fixture's
    /// equivalent of somebody pushing.
    commit: String,
    /// Repository path to content.
    tree: BTreeMap<String, String>,
}

#[derive(Clone)]
struct FakeGitHub {
    base: String,
    state: Arc<Mutex<FakeState>>,
}

impl FakeGitHub {
    async fn start(tree: &[(&str, &str)]) -> anyhow::Result<Self> {
        let state = Arc::new(Mutex::new(FakeState {
            default_branch: "main".to_string(),
            commit: "1111111111111111111111111111111111111111".to_string(),
            tree: tree
                .iter()
                .map(|(path, content)| (path.to_string(), content.to_string()))
                .collect(),
        }));

        let app = axum::Router::new()
            .route("/repos/{owner}/{repo}", axum::routing::get(repo_info))
            .route(
                "/repos/{owner}/{repo}/git/ref/heads/{*branch}",
                axum::routing::get(git_ref),
            )
            .route(
                "/repos/{owner}/{repo}/tarball/{sha}",
                axum::routing::get(tarball),
            )
            .with_state(state.clone());

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let addr: SocketAddr = listener.local_addr()?;
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        Ok(Self {
            base: format!("http://127.0.0.1:{}", addr.port()),
            state,
        })
    }

    fn client(&self) -> GitHubClient {
        GitHubClient::for_tests(&self.base).expect("test client should build")
    }

    /// Replace the tree and move the branch, the way a push would.
    fn push(&self, commit: &str, tree: &[(&str, &str)]) {
        let mut state = self.state.lock().expect("fixture state");
        state.commit = commit.to_string();
        state.tree = tree
            .iter()
            .map(|(path, content)| (path.to_string(), content.to_string()))
            .collect();
    }
}

async fn repo_info(State(state): State<Arc<Mutex<FakeState>>>) -> Response {
    let branch = state
        .lock()
        .map(|s| s.default_branch.clone())
        .unwrap_or_default();
    axum::Json(json!({ "default_branch": branch })).into_response()
}

async fn git_ref(
    State(state): State<Arc<Mutex<FakeState>>>,
    Path((_owner, _repo, branch)): Path<(String, String, String)>,
) -> Response {
    let Ok(state) = state.lock() else {
        return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
    };
    if branch != state.default_branch {
        return (axum::http::StatusCode::NOT_FOUND, "").into_response();
    }
    axum::Json(json!({ "object": { "sha": state.commit, "type": "commit" } })).into_response()
}

async fn tarball(
    State(state): State<Arc<Mutex<FakeState>>>,
    Path((owner, repo, sha)): Path<(String, String, String)>,
) -> Response {
    let Ok(state) = state.lock() else {
        return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
    };
    if sha != state.commit {
        return (axum::http::StatusCode::NOT_FOUND, "").into_response();
    }

    // GitHub wraps every archive in one `{owner}-{repo}-{short sha}` directory,
    // and stripping it is part of what is under test.
    let prefix = format!("{}-{}-{}", owner, repo, &sha[..7.min(sha.len())]);
    let entries: Vec<(String, String)> = state
        .tree
        .iter()
        .map(|(path, content)| (format!("{}/{}", prefix, path), content.clone()))
        .collect();

    (
        [(axum::http::header::CONTENT_TYPE, "application/gzip")],
        gzipped_tar(&entries),
    )
        .into_response()
}

fn gzipped_tar(entries: &[(String, String)]) -> Vec<u8> {
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

// ============================================================================
// Helpers
// ============================================================================

fn puller() -> UserContext {
    UserContext::admin("git-puller".to_string())
}

fn request(prefix: &str) -> PullRequest {
    PullRequest {
        repo: "lpajunen/solution".to_string(),
        branch: None,
        prefix: Some(prefix.to_string()),
    }
}

fn asset_text(script_uri: &str, asset_uri: &str) -> Option<String> {
    repository::fetch_asset(script_uri, asset_uri)
        .and_then(|asset| String::from_utf8(asset.content).ok())
}

/// Remove whatever an earlier run of the same test left behind, so a test that
/// asserts on deletion is asserting on this run's deletion.
fn clear(script_uri: &str) {
    for existing in repository::fetch_assets(script_uri).keys() {
        repository::delete_asset(script_uri, existing);
    }
}

// ============================================================================
// Tests
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn a_repository_with_a_root_entry_becomes_one_script() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(&[
        ("main.ts", "function init() {}"),
        ("server/handler.ts", "export const x = 1;"),
        ("templates/page.html", "<p>hi</p>"),
        ("README.md", "not an asset"),
        (".gitignore", "target"),
    ])
    .await
    .expect("fixture should start");

    let uri = "git-one/main.ts";
    clear(uri);

    let report = pull_with(github.client(), &puller(), request("git-one"))
        .await
        .expect("pull should succeed");

    assert_eq!(report.branch, "main");
    assert_eq!(report.scripts.len(), 1, "one script expected");
    assert_eq!(report.scripts[0].script_uri, uri);

    // The URI ends with the entry's own file name, because a script's imports
    // resolve against the basename of its URI.
    assert!(
        repository::fetch_script(uri).is_some(),
        "script should exist"
    );

    assert_eq!(
        asset_text(uri, "server/handler.ts").as_deref(),
        Some("export const x = 1;"),
        "an asset keeps its path relative to the script's root"
    );
    assert_eq!(
        asset_text(uri, "templates/page.html").as_deref(),
        Some("<p>hi</p>")
    );

    // Repository furniture is not content the solution serves.
    assert!(
        asset_text(uri, "README.md").is_none(),
        "README is not an asset"
    );
    assert!(
        asset_text(uri, ".gitignore").is_none(),
        "dotfiles are dropped"
    );
    assert!(
        asset_text(uri, "main.ts").is_none(),
        "the entry is the script row, not one of its assets"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn each_top_level_directory_becomes_its_own_script() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(&[
        ("shop/main.ts", "function init() {}"),
        ("shop/lib/cart.ts", "export const cart = [];"),
        ("admin/main.ts", "function init() {}"),
    ])
    .await
    .expect("fixture should start");

    clear("git-many/shop/main.ts");
    clear("git-many/admin/main.ts");

    let report = pull_with(github.client(), &puller(), request("git-many"))
        .await
        .expect("pull should succeed");

    let mut uris: Vec<&str> = report
        .scripts
        .iter()
        .map(|script| script.script_uri.as_str())
        .collect();
    uris.sort();
    assert_eq!(
        uris,
        vec!["git-many/admin/main.ts", "git-many/shop/main.ts"]
    );

    assert_eq!(
        asset_text("git-many/shop/main.ts", "lib/cart.ts").as_deref(),
        Some("export const cart = [];"),
        "asset paths are relative to their own script, not to the repository"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pulling_an_unchanged_commit_does_no_work() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(&[("main.ts", "function init() {}")])
        .await
        .expect("fixture should start");

    clear("git-idempotent/main.ts");

    let first = pull_with(github.client(), &puller(), request("git-idempotent"))
        .await
        .expect("first pull should succeed");
    assert!(!first.up_to_date, "the first pull has work to do");

    let second = pull_with(github.client(), &puller(), request("git-idempotent"))
        .await
        .expect("second pull should succeed");
    assert!(
        second.up_to_date,
        "a repository that has not moved is not downloaded again"
    );
    assert!(second.scripts.is_empty(), "nothing was written");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_removed_upstream_is_removed_here() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(&[
        ("main.ts", "function init() {}"),
        ("lib/keep.ts", "export const keep = 1;"),
        ("lib/drop.ts", "export const drop = 2;"),
    ])
    .await
    .expect("fixture should start");

    let uri = "git-sync/main.ts";
    clear(uri);

    pull_with(github.client(), &puller(), request("git-sync"))
        .await
        .expect("first pull should succeed");
    assert!(
        asset_text(uri, "lib/drop.ts").is_some(),
        "written by the first pull"
    );

    // Somebody removed the module and pushed.
    github.push(
        "2222222222222222222222222222222222222222",
        &[
            ("main.ts", "function init() {}"),
            ("lib/keep.ts", "export const keep = 1;"),
        ],
    );

    let report = pull_with(github.client(), &puller(), request("git-sync"))
        .await
        .expect("second pull should succeed");

    assert_eq!(report.scripts[0].deleted, 1, "the removal is applied here");
    assert!(
        asset_text(uri, "lib/drop.ts").is_none(),
        "a pull is a sync: a module the repository dropped does not survive it"
    );
    assert!(
        asset_text(uri, "lib/keep.ts").is_some(),
        "and one it still holds does"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pull_records_one_revision_for_the_whole_script() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(&[
        ("main.ts", "function init() {}"),
        ("a.ts", "export const a = 1;"),
        ("b.ts", "export const b = 2;"),
        ("c.ts", "export const c = 3;"),
    ])
    .await
    .expect("fixture should start");

    let uri = "git-revision/main.ts";
    clear(uri);

    let before = aiwebengine::revisions::current(uri).unwrap_or(0);
    let report = pull_with(github.client(), &puller(), request("git-revision"))
        .await
        .expect("pull should succeed");
    let after = aiwebengine::revisions::current(uri).unwrap_or(0);

    assert_eq!(report.scripts[0].written, 3, "three assets written");
    assert_eq!(
        after - before,
        1,
        "three files are one change, so they are one revision"
    );
    assert_eq!(report.scripts[0].revision, Some(after));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_repository_with_no_entry_point_is_refused() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(&[("lib/util.ts", "export const x = 1;"), ("README.md", "hi")])
        .await
        .expect("fixture should start");

    let error = pull_with(github.client(), &puller(), request("git-empty"))
        .await
        .expect_err("a repository with no entry point should be refused");

    assert!(
        error.to_string().contains("main.ts"),
        "the refusal should name what it looked for, got: {}",
        error
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_missing_branch_is_reported_as_missing() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(&[("main.ts", "function init() {}")])
        .await
        .expect("fixture should start");

    let error = pull_with(
        github.client(),
        &puller(),
        PullRequest {
            repo: "lpajunen/solution".to_string(),
            branch: Some("no-such-branch".to_string()),
            prefix: Some("git-branch".to_string()),
        },
    )
    .await
    .expect_err("a branch that does not exist should be refused");

    assert!(
        error.to_string().contains("no-such-branch"),
        "the refusal should name the branch, got: {}",
        error
    );
}

/// The MCP tool — the entry point an agent actually uses — reaches `pull`
/// through `database::run_blocking` from a blocking context, and `pull` in turn
/// hands its network and database work to `spawn_blocking`. That nesting is the
/// kind that deadlocks when it is wrong, and it is worth one test that is not
/// about the mapping at all.
#[tokio::test(flavor = "multi_thread")]
async fn a_pull_completes_when_driven_from_a_blocking_context() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(&[
        ("main.ts", "function init() {}"),
        ("lib/util.ts", "export const util = 1;"),
    ])
    .await
    .expect("fixture should start");

    let uri = "git-blocking/main.ts";
    clear(uri);

    let client = github.client();
    let user = puller();
    let report = tokio::task::spawn_blocking(move || {
        // The same sequence `database::run_blocking` performs: hand the
        // reactor off, then drive the future to completion from here.
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(pull_with(
                client,
                &user,
                request("git-blocking"),
            ))
        })
    })
    .await
    .expect("the blocking task should finish")
    .expect("pull should succeed");

    assert_eq!(report.scripts.len(), 1);
    assert_eq!(report.scripts[0].script_uri, uri);
    assert!(
        asset_text(uri, "lib/util.ts").is_some(),
        "the pull ran to completion rather than stalling"
    );
}

/// The ignore file, read through a whole pull rather than against the matcher.
///
/// Worth having end to end because the rules are written against repository
/// paths while assets are keyed by script-relative ones, and that translation
/// is where the feature would silently do the wrong thing.
#[tokio::test(flavor = "multi_thread")]
async fn the_repository_says_what_not_to_take() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(&[
        ("main.ts", "function init() {}"),
        ("lib/cart.ts", "export const cart = [];"),
        ("tsconfig.json", "{}"),
        ("docs/guide.md", "long explanation"),
        ("templates/help.md", "served to users"),
        ("README.md", "repository furniture"),
        (
            ".aiwebengineignore",
            "# editor support, meaningless to the engine\ntsconfig.json\n\n*.md\n!templates/help.md\n",
        ),
    ])
    .await
    .expect("fixture should start");

    let uri = "git-ignore/main.ts";
    clear(uri);

    pull_with(github.client(), &puller(), request("git-ignore"))
        .await
        .expect("pull should succeed");

    assert!(
        asset_text(uri, "lib/cart.ts").is_some(),
        "a module nothing mentions is still content"
    );
    assert!(
        asset_text(uri, "tsconfig.json").is_none(),
        "a named file is dropped"
    );
    assert!(
        asset_text(uri, "docs/guide.md").is_none(),
        "*.md matches at any depth"
    );
    assert_eq!(
        asset_text(uri, "templates/help.md").as_deref(),
        Some("served to users"),
        "and a negation puts back the one page the solution serves"
    );
    assert!(
        asset_text(uri, ".aiwebengineignore").is_none(),
        "the rules are not themselves content"
    );
}
