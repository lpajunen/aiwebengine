//! Publishing a script back to the repository it came from.
//!
//! The stand-in here is a real if tiny git object store — blobs addressed the
//! way git addresses them, trees, commits with parents, and a branch ref —
//! because a push is not one request whose response you can stub. It uploads
//! blobs, assembles a tree, commits it and moves a ref, and the thing worth
//! testing is what the repository *holds* at the end of that, which only a
//! store can answer.

mod common;

use aiwebengine::git_github::GitHubClient;
use aiwebengine::git_sync::{
    PullRequest, PushRequest, SyncState, pull_with, push_with, status_with,
};
use aiwebengine::repository;
use aiwebengine::security::UserContext;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use common::{setup_env, test_mutex};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

// ============================================================================
// A stand-in with an object store
// ============================================================================

#[derive(Clone, Default)]
struct FakeGit {
    default_branch: String,
    /// branch -> commit sha
    branches: BTreeMap<String, String>,
    /// commit sha -> (tree sha, parents)
    commits: BTreeMap<String, (String, Vec<String>)>,
    /// tree sha -> path -> blob sha
    trees: BTreeMap<String, BTreeMap<String, String>>,
    /// blob sha -> content
    blobs: BTreeMap<String, Vec<u8>>,
}

/// Git's own blob id, which the engine computes too — a push recognising a file
/// the repository already holds depends on the two agreeing.
fn blob_sha(content: &[u8]) -> String {
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(format!("blob {}\0", content.len()).as_bytes());
    hasher.update(content);
    hex::encode(hasher.finalize())
}

fn digest(label: &str, body: &str) -> String {
    use sha1::{Digest, Sha1};
    let mut hasher = Sha1::new();
    hasher.update(label.as_bytes());
    hasher.update(body.as_bytes());
    hex::encode(hasher.finalize())
}

impl FakeGit {
    fn seed(&mut self, files: &[(&str, &str)]) -> String {
        let mut tree = BTreeMap::new();
        for (path, content) in files {
            let sha = blob_sha(content.as_bytes());
            self.blobs.insert(sha.clone(), content.as_bytes().to_vec());
            tree.insert(path.to_string(), sha);
        }
        let tree_sha = digest("tree", &format!("{:?}", tree));
        self.trees.insert(tree_sha.clone(), tree);
        let commit = digest("commit", &tree_sha);
        self.commits.insert(commit.clone(), (tree_sha, Vec::new()));
        self.branches
            .insert(self.default_branch.clone(), commit.clone());
        commit
    }

    /// What the branch holds now, as path -> content.
    fn snapshot(&self) -> BTreeMap<String, String> {
        let Some(commit) = self.branches.get(&self.default_branch) else {
            return BTreeMap::new();
        };
        let Some((tree_sha, _)) = self.commits.get(commit) else {
            return BTreeMap::new();
        };
        let Some(tree) = self.trees.get(tree_sha) else {
            return BTreeMap::new();
        };
        tree.iter()
            .filter_map(|(path, sha)| {
                self.blobs
                    .get(sha)
                    .map(|bytes| (path.clone(), String::from_utf8_lossy(bytes).into_owned()))
            })
            .collect()
    }
}

#[derive(Clone)]
struct Fixture {
    base: String,
    state: Arc<Mutex<FakeGit>>,
}

impl Fixture {
    async fn start(files: &[(&str, &str)]) -> anyhow::Result<Self> {
        let mut git = FakeGit {
            default_branch: "main".to_string(),
            ..Default::default()
        };
        git.seed(files);
        let state = Arc::new(Mutex::new(git));

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
            .route(
                "/repos/{owner}/{repo}/git/trees/{sha}",
                axum::routing::get(get_tree),
            )
            .route(
                "/repos/{owner}/{repo}/git/blobs",
                axum::routing::post(post_blob),
            )
            .route(
                "/repos/{owner}/{repo}/git/blobs/{sha}",
                axum::routing::get(get_blob),
            )
            .route(
                "/repos/{owner}/{repo}/git/trees",
                axum::routing::post(post_tree),
            )
            .route(
                "/repos/{owner}/{repo}/git/commits",
                axum::routing::post(post_commit),
            )
            .route(
                "/repos/{owner}/{repo}/git/refs/heads/{*branch}",
                axum::routing::patch(patch_ref),
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

    fn snapshot(&self) -> BTreeMap<String, String> {
        self.state.lock().expect("fixture state").snapshot()
    }

    /// Somebody else pushes, so the two sides diverge.
    fn land_commit(&self, files: &[(&str, &str)]) {
        let mut git = self.state.lock().expect("fixture state");
        git.seed(files);
    }
}

type Shared = State<Arc<Mutex<FakeGit>>>;

async fn repo_info(State(state): Shared) -> Response {
    let branch = state
        .lock()
        .map(|git| git.default_branch.clone())
        .unwrap_or_default();
    axum::Json(json!({ "default_branch": branch })).into_response()
}

async fn git_ref(
    State(state): Shared,
    Path((_o, _r, branch)): Path<(String, String, String)>,
) -> Response {
    let Ok(git) = state.lock() else {
        return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
    };
    match git.branches.get(&branch) {
        Some(sha) => {
            axum::Json(json!({ "object": { "sha": sha, "type": "commit" } })).into_response()
        }
        None => (
            axum::http::StatusCode::NOT_FOUND,
            axum::Json(json!({ "message": "Not Found" })),
        )
            .into_response(),
    }
}

async fn get_tree(
    State(state): Shared,
    Path((_o, _r, sha)): Path<(String, String, String)>,
) -> Response {
    let Ok(git) = state.lock() else {
        return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
    };
    // The engine asks by commit sha; git resolves that to the commit's tree.
    let tree_sha = match git.commits.get(&sha) {
        Some((tree, _)) => tree.clone(),
        None => sha.clone(),
    };
    let Some(tree) = git.trees.get(&tree_sha) else {
        return (
            axum::http::StatusCode::NOT_FOUND,
            axum::Json(json!({ "message": "Not Found" })),
        )
            .into_response();
    };
    let entries: Vec<Value> = tree
        .iter()
        .map(|(path, blob)| json!({ "path": path, "mode": "100644", "type": "blob", "sha": blob }))
        .collect();
    axum::Json(json!({ "sha": tree_sha, "tree": entries, "truncated": false })).into_response()
}

async fn get_blob(
    State(state): Shared,
    Path((_o, _r, sha)): Path<(String, String, String)>,
) -> Response {
    let Ok(git) = state.lock() else {
        return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
    };
    match git.blobs.get(&sha) {
        Some(content) => axum::Json(json!({
            "content": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, content),
            "encoding": "base64",
        }))
        .into_response(),
        None => (
            axum::http::StatusCode::NOT_FOUND,
            axum::Json(json!({ "message": "Not Found" })),
        )
            .into_response(),
    }
}

async fn post_blob(State(state): Shared, axum::Json(body): axum::Json<Value>) -> Response {
    let encoded = body["content"].as_str().unwrap_or_default();
    let content = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, encoded)
        .unwrap_or_default();
    let sha = blob_sha(&content);
    if let Ok(mut git) = state.lock() {
        git.blobs.insert(sha.clone(), content);
    }
    axum::Json(json!({ "sha": sha })).into_response()
}

async fn post_tree(State(state): Shared, axum::Json(body): axum::Json<Value>) -> Response {
    let mut tree = BTreeMap::new();
    for entry in body["tree"].as_array().cloned().unwrap_or_default() {
        let path = entry["path"].as_str().unwrap_or_default().to_string();
        let sha = entry["sha"].as_str().unwrap_or_default().to_string();
        tree.insert(path, sha);
    }
    let sha = digest("tree", &format!("{:?}", tree));
    if let Ok(mut git) = state.lock() {
        git.trees.insert(sha.clone(), tree);
    }
    axum::Json(json!({ "sha": sha })).into_response()
}

async fn post_commit(State(state): Shared, axum::Json(body): axum::Json<Value>) -> Response {
    let tree = body["tree"].as_str().unwrap_or_default().to_string();
    let parents: Vec<String> = body["parents"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|p| p.as_str().map(str::to_string))
        .collect();
    let sha = digest("commit", &format!("{}{:?}", tree, parents));
    if let Ok(mut git) = state.lock() {
        git.commits.insert(sha.clone(), (tree, parents));
    }
    axum::Json(json!({ "sha": sha })).into_response()
}

async fn patch_ref(
    State(state): Shared,
    Path((_o, _r, branch)): Path<(String, String, String)>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    let sha = body["sha"].as_str().unwrap_or_default().to_string();
    if let Ok(mut git) = state.lock() {
        git.branches.insert(branch, sha.clone());
    }
    axum::Json(json!({ "object": { "sha": sha } })).into_response()
}

async fn tarball(
    State(state): Shared,
    Path((owner, repo, sha)): Path<(String, String, String)>,
) -> Response {
    use std::io::Write;
    let Ok(git) = state.lock() else {
        return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
    };
    let Some((tree_sha, _)) = git.commits.get(&sha) else {
        return (axum::http::StatusCode::NOT_FOUND, "").into_response();
    };
    let Some(tree) = git.trees.get(tree_sha) else {
        return (axum::http::StatusCode::NOT_FOUND, "").into_response();
    };

    let prefix = format!("{}-{}-{}", owner, repo, &sha[..7.min(sha.len())]);
    let mut builder = tar::Builder::new(Vec::new());
    for (path, blob) in tree {
        let empty = Vec::new();
        let content = git.blobs.get(blob).unwrap_or(&empty);
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(0o644);
        header.set_entry_type(tar::EntryType::Regular);
        header.set_cksum();
        builder
            .append_data(&mut header, format!("{}/{}", prefix, path), &content[..])
            .expect("append");
    }
    let tar_bytes = builder.into_inner().expect("finish tar");
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&tar_bytes).expect("gzip");
    (
        [(axum::http::header::CONTENT_TYPE, "application/gzip")],
        encoder.finish().expect("finish gzip"),
    )
        .into_response()
}

// ============================================================================
// Helpers
// ============================================================================

fn pusher() -> UserContext {
    UserContext::admin("git-pusher".to_string())
}

fn pull_request(prefix: &str) -> PullRequest {
    PullRequest {
        repo: "lpajunen/solution".to_string(),
        branch: None,
        prefix: Some(prefix.to_string()),
        force: false,
    }
}

fn push_request(script_uri: &str) -> PushRequest {
    PushRequest {
        script_uri: script_uri.to_string(),
        repo: None,
        branch: None,
        message: None,
        force: false,
    }
}

fn script_uri(suffix: &str) -> String {
    if aiwebengine::hosts::is_configured() {
        format!(
            "{}/{}",
            aiwebengine::hosts::origin(&aiwebengine::hosts::default_host()),
            suffix
        )
    } else {
        suffix.to_string()
    }
}

/// Edit a script the way a caller does, through the authorized write.
///
/// Not `repository::upsert_script`, which stores the row and records no
/// revision — and the divergence check asks whether the script has moved since
/// the sync, which is a question about revisions.
fn edit(script_uri: &str, content: &str) {
    aiwebengine::engine_api::upsert_script_authorized(&pusher(), script_uri, content, Some("test"))
        .expect("script should store");
}

fn clear(script_uri: &str) {
    for existing in repository::fetch_assets(script_uri).keys() {
        repository::delete_asset(script_uri, existing);
    }
}

// ============================================================================
// Tests
// ============================================================================

/// The round trip the whole feature exists for: pull it, change it here, put it
/// back.
#[tokio::test(flavor = "multi_thread")]
async fn a_local_change_lands_in_the_repository() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = Fixture::start(&[
        ("shop/main.js", "function init() {}"),
        ("shop/lib/cart.js", "export const cart = [];"),
    ])
    .await
    .expect("fixture should start");

    let uri = &script_uri("push-round/shop.js");
    clear(uri);

    pull_with(github.client(), &pusher(), pull_request("push-round"))
        .await
        .expect("pull should succeed");

    // Edit here, the way an agent would.
    edit(uri, "function init() { /* edited */ }");

    let report = push_with(github.client(), &pusher(), push_request(uri))
        .await
        .expect("push should succeed");

    assert!(!report.up_to_date);
    assert_eq!(report.written, vec!["shop/main.js".to_string()]);

    let repo = github.snapshot();
    assert_eq!(
        repo.get("shop/main.js").map(String::as_str),
        Some("function init() { /* edited */ }"),
        "the edit reached the repository"
    );
    assert_eq!(
        repo.get("shop/lib/cart.js").map(String::as_str),
        Some("export const cart = [];"),
        "and the module nobody touched is unchanged"
    );
}

/// The property that keeps a push from being destructive. Everything the script
/// does not own — repository furniture, CI configuration, whatever the ignore
/// file excludes — is invisible to the engine and must survive anyway.
#[tokio::test(flavor = "multi_thread")]
async fn files_the_script_does_not_own_survive() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = Fixture::start(&[
        ("shop/main.js", "function init() {}"),
        ("shop/docs/guide.md", "long explanation"),
        ("README.md", "repository furniture"),
        (".github/workflows/ci.yml", "on: push"),
        (".aiwebengineignore", "docs/\n"),
    ])
    .await
    .expect("fixture should start");

    let uri = &script_uri("push-keep/shop.js");
    clear(uri);

    pull_with(github.client(), &pusher(), pull_request("push-keep"))
        .await
        .expect("pull should succeed");
    edit(uri, "function init() { /* v2 */ }");

    push_with(github.client(), &pusher(), push_request(uri))
        .await
        .expect("push should succeed");

    let repo = github.snapshot();
    for kept in [
        "README.md",
        ".github/workflows/ci.yml",
        ".aiwebengineignore",
        "shop/docs/guide.md",
    ] {
        assert!(
            repo.contains_key(kept),
            "{} must survive a push; the repository holds {:?}",
            kept,
            repo.keys().collect::<Vec<_>>()
        );
    }
}

/// A module removed here is removed there. The mirror of a pull applying an
/// upstream deletion.
#[tokio::test(flavor = "multi_thread")]
async fn a_module_deleted_here_is_deleted_there() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = Fixture::start(&[
        ("shop/main.js", "function init() {}"),
        ("shop/lib/keep.js", "keep"),
        ("shop/lib/drop.js", "drop"),
    ])
    .await
    .expect("fixture should start");

    let uri = &script_uri("push-delete/shop.js");
    clear(uri);

    pull_with(github.client(), &pusher(), pull_request("push-delete"))
        .await
        .expect("pull should succeed");

    assert!(repository::delete_asset(uri, "lib/drop.js"), "removed here");

    let report = push_with(github.client(), &pusher(), push_request(uri))
        .await
        .expect("push should succeed");

    assert_eq!(report.removed, vec!["shop/lib/drop.js".to_string()]);
    let repo = github.snapshot();
    assert!(!repo.contains_key("shop/lib/drop.js"));
    assert!(repo.contains_key("shop/lib/keep.js"));
}

#[tokio::test(flavor = "multi_thread")]
async fn pushing_an_unchanged_script_writes_no_commit() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = Fixture::start(&[("shop/main.js", "function init() {}")])
        .await
        .expect("fixture should start");

    let uri = &script_uri("push-noop/shop.js");
    clear(uri);

    pull_with(github.client(), &pusher(), pull_request("push-noop"))
        .await
        .expect("pull should succeed");

    let before = github.snapshot();
    let report = push_with(github.client(), &pusher(), push_request(uri))
        .await
        .expect("push should succeed");

    assert!(report.up_to_date, "nothing changed, so nothing was written");
    assert!(report.written.is_empty());
    assert_eq!(github.snapshot(), before, "the repository is untouched");
}

/// The engine implements no merge. When both sides have moved it says so and
/// stops, and the message carries what a person needs to reconcile them.
#[tokio::test(flavor = "multi_thread")]
async fn both_sides_moving_is_refused_rather_than_merged() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = Fixture::start(&[("shop/main.js", "function init() {}")])
        .await
        .expect("fixture should start");

    let uri = &script_uri("push-diverge/shop.js");
    clear(uri);

    pull_with(github.client(), &pusher(), pull_request("push-diverge"))
        .await
        .expect("pull should succeed");

    // Somebody else pushes.
    github.land_commit(&[("shop/main.js", "function init() { /* theirs */ }")]);
    // And we change it here.
    edit(uri, "function init() { /* ours */ }");

    let error = push_with(github.client(), &pusher(), push_request(uri))
        .await
        .expect_err("a diverged push should be refused");

    let message = error.to_string();
    assert!(
        message.contains("Both sides have moved"),
        "unexpected: {}",
        message
    );
    assert!(
        message.contains("revisions/diff"),
        "the refusal should point at what reconciles them: {}",
        message
    );

    assert_eq!(
        github.snapshot().get("shop/main.js").map(String::as_str),
        Some("function init() { /* theirs */ }"),
        "and nothing was written over their work"
    );
}

/// A remote that moved while this engine did not is not a conflict — it is a
/// pull waiting to happen, and saying so is more useful than a merge prompt.
#[tokio::test(flavor = "multi_thread")]
async fn a_moved_remote_alone_asks_for_a_pull() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = Fixture::start(&[("shop/main.js", "function init() {}")])
        .await
        .expect("fixture should start");

    let uri = &script_uri("push-behind/shop.js");
    clear(uri);

    pull_with(github.client(), &pusher(), pull_request("push-behind"))
        .await
        .expect("pull should succeed");

    github.land_commit(&[("shop/main.js", "function init() { /* theirs */ }")]);

    let error = push_with(github.client(), &pusher(), push_request(uri))
        .await
        .expect_err("should be refused");

    assert!(
        error.to_string().contains("Pull first"),
        "unexpected: {}",
        error
    );
}

/// `force` skips this engine's divergence check, which is the only check it can
/// skip — GitHub's refusal to fast-forward a ref is not ours to waive.
#[tokio::test(flavor = "multi_thread")]
async fn force_publishes_over_a_divergence() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = Fixture::start(&[("shop/main.js", "function init() {}")])
        .await
        .expect("fixture should start");

    let uri = &script_uri("push-force/shop.js");
    clear(uri);

    pull_with(github.client(), &pusher(), pull_request("push-force"))
        .await
        .expect("pull should succeed");

    github.land_commit(&[("shop/main.js", "function init() { /* theirs */ }")]);
    edit(uri, "function init() { /* ours */ }");

    let forced = PushRequest {
        force: true,
        ..push_request(uri)
    };
    push_with(github.client(), &pusher(), forced)
        .await
        .expect("forced push should succeed");

    assert_eq!(
        github.snapshot().get("shop/main.js").map(String::as_str),
        Some("function init() { /* ours */ }")
    );
}

/// A script nobody has pulled has nowhere to go, and the refusal says so rather
/// than guessing a repository.
#[tokio::test(flavor = "multi_thread")]
async fn a_script_with_no_repository_is_refused() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = Fixture::start(&[("shop/main.js", "function init() {}")])
        .await
        .expect("fixture should start");

    let uri = "push-unbound/loose.js";
    edit(uri, "function init() {}");

    let error = push_with(github.client(), &pusher(), push_request(uri))
        .await
        .expect_err("should be refused");

    assert!(
        error.to_string().contains("nowhere to push it"),
        "unexpected: {}",
        error
    );
}

// ============================================================================
// Status
// ============================================================================

/// The four cells of the sync matrix, each reached the way it happens in
/// practice. This is what an agent asks instead of attempting an operation and
/// reading the refusal, so the mapping from "what has moved" to "what to do"
/// is the thing worth pinning down.
#[tokio::test(flavor = "multi_thread")]
async fn status_reports_which_cell_a_script_is_in() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = Fixture::start(&[("shop/main.js", "function init() {}")])
        .await
        .expect("fixture should start");

    let uri = &script_uri("status-matrix/shop.js");
    clear(uri);

    pull_with(github.client(), &pusher(), pull_request("status-matrix"))
        .await
        .expect("pull should succeed");

    // Neither side has moved.
    let settled = status_with(github.client(), uri)
        .await
        .expect("status should answer");
    assert_eq!(settled.state, SyncState::InSync);
    assert_eq!(settled.remote.as_deref(), Some("lpajunen/solution"));
    assert_eq!(settled.branch.as_deref(), Some("main"));

    // We change it here.
    edit(uri, "function init() { /* ours */ }");
    let ahead = status_with(github.client(), uri)
        .await
        .expect("status should answer");
    assert_eq!(ahead.state, SyncState::Ahead);
    assert!(
        ahead.state.advice().contains("Push"),
        "unexpected: {}",
        ahead.state.advice()
    );

    // And they change it too.
    github.land_commit(&[("shop/main.js", "function init() { /* theirs */ }")]);
    let diverged = status_with(github.client(), uri)
        .await
        .expect("status should answer");
    assert_eq!(diverged.state, SyncState::Diverged);
    assert_ne!(diverged.remote_commit, diverged.commit_at_sync);
    assert_ne!(diverged.revision, diverged.revision_at_sync);
}

/// A remote that moved on its own is a pull waiting to happen, and status says
/// so before anything is attempted.
#[tokio::test(flavor = "multi_thread")]
async fn status_reports_behind_when_only_the_remote_moved() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = Fixture::start(&[("shop/main.js", "function init() {}")])
        .await
        .expect("fixture should start");

    let uri = &script_uri("status-behind/shop.js");
    clear(uri);

    pull_with(github.client(), &pusher(), pull_request("status-behind"))
        .await
        .expect("pull should succeed");

    github.land_commit(&[("shop/main.js", "function init() { /* theirs */ }")]);

    let behind = status_with(github.client(), uri)
        .await
        .expect("status should answer");
    assert_eq!(behind.state, SyncState::Behind);
    assert!(behind.state.advice().contains("Pull"));
}

/// A script nobody has synced is not an error and not a conflict — it has
/// simply never been anywhere, and the answer says what would change that.
#[tokio::test(flavor = "multi_thread")]
async fn status_reports_a_script_that_came_from_nowhere() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = Fixture::start(&[("shop/main.js", "function init() {}")])
        .await
        .expect("fixture should start");

    let uri = "status-unbound/loose.js";
    edit(uri, "function init() {}");

    let status = status_with(github.client(), uri)
        .await
        .expect("status should answer");
    assert_eq!(status.state, SyncState::Unbound);
    assert_eq!(status.remote, None);
    assert!(status.state.advice().contains("Push it"));
}

/// The remote half is the part that can fail for reasons unrelated to the
/// script, so it degrades rather than taking the whole answer down with it.
#[tokio::test(flavor = "multi_thread")]
async fn status_still_answers_when_the_host_cannot_be_reached() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = Fixture::start(&[("shop/main.js", "function init() {}")])
        .await
        .expect("fixture should start");

    let uri = &script_uri("status-offline/shop.js");
    clear(uri);

    pull_with(github.client(), &pusher(), pull_request("status-offline"))
        .await
        .expect("pull should succeed");

    // A client pointed at nothing listening.
    let offline = GitHubClient::for_tests("http://127.0.0.1:1").expect("client");
    let status = status_with(offline, uri)
        .await
        .expect("status should still answer");

    assert_eq!(status.state, SyncState::Unreachable);
    assert!(status.unreachable.is_some(), "and says why");
    assert!(
        status.revision.is_some(),
        "while still reporting the half it does know"
    );
    assert_eq!(status.remote.as_deref(), Some("lpajunen/solution"));
}

/// A pinned script serves its pin, so a pull advancing head changes nothing
/// about what answers requests. Status reports the pin for that reason.
#[tokio::test(flavor = "multi_thread")]
async fn status_reports_a_deployment_pin() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = Fixture::start(&[("shop/main.js", "function init() {}")])
        .await
        .expect("fixture should start");

    let uri = &script_uri("status-pinned/shop.js");
    clear(uri);

    pull_with(github.client(), &pusher(), pull_request("status-pinned"))
        .await
        .expect("pull should succeed");

    let revision = aiwebengine::revisions::current(uri).expect("a revision");
    aiwebengine::deployments::deploy(uri, revision, Some("git-pusher"))
        .await
        .expect("should pin");

    let status = status_with(github.client(), uri)
        .await
        .expect("status should answer");
    assert_eq!(status.pinned, Some(revision));
}
