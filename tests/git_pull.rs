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
    /// The commit the default branch points at. Changing it is this fixture's
    /// equivalent of somebody pushing.
    commit: String,
    /// Other branches, by name, to the commit each points at.
    branches: BTreeMap<String, String>,
    /// Tags, by name, to what the ref names: a commit for a lightweight tag,
    /// a tag object for an annotated one.
    tags: BTreeMap<String, String>,
    /// Annotated tag objects, by sha, to the commit each tags.
    tag_objects: BTreeMap<String, String>,
    /// Every commit's tree: repository path to content.
    trees: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Clone)]
struct FakeGitHub {
    base: String,
    state: Arc<Mutex<FakeState>>,
}

fn tree_of(tree: &[(&str, &str)]) -> BTreeMap<String, String> {
    tree.iter()
        .map(|(path, content)| (path.to_string(), content.to_string()))
        .collect()
}

impl FakeGitHub {
    async fn start(tree: &[(&str, &str)]) -> anyhow::Result<Self> {
        let commit = "1111111111111111111111111111111111111111".to_string();
        let state = Arc::new(Mutex::new(FakeState {
            default_branch: "main".to_string(),
            commit: commit.clone(),
            branches: BTreeMap::new(),
            tags: BTreeMap::new(),
            tag_objects: BTreeMap::new(),
            trees: BTreeMap::from([(commit, tree_of(tree))]),
        }));

        let app = axum::Router::new()
            .route("/repos/{owner}/{repo}", axum::routing::get(repo_info))
            .route(
                "/repos/{owner}/{repo}/git/ref/heads/{*branch}",
                axum::routing::get(git_ref),
            )
            .route(
                "/repos/{owner}/{repo}/git/ref/tags/{*tag}",
                axum::routing::get(tag_ref),
            )
            .route(
                "/repos/{owner}/{repo}/git/tags/{sha}",
                axum::routing::get(tag_object),
            )
            .route(
                "/repos/{owner}/{repo}/git/commits/{sha}",
                axum::routing::get(commit_object),
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

    /// Replace the tree and move the default branch, the way a push would.
    fn push(&self, commit: &str, tree: &[(&str, &str)]) {
        let mut state = self.state.lock().expect("fixture state");
        state.commit = commit.to_string();
        state.trees.insert(commit.to_string(), tree_of(tree));
    }

    /// Point another branch at a new commit holding `tree`.
    fn push_branch(&self, branch: &str, commit: &str, tree: &[(&str, &str)]) {
        let mut state = self.state.lock().expect("fixture state");
        state
            .branches
            .insert(branch.to_string(), commit.to_string());
        state.trees.insert(commit.to_string(), tree_of(tree));
    }

    /// Tag what the default branch points at now, lightweight.
    fn tag(&self, name: &str) {
        let mut state = self.state.lock().expect("fixture state");
        let commit = state.commit.clone();
        state.tags.insert(name.to_string(), commit);
    }

    /// Tag what the default branch points at now with an annotated tag, whose
    /// ref names a tag object rather than the commit.
    fn annotated_tag(&self, name: &str, object: &str) {
        let mut state = self.state.lock().expect("fixture state");
        let commit = state.commit.clone();
        state.tag_objects.insert(object.to_string(), commit);
        state.tags.insert(name.to_string(), object.to_string());
    }
}

async fn repo_info(State(state): State<Arc<Mutex<FakeState>>>) -> Response {
    let branch = state
        .lock()
        .map(|s| s.default_branch.clone())
        .unwrap_or_default();
    axum::Json(json!({ "default_branch": branch })).into_response()
}

fn not_found() -> Response {
    (axum::http::StatusCode::NOT_FOUND, "").into_response()
}

async fn git_ref(
    State(state): State<Arc<Mutex<FakeState>>>,
    Path((_owner, _repo, branch)): Path<(String, String, String)>,
) -> Response {
    let Ok(state) = state.lock() else {
        return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
    };
    let commit = if branch == state.default_branch {
        Some(state.commit.clone())
    } else {
        state.branches.get(&branch).cloned()
    };
    match commit {
        Some(sha) => {
            axum::Json(json!({ "object": { "sha": sha, "type": "commit" } })).into_response()
        }
        None => not_found(),
    }
}

async fn tag_ref(
    State(state): State<Arc<Mutex<FakeState>>>,
    Path((_owner, _repo, tag)): Path<(String, String, String)>,
) -> Response {
    let Ok(state) = state.lock() else {
        return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
    };
    match state.tags.get(&tag) {
        Some(sha) => {
            let kind = if state.tag_objects.contains_key(sha) {
                "tag"
            } else {
                "commit"
            };
            axum::Json(json!({ "object": { "sha": sha, "type": kind } })).into_response()
        }
        None => not_found(),
    }
}

async fn tag_object(
    State(state): State<Arc<Mutex<FakeState>>>,
    Path((_owner, _repo, sha)): Path<(String, String, String)>,
) -> Response {
    let Ok(state) = state.lock() else {
        return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
    };
    match state.tag_objects.get(&sha) {
        Some(commit) => {
            axum::Json(json!({ "sha": sha, "object": { "sha": commit, "type": "commit" } }))
                .into_response()
        }
        None => not_found(),
    }
}

async fn commit_object(
    State(state): State<Arc<Mutex<FakeState>>>,
    Path((_owner, _repo, sha)): Path<(String, String, String)>,
) -> Response {
    let Ok(state) = state.lock() else {
        return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
    };
    if state.trees.contains_key(&sha) {
        axum::Json(json!({ "sha": sha })).into_response()
    } else {
        not_found()
    }
}

async fn tarball(
    State(state): State<Arc<Mutex<FakeState>>>,
    Path((owner, repo, sha)): Path<(String, String, String)>,
) -> Response {
    let Ok(state) = state.lock() else {
        return (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "").into_response();
    };
    let Some(tree) = state.trees.get(&sha) else {
        return not_found();
    };

    // GitHub wraps every archive in one `{owner}-{repo}-{short sha}` directory,
    // and stripping it is part of what is under test.
    let prefix = format!("{}-{}-{}", owner, repo, &sha[..7.min(sha.len())]);
    let entries: Vec<(String, String)> = tree
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
        git_ref: None,
        prefix: Some(prefix.to_string()),
        force: false,
        scripts: None,
    }
}

/// The URI a pull composes for `suffix`, against whatever origin this test
/// server came up on. The mapping decides the suffix; the engine decides the
/// origin, and a test that hard-coded either would be testing the wrong one.
fn script_uri(name: &str) -> String {
    name.to_string()
}

fn asset_text(script_uri: &str, asset_uri: &str) -> Option<String> {
    repository::fetch_asset(script_uri, asset_uri)
        .and_then(|asset| String::from_utf8(asset.content).ok())
}

/// Remove whatever an earlier run of the same test left behind, so a test that
/// asserts on deletion is asserting on this run's deletion.
fn clear(script_uri: &str) {
    // Everything but the entrypoint. A script's root module is a file of its
    // tree now, so clearing the tree wholesale would clear the script's source
    // with it and leave nothing to run.
    for existing in repository::fetch_assets(script_uri).keys() {
        if aiwebengine::module_loader::is_root_module_name(existing) {
            continue;
        }
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

    let uri = &script_uri("git-one");
    clear(uri);

    let report = pull_with(github.client(), &puller(), request("git-one"))
        .await
        .expect("pull should succeed");

    assert_eq!(report.git_ref.stored(), "refs/heads/main");
    assert_eq!(report.scripts.len(), 1, "one script expected");
    assert_eq!(&report.scripts[0].script_uri, uri);

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
    // The entry is a file of the tree under the same name the repository gave
    // it, which is what stops a pull having to split a directory into a script
    // row and an asset set and a push having to put them back together.
    assert_eq!(
        asset_text(uri, "main.ts").as_deref(),
        Some("function init() {}"),
        "the entry is a file of the tree, named as the repository named it"
    );
    assert_eq!(
        repository::fetch_script(uri).as_deref(),
        Some("function init() {}"),
        "and it is what the script runs"
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

    clear(&script_uri("git-many-shop"));
    clear(&script_uri("git-many-admin"));

    let report = pull_with(github.client(), &puller(), request("git-many"))
        .await
        .expect("pull should succeed");

    let mut uris: Vec<String> = report
        .scripts
        .iter()
        .map(|script| script.script_uri.clone())
        .collect();
    uris.sort();
    assert_eq!(
        uris,
        vec![script_uri("git-many-admin"), script_uri("git-many-shop")]
    );

    assert_eq!(
        asset_text(&script_uri("git-many-shop"), "lib/cart.ts").as_deref(),
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

    clear(&script_uri("git-idempotent"));

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

    let uri = &script_uri("git-sync");
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

    let uri = &script_uri("git-revision");
    clear(uri);

    let before = aiwebengine::revisions::current(uri).unwrap_or(0);
    let report = pull_with(github.client(), &puller(), request("git-revision"))
        .await
        .expect("pull should succeed");
    let after = aiwebengine::revisions::current(uri).unwrap_or(0);

    // Four, not three: the entrypoint is a file of the tree like the modules
    // beside it, so a pull writes it through the same path and counts it the
    // same way.
    assert_eq!(report.scripts[0].written, 4, "four files written");
    assert_eq!(
        after - before,
        1,
        "four files are one change, so they are one revision"
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
            git_ref: Some("no-such-branch".to_string()),
            prefix: Some("git-branch".to_string()),
            force: false,
            scripts: None,
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

    let uri = &script_uri("git-blocking");
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
    assert_eq!(&report.scripts[0].script_uri, uri);
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

    let uri = &script_uri("git-ignore");
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

/// `force` exists because the up-to-date check can only answer the questions
/// the engine knows to ask, and clearing it should not mean deleting rows.
#[tokio::test(flavor = "multi_thread")]
async fn force_re_applies_a_repository_that_has_not_moved() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(&[("main.ts", "function init() {}")])
        .await
        .expect("fixture should start");

    let uri = &script_uri("git-force");
    clear(uri);

    pull_with(github.client(), &puller(), request("git-force"))
        .await
        .expect("first pull should succeed");

    let skipped = pull_with(github.client(), &puller(), request("git-force"))
        .await
        .expect("second pull should succeed");
    assert!(skipped.up_to_date, "unchanged, so the shortcut applies");

    let forced = pull_with(
        github.client(),
        &puller(),
        PullRequest {
            repo: "lpajunen/solution".to_string(),
            git_ref: None,
            prefix: Some("git-force".to_string()),
            force: true,
            scripts: None,
        },
    )
    .await
    .expect("forced pull should succeed");

    assert!(!forced.up_to_date, "force does the work regardless");
    assert_eq!(forced.scripts.len(), 1);
    // The content is identical, so re-applying it writes nothing and records no
    // revision — forcing the work is not the same as inventing a change.
    assert!(
        !forced.scripts[0].changed,
        "identical content changes nothing"
    );
    assert_eq!(forced.scripts[0].revision, None);
}

/// A pull that wrote nothing still says the two now agree.
///
/// `record_sync` keeps the previous watermark when a pull records no revision,
/// so the engine went on reporting `ahead` after a pull had just confirmed the
/// content matches. Invisible while only the engine API wrote assets — every
/// write moved head, and the next pull moved the watermark with it. A script
/// writing its own asset records a revision too, so head can move for reasons
/// the repository knows nothing about: write a skill, delete it again, and the
/// content agrees while the number does not.
///
/// What the pull *wrote* is still reported as nothing, which is the other half
/// of this: forcing the work is not the same as inventing a change.
#[tokio::test(flavor = "multi_thread")]
async fn a_pull_that_writes_nothing_still_moves_the_watermark() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(&[("main.ts", "function init() {}")])
        .await
        .expect("fixture should start");

    let uri = &script_uri("git-watermark");
    clear(uri);

    pull_with(github.client(), &puller(), request("git-watermark"))
        .await
        .expect("first pull should succeed");

    // Head moves for a reason the repository has nothing to do with, the way
    // an agent writing itself a skill and thinking better of it would. Done
    // through the asset API rather than by running a snippet, because
    // `execute_script_secure` stores the source it runs — which would change
    // the root as well and make the forced pull a real change rather than the
    // no-op this test is about.
    let encoded = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode("scratch")
    };
    let admin = UserContext::admin("watermark".to_string());
    tokio::task::spawn_blocking({
        let (admin, uri, encoded) = (admin.clone(), uri.to_string(), encoded.clone());
        move || {
            aiwebengine::engine_api::upsert_asset_authorized(
                &admin,
                &uri,
                "scratch.txt",
                "text/plain",
                &encoded,
            )
        }
    })
    .await
    .expect("no panic")
    .expect("the write should land");

    let drifted = aiwebengine::git_sync::status_with(github.client(), uri)
        .await
        .expect("status should read");
    assert_eq!(
        drifted.state,
        aiwebengine::git_sync::SyncState::Ahead,
        "a write the repository does not have is drift"
    );

    // Put the content back, so the engine and the repository agree again on
    // everything but the revision number.
    tokio::task::spawn_blocking({
        let (admin, uri) = (admin.clone(), uri.to_string());
        move || {
            aiwebengine::engine_api::delete_asset_authorized(&admin, &uri, "scratch.txt")
                .unwrap_or_else(|_| panic!("the delete should be authorized"))
        }
    })
    .await
    .expect("no panic");

    let forced = pull_with(
        github.client(),
        &puller(),
        PullRequest {
            repo: "lpajunen/solution".to_string(),
            git_ref: None,
            prefix: Some("git-watermark".to_string()),
            force: true,
            scripts: None,
        },
    )
    .await
    .expect("forced pull should succeed");

    assert!(
        !forced.scripts[0].changed,
        "identical content changes nothing; got written={} deleted={} unchanged={} revision={:?}",
        forced.scripts[0].written,
        forced.scripts[0].deleted,
        forced.scripts[0].unchanged,
        forced.scripts[0].revision
    );
    assert_eq!(
        forced.scripts[0].revision, None,
        "the report says what the pull wrote, which is nothing"
    );

    let settled = aiwebengine::git_sync::status_with(github.client(), uri)
        .await
        .expect("status should read");
    assert_eq!(
        settled.state,
        aiwebengine::git_sync::SyncState::InSync,
        "a pull that confirmed the content agrees should stop reporting drift"
    );
}

/// The shortcut has to be about this end as well as the remote. A pull that
/// would compose different URIs than the recorded ones is not up to date,
/// however still the repository has been — which is exactly the case that made
/// a pull silently do nothing after the URI composition changed underneath it.
#[tokio::test(flavor = "multi_thread")]
async fn the_same_commit_landing_elsewhere_is_not_up_to_date() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(&[("main.ts", "function init() {}")])
        .await
        .expect("fixture should start");

    clear(&script_uri("git-base-a"));
    clear(&script_uri("git-base-b"));

    pull_with(github.client(), &puller(), request("git-base-a"))
        .await
        .expect("first pull should succeed");

    let elsewhere = pull_with(github.client(), &puller(), request("git-base-b"))
        .await
        .expect("second pull should succeed");

    assert!(
        !elsewhere.up_to_date,
        "the same commit landing somewhere else is work, not a no-op"
    );
    // The repository is already bound to the script the first pull wrote, so
    // a different prefix does not make a second one: the script is found by
    // the directory its row records, which is also what lets it be renamed.
    assert_eq!(elsewhere.scripts[0].script_uri, script_uri("git-base-a"));
}

/// A pulled script is named after where it came from, not after its entry file.
/// Naming every one of them `main.js` is accurate and useless: the point of a
/// URI is to say which script it is.
#[tokio::test(flavor = "multi_thread")]
async fn a_script_is_named_after_its_source() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(&[
        ("shop/main.ts", "function init() {}"),
        ("admin/main.js", "function init() {}"),
    ])
    .await
    .expect("fixture should start");

    clear(&script_uri("git-named-shop"));
    clear(&script_uri("git-named-admin"));

    let report = pull_with(github.client(), &puller(), request("git-named"))
        .await
        .expect("pull should succeed");

    let mut uris: Vec<String> = report
        .scripts
        .iter()
        .map(|script| script.script_uri.clone())
        .collect();
    uris.sort();

    assert_eq!(
        uris,
        vec![
            // The extension follows the entry, because the transpiler reads it
            // and nothing else about the name.
            script_uri("git-named-admin"),
            script_uri("git-named-shop"),
        ]
    );
}

/// A repository that is itself one script takes the repository's name, rather
/// than repeating it as `.../shop/shop.js`.
#[tokio::test(flavor = "multi_thread")]
async fn a_single_script_repository_is_named_after_the_repository() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(&[("main.js", "function init() {}")])
        .await
        .expect("fixture should start");

    let uri = &script_uri("git-solo");
    clear(uri);

    let report = pull_with(github.client(), &puller(), request("git-solo"))
        .await
        .expect("pull should succeed");

    assert_eq!(&report.scripts[0].script_uri, uri);
    assert!(repository::fetch_script(uri).is_some());
}

/// Owning a script is not the same as having agreed that a repository may
/// replace it. The prefix defaults to the repository's name, so a repository
/// named like an existing script's prefix is all it takes to overwrite work
/// written here and never pulled.
#[tokio::test(flavor = "multi_thread")]
async fn a_pull_will_not_overwrite_a_script_it_did_not_write() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(&[("main.js", "function init() { /* theirs */ }")])
        .await
        .expect("fixture should start");

    let uri = &script_uri("git-occupied");
    clear(uri);
    aiwebengine::engine_api::upsert_script_authorized(
        &puller(),
        uri,
        "function init() { /* mine, written here */ }",
        Some("test"),
    )
    .expect("script should store");

    let error = pull_with(github.client(), &puller(), request("git-occupied"))
        .await
        .expect_err("should refuse to clobber");

    assert!(
        error.to_string().contains("would overwrite"),
        "unexpected: {}",
        error
    );
    assert_eq!(
        aiwebengine::repository::fetch_script(uri).as_deref(),
        Some("function init() { /* mine, written here */ }"),
        "and left it alone"
    );

    // Force is how a caller says they meant it.
    let forced = PullRequest {
        force: true,
        ..request("git-occupied")
    };
    pull_with(github.client(), &puller(), forced)
        .await
        .expect("forced pull should succeed");
    assert_eq!(
        aiwebengine::repository::fetch_script(uri).as_deref(),
        Some("function init() { /* theirs */ }")
    );
}

/// A second pull of the same repository is not a clobber — replacing what it
/// wrote is the entire point of pulling again.
#[tokio::test(flavor = "multi_thread")]
async fn a_repeat_pull_is_not_treated_as_an_overwrite() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(&[("main.js", "function init() {}")])
        .await
        .expect("fixture should start");

    let uri = &script_uri("git-repeat");
    clear(uri);

    pull_with(github.client(), &puller(), request("git-repeat"))
        .await
        .expect("first pull should succeed");

    github.push(
        "3333333333333333333333333333333333333333",
        &[("main.js", "function init() { /* v2 */ }")],
    );

    pull_with(github.client(), &puller(), request("git-repeat"))
        .await
        .expect("second pull should succeed without force");

    assert_eq!(
        aiwebengine::repository::fetch_script(uri).as_deref(),
        Some("function init() { /* v2 */ }")
    );
}

/// A binding is what tells a later pull the script is that repository's to
/// replace, so clearing it puts the script back under the clobber protection.
#[tokio::test(flavor = "multi_thread")]
async fn clearing_a_binding_leaves_the_script_and_restores_its_protection() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(&[("main.js", "function init() {}")])
        .await
        .expect("fixture should start");

    let uri = &script_uri("git-unbind");
    clear(uri);

    pull_with(github.client(), &puller(), request("git-unbind"))
        .await
        .expect("pull should succeed");

    let listed = aiwebengine::git_sync::bindings()
        .await
        .expect("should list");
    assert!(
        listed.iter().any(|b| &b.script_uri == uri),
        "the pull recorded a binding"
    );

    assert!(
        aiwebengine::git_sync::unbind(uri)
            .await
            .expect("should clear"),
        "there was one to clear"
    );

    assert!(
        aiwebengine::repository::fetch_script(uri).is_some(),
        "the script itself stays"
    );

    github.push(
        "4444444444444444444444444444444444444444",
        &[("main.js", "function init() { /* v2 */ }")],
    );
    let error = pull_with(github.client(), &puller(), request("git-unbind"))
        .await
        .expect_err("unbound, so no longer theirs to replace");
    assert!(error.to_string().contains("would overwrite"), "{}", error);
}

/// A pulled script is found again by the directory its row records, not by
/// composing its name a second time — so renaming it is not a way to make the
/// next pull write another one beside it.
#[tokio::test(flavor = "multi_thread")]
async fn a_renamed_script_is_still_the_one_the_repository_writes() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(&[("shop/main.ts", "function init() {}")])
        .await
        .expect("fixture should start");

    let original = script_uri("git-renamed-shop");
    let renamed = script_uri("git-renamed-storefront");
    clear(&original);
    clear(&renamed);

    pull_with(github.client(), &puller(), request("git-renamed"))
        .await
        .expect("first pull should succeed");
    assert!(repository::fetch_script(&original).is_some());

    aiwebengine::engine_api::rename_script_authorized(&puller(), &original, &renamed)
        .expect("the owner may rename it");

    let mut again = request("git-renamed");
    again.force = true;
    let report = pull_with(github.client(), &puller(), again)
        .await
        .expect("a pull after a rename should succeed");

    assert_eq!(report.scripts.len(), 1);
    assert_eq!(
        report.scripts[0].script_uri, renamed,
        "the pull wrote the script it already had"
    );
    assert!(
        repository::fetch_script(&original).is_none(),
        "and did not make a second one under the name it would have composed"
    );

    clear(&renamed);
}

// ============================================================================
// Reading a tag or a commit
// ============================================================================

const V1: &str = "1111111111111111111111111111111111111111";
const V2: &str = "2222222222222222222222222222222222222222";

/// A pull of `lpajunen/solution` at `git_ref`, under `prefix`.
fn at(git_ref: &str, prefix: &str) -> PullRequest {
    PullRequest {
        git_ref: Some(git_ref.to_string()),
        ..request(prefix)
    }
}

/// The default branch holds `v1` at [`V1`], tagged `v1`, and then moves on to
/// `v2` at [`V2`].
async fn tagged_then_moved() -> FakeGitHub {
    let github = FakeGitHub::start(&[("main.ts", "// v1")])
        .await
        .expect("fixture should start");
    github.tag("v1");
    github.push(V2, &[("main.ts", "// v2")]);
    github
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tag_holds_still_while_the_branch_moves() {
    let _guard = test_mutex().lock().await;
    setup_env().await;
    let github = tagged_then_moved().await;

    let report = pull_with(github.client(), &puller(), at("v1", "git-tag"))
        .await
        .expect("a tag should pull");
    assert_eq!(report.git_ref.stored(), "refs/tags/v1");
    assert_eq!(report.commit, V1);
    assert_eq!(asset_text("git-tag", "main.ts").as_deref(), Some("// v1"));

    // A pull naming no ref reads what the repository is followed at, not the
    // branch that has moved on.
    let again = pull_with(github.client(), &puller(), request("git-tag"))
        .await
        .expect("a second pull should succeed");
    assert!(again.up_to_date, "the tag has not moved");
    assert_eq!(asset_text("git-tag", "main.ts").as_deref(), Some("// v1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_annotated_tag_reads_the_commit_it_tags() {
    let _guard = test_mutex().lock().await;
    setup_env().await;
    let github = FakeGitHub::start(&[("main.ts", "// v1")])
        .await
        .expect("fixture should start");
    github.annotated_tag("v1", "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    github.push(V2, &[("main.ts", "// v2")]);

    let report = pull_with(github.client(), &puller(), at("v1", "git-annotated"))
        .await
        .expect("an annotated tag should pull");
    assert_eq!(report.commit, V1, "the commit, not the tag object");
    assert_eq!(
        asset_text("git-annotated", "main.ts").as_deref(),
        Some("// v1")
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_full_commit_sha_pulls_that_commit() {
    let _guard = test_mutex().lock().await;
    setup_env().await;
    let github = tagged_then_moved().await;

    let report = pull_with(github.client(), &puller(), at(V1, "git-sha"))
        .await
        .expect("a commit should pull");
    assert_eq!(report.git_ref.stored(), V1);
    assert_eq!(asset_text("git-sha", "main.ts").as_deref(), Some("// v1"));

    let error = pull_with(github.client(), &puller(), at(&V1[..7], "git-sha"))
        .await
        .expect_err("a short sha is a name, and no branch or tag has it");
    assert!(
        error.to_string().contains("full 40-character sha"),
        "{error}"
    );

    let error = pull_with(
        github.client(),
        &puller(),
        at("3333333333333333333333333333333333333333", "git-sha"),
    )
    .await
    .expect_err("a commit the repository lacks");
    assert!(error.to_string().contains("no commit"), "{error}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_name_that_is_both_a_branch_and_a_tag_is_refused() {
    let _guard = test_mutex().lock().await;
    setup_env().await;
    let github = tagged_then_moved().await;
    github.push_branch("v1", V2, &[("main.ts", "// v2")]);

    let error = pull_with(github.client(), &puller(), at("v1", "git-both"))
        .await
        .expect_err("which one is meant is not the engine's to guess");
    assert!(error.to_string().contains("refs/tags/v1"), "{error}");

    let report = pull_with(github.client(), &puller(), at("refs/tags/v1", "git-both"))
        .await
        .expect("spelled out, it pulls");
    assert_eq!(report.commit, V1);
}

/// Upgrading is pulling the next tag: the scripts the first one wrote are the
/// ones written, renamed or not, and the follow moves with them.
#[tokio::test(flavor = "multi_thread")]
async fn pulling_another_ref_moves_the_scripts_and_the_follow() {
    let _guard = test_mutex().lock().await;
    setup_env().await;
    let github = tagged_then_moved().await;
    github.tag("v2");

    pull_with(github.client(), &puller(), at("v1", "git-upgrade"))
        .await
        .expect("v1 should pull");
    aiwebengine::engine_api::rename_script_authorized(&puller(), "git-upgrade", "git-renamed")
        .expect("the owner may rename it");

    // No prefix this time: the follow's is kept.
    let report = pull_with(
        github.client(),
        &puller(),
        PullRequest {
            prefix: None,
            ..at("v2", "")
        },
    )
    .await
    .expect("v2 should pull");
    assert_eq!(report.scripts.len(), 1);
    assert_eq!(report.scripts[0].script_uri, "git-renamed");
    assert_eq!(
        asset_text("git-renamed", "main.ts").as_deref(),
        Some("// v2")
    );
    assert!(repository::fetch_script("git-upgrade").is_none());

    let follow = aiwebengine::git_sync::followed("lpajunen/solution")
        .await
        .expect("follow should read")
        .expect("the repository is followed");
    assert_eq!(follow.git_ref, "refs/tags/v2");
    assert_eq!(follow.uri_base, "git-upgrade");
}

/// The bug a follow per ref had: a schedule left at the old ref pulled it back
/// over the new one on its next run.
#[tokio::test(flavor = "multi_thread")]
async fn a_scheduled_follow_runs_at_the_ref_it_was_moved_to() {
    let _guard = test_mutex().lock().await;
    setup_env().await;
    let github = tagged_then_moved().await;
    github.tag("v2");
    let (_, editor) = an_editor("moved").await;

    git_follow::schedule_with(
        github.client(),
        &editor,
        FollowRequest {
            git_ref: Some("v1".to_string()),
            ..every_ten("lpajunen/moved", "moved", None, Mode::Apply)
        },
    )
    .await
    .expect("an editor may follow a tag");
    let follow = git_follow::get("lpajunen/moved")
        .await
        .expect("follow should read")
        .expect("it is scheduled");
    assert_eq!(follow.git_ref, "refs/tags/v1");
    let ran = git_follow::run(&follow, Some(github.client())).await;
    assert_eq!(ran.state, git_follow::State::Ok, "{}", ran.outcome);
    assert_eq!(asset_text("moved", "main.ts").as_deref(), Some("// v1"));

    let mut upgrade = at("v2", "moved");
    upgrade.repo = "lpajunen/moved".to_string();
    pull_with(github.client(), &editor, upgrade)
        .await
        .expect("the account that runs it may move it");

    let follow = git_follow::get("lpajunen/moved")
        .await
        .expect("follow should read")
        .expect("still scheduled");
    assert_eq!(follow.git_ref, "refs/tags/v2");
    assert!(follow.every_minutes.is_some(), "moving keeps the schedule");
    let ran = git_follow::run(&follow, Some(github.client())).await;
    assert_eq!(ran.state, git_follow::State::Ok, "{}", ran.outcome);
    assert!(ran.outcome.starts_with("Up to date"), "{}", ran.outcome);
    assert_eq!(asset_text("moved", "main.ts").as_deref(), Some("// v2"));
}

#[tokio::test(flavor = "multi_thread")]
async fn another_account_cannot_move_a_scheduled_follow() {
    let _guard = test_mutex().lock().await;
    setup_env().await;
    let github = tagged_then_moved().await;
    github.tag("v2");
    let (_, first) = an_editor("owner").await;
    let (second_id, second) = an_editor("other").await;

    git_follow::schedule_with(
        github.client(),
        &first,
        FollowRequest {
            git_ref: Some("v1".to_string()),
            ..every_ten("lpajunen/held", "held", None, Mode::Apply)
        },
    )
    .await
    .expect("the first account follows v1");
    let follow = git_follow::get("lpajunen/held")
        .await
        .expect("follow should read")
        .expect("it is scheduled");
    git_follow::run(&follow, Some(github.client())).await;
    aiwebengine::repository::add_script_owner("held", &second_id)
        .expect("a second owner can be added");

    let mut moved = at("v2", "held");
    moved.repo = "lpajunen/held".to_string();
    let error = pull_with(github.client(), &second, moved)
        .await
        .expect_err("a co-owner cannot redirect another account's schedule");
    assert!(error.to_string().contains("another account"), "{error}");

    let mut same = at("v1", "held");
    same.repo = "lpajunen/held".to_string();
    pull_with(github.client(), &second, same)
        .await
        .expect("pulling what is followed is not moving it");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_commit_is_pulled_once_rather_than_followed() {
    let _guard = test_mutex().lock().await;
    setup_env().await;
    let github = tagged_then_moved().await;
    let (_, editor) = an_editor("commit").await;

    let error = git_follow::schedule_with(
        github.client(),
        &editor,
        FollowRequest {
            git_ref: Some(V1.to_string()),
            ..every_ten("lpajunen/pinned", "pinned", None, Mode::Apply)
        },
    )
    .await
    .expect_err("a commit never moves");
    assert!(error.to_string().contains("never moves"), "{error}");
}

// ============================================================================
// Taking part of a repository
// ============================================================================

/// A pull of `repo` under `prefix` that takes only the directories named.
fn selecting(repo: &str, prefix: &str, scripts: &[&str]) -> PullRequest {
    PullRequest {
        repo: repo.to_string(),
        git_ref: None,
        prefix: Some(prefix.to_string()),
        force: false,
        scripts: Some(scripts.iter().map(|name| name.to_string()).collect()),
    }
}

/// A pull of `repo` under `prefix` that names no directories.
fn following(repo: &str, prefix: &str) -> PullRequest {
    PullRequest {
        scripts: None,
        ..selecting(repo, prefix, &[])
    }
}

fn pulled(report: &aiwebengine::git_sync::PullReport) -> Vec<&str> {
    let mut names: Vec<&str> = report
        .scripts
        .iter()
        .map(|script| script.script_uri.as_str())
        .collect();
    names.sort_unstable();
    names
}

const THREE_SCRIPTS: &[(&str, &str)] = &[
    ("a/main.ts", "function init() {}"),
    ("b/main.ts", "function init() {}"),
    ("c/main.ts", "function init() {}"),
];

#[tokio::test(flavor = "multi_thread")]
async fn naming_scripts_takes_only_those() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(THREE_SCRIPTS)
        .await
        .expect("fixture should start");

    let report = pull_with(
        github.client(),
        &puller(),
        selecting("lpajunen/named", "named", &["a", "/c/"]),
    )
    .await
    .expect("pull should succeed");

    assert_eq!(pulled(&report), ["named-a", "named-c"]);
    assert!(
        repository::fetch_script("named-b").is_none(),
        "a directory not named is not taken"
    );
}

/// The selection is remembered, so following part of a repository does not
/// turn into following all of it the first time somebody pulls without
/// repeating the names.
#[tokio::test(flavor = "multi_thread")]
async fn a_pull_naming_none_refreshes_only_what_was_named() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(THREE_SCRIPTS)
        .await
        .expect("fixture should start");
    pull_with(
        github.client(),
        &puller(),
        selecting("lpajunen/followed", "followed", &["a"]),
    )
    .await
    .expect("first pull should succeed");

    let unmoved = pull_with(
        github.client(),
        &puller(),
        following("lpajunen/followed", "followed"),
    )
    .await
    .expect("repeat pull should succeed");
    assert!(unmoved.up_to_date, "nothing moved, so nothing to do");

    github.push(
        "2222222222222222222222222222222222222222",
        &[
            ("a/main.ts", "function init() { /* v2 */ }"),
            ("b/main.ts", "function init() { /* v2 */ }"),
            ("d/main.ts", "function init() {}"),
        ],
    );
    let report = pull_with(
        github.client(),
        &puller(),
        following("lpajunen/followed", "followed"),
    )
    .await
    .expect("pull after a push should succeed");

    assert_eq!(pulled(&report), ["followed-a"]);
    assert_eq!(
        asset_text("followed-a", "main.ts").as_deref(),
        Some("function init() { /* v2 */ }")
    );
    assert!(repository::fetch_script("followed-b").is_none());
    assert!(
        repository::fetch_script("followed-d").is_none(),
        "a script added upstream is not pulled in unasked"
    );
}

/// Naming one more directory at a commit the others already stand at still
/// has work to do, and afterwards the binding follows both.
#[tokio::test(flavor = "multi_thread")]
async fn naming_another_script_adds_it_to_what_is_followed() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(THREE_SCRIPTS)
        .await
        .expect("fixture should start");
    pull_with(
        github.client(),
        &puller(),
        selecting("lpajunen/added", "added", &["a"]),
    )
    .await
    .expect("first pull should succeed");

    let report = pull_with(
        github.client(),
        &puller(),
        selecting("lpajunen/added", "added", &["b"]),
    )
    .await
    .expect("naming another should succeed");
    assert!(!report.up_to_date, "b has never been written here");
    assert_eq!(pulled(&report), ["added-b"]);

    github.push(
        "3333333333333333333333333333333333333333",
        &[
            ("a/main.ts", "function init() { /* v2 */ }"),
            ("b/main.ts", "function init() { /* v2 */ }"),
            ("c/main.ts", "function init() { /* v2 */ }"),
        ],
    );
    let report = pull_with(
        github.client(),
        &puller(),
        following("lpajunen/added", "added"),
    )
    .await
    .expect("refresh should succeed");
    assert_eq!(pulled(&report), ["added-a", "added-b"]);
    assert!(repository::fetch_script("added-c").is_none());
}

/// Without a selection, a binding follows the whole repository, and a script
/// added upstream arrives with the next pull.
#[tokio::test(flavor = "multi_thread")]
async fn a_whole_repository_binding_takes_what_is_added_upstream() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(&[("a/main.ts", "function init() {}")])
        .await
        .expect("fixture should start");
    pull_with(
        github.client(),
        &puller(),
        following("lpajunen/whole", "whole"),
    )
    .await
    .expect("first pull should succeed");

    github.push(
        "4444444444444444444444444444444444444444",
        &[
            ("a/main.ts", "function init() {}"),
            ("b/main.ts", "function init() {}"),
        ],
    );
    let report = pull_with(
        github.client(),
        &puller(),
        following("lpajunen/whole", "whole"),
    )
    .await
    .expect("second pull should succeed");
    assert_eq!(pulled(&report), ["whole-a", "whole-b"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn naming_a_directory_the_repository_lacks_is_refused() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(THREE_SCRIPTS)
        .await
        .expect("fixture should start");

    let error = pull_with(
        github.client(),
        &puller(),
        selecting("lpajunen/missing", "missing", &["a", "nope"]),
    )
    .await
    .expect_err("an unknown directory should be refused");

    let message = error.to_string();
    assert!(message.contains("'nope'"), "{message}");
    assert!(
        message.contains("a, b, c"),
        "names what is there: {message}"
    );
    assert!(
        repository::fetch_script("missing-a").is_none(),
        "refused whole, not applied in part"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_single_script_repository_cannot_be_taken_in_part() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(&[
        ("main.ts", "function init() {}"),
        ("a/main.ts", "function init() {}"),
    ])
    .await
    .expect("fixture should start");

    let error = pull_with(
        github.client(),
        &puller(),
        selecting("lpajunen/single", "single", &["a"]),
    )
    .await
    .expect_err("a one-script repository has no directories to choose");
    assert!(error.to_string().contains("one script"), "{error}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_selection_must_name_a_top_level_directory() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(THREE_SCRIPTS)
        .await
        .expect("fixture should start");

    for scripts in [&["a/lib"][..], &[" "][..]] {
        let error = pull_with(
            github.client(),
            &puller(),
            selecting("lpajunen/shape", "shape", scripts),
        )
        .await
        .expect_err("the selection should be refused");
        assert!(
            matches!(error, aiwebengine::git_sync::SyncError::Layout(_)),
            "{error}"
        );
    }
}

/// Write `path` into a script the way a caller does, recording a revision.
async fn write_here(script_uri: &str, path: &str, content: &str) {
    let encoded = {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD.encode(content)
    };
    let (uri, path) = (script_uri.to_string(), path.to_string());
    tokio::task::spawn_blocking(move || {
        aiwebengine::engine_api::upsert_asset_authorized(
            &UserContext::admin("local-editor".to_string()),
            &uri,
            &path,
            "text/plain",
            &encoded,
        )
    })
    .await
    .expect("no panic")
    .expect("the write should land");
}

/// A pull replaces a script's whole tree, so a change made here since the last
/// sync would go without anybody having decided it should.
#[tokio::test(flavor = "multi_thread")]
async fn a_pull_will_not_replace_a_change_made_here() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(&[("a/main.ts", "function init() {}")])
        .await
        .expect("fixture should start");
    pull_with(
        github.client(),
        &puller(),
        following("lpajunen/edited", "edited"),
    )
    .await
    .expect("first pull should succeed");

    write_here("edited-a", "notes.md", "mine").await;
    github.push(
        "5555555555555555555555555555555555555555",
        &[("a/main.ts", "function init() { /* v2 */ }")],
    );

    let error = pull_with(
        github.client(),
        &puller(),
        following("lpajunen/edited", "edited"),
    )
    .await
    .expect_err("a change made here should stop the pull");
    assert!(
        matches!(error, aiwebengine::git_sync::SyncError::ChangedHere { .. }),
        "{error}"
    );
    assert!(error.to_string().contains("notes.md"), "{error}");
    assert_eq!(asset_text("edited-a", "notes.md").as_deref(), Some("mine"));
    assert_eq!(
        asset_text("edited-a", "main.ts").as_deref(),
        Some("function init() {}"),
        "nothing was pulled"
    );

    let mut forced = following("lpajunen/edited", "edited");
    forced.force = true;
    pull_with(github.client(), &puller(), forced)
        .await
        .expect("force replaces it");
    assert!(asset_text("edited-a", "notes.md").is_none());
    assert_eq!(
        asset_text("edited-a", "main.ts").as_deref(),
        Some("function init() { /* v2 */ }")
    );
}

/// Content is what counts: a file written and deleted again changes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_change_undone_here_does_not_stop_a_pull() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(&[("a/main.ts", "function init() {}")])
        .await
        .expect("fixture should start");
    pull_with(
        github.client(),
        &puller(),
        following("lpajunen/undone", "undone"),
    )
    .await
    .expect("first pull should succeed");

    write_here("undone-a", "scratch.txt", "scratch").await;
    tokio::task::spawn_blocking(|| {
        aiwebengine::engine_api::delete_asset_authorized(
            &UserContext::admin("local-editor".to_string()),
            "undone-a",
            "scratch.txt",
        )
        .unwrap_or_else(|_| panic!("the delete should be authorized"))
    })
    .await
    .expect("no panic");

    github.push(
        "6666666666666666666666666666666666666666",
        &[("a/main.ts", "function init() { /* v2 */ }")],
    );
    pull_with(
        github.client(),
        &puller(),
        following("lpajunen/undone", "undone"),
    )
    .await
    .expect("the content is what the last sync left, so the pull goes ahead");
}

/// Naming one script of a whole-repository follow pulls that one now; it is
/// not a request to stop following the rest.
#[tokio::test(flavor = "multi_thread")]
async fn naming_a_script_does_not_narrow_a_whole_follow() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(&[
        ("a/main.ts", "function init() {}"),
        ("b/main.ts", "function init() {}"),
    ])
    .await
    .expect("fixture should start");
    pull_with(
        github.client(),
        &puller(),
        following("lpajunen/wide", "wide"),
    )
    .await
    .expect("first pull should succeed");

    github.push(
        "7777777777777777777777777777777777777777",
        &[
            ("a/main.ts", "function init() { /* v2 */ }"),
            ("b/main.ts", "function init() {}"),
            ("c/main.ts", "function init() {}"),
        ],
    );
    let report = pull_with(
        github.client(),
        &puller(),
        selecting("lpajunen/wide", "wide", &["a"]),
    )
    .await
    .expect("naming one should succeed");
    assert_eq!(pulled(&report), ["wide-a"]);

    let report = pull_with(
        github.client(),
        &puller(),
        following("lpajunen/wide", "wide"),
    )
    .await
    .expect("refresh should succeed");
    assert!(
        pulled(&report).contains(&"wide-c"),
        "still the whole repository: {:?}",
        pulled(&report)
    );
}

/// Clearing one followed script's binding stops following it, rather than
/// leaving every later pull to refuse over a script it no longer owns.
#[tokio::test(flavor = "multi_thread")]
async fn clearing_a_followed_script_stops_following_it() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(THREE_SCRIPTS)
        .await
        .expect("fixture should start");
    pull_with(
        github.client(),
        &puller(),
        selecting("lpajunen/cleared", "cleared", &["a", "b"]),
    )
    .await
    .expect("first pull should succeed");

    aiwebengine::git_sync::unbind("cleared-b")
        .await
        .expect("unbind should succeed");
    assert_eq!(
        aiwebengine::git_sync::followed("lpajunen/cleared")
            .await
            .expect("follow should read")
            .map(|follow| follow.scripts),
        Some(Some(["a".to_string()].into_iter().collect()))
    );

    github.push(
        "8888888888888888888888888888888888888888",
        &[
            ("a/main.ts", "function init() { /* v2 */ }"),
            ("b/main.ts", "function init() { /* v2 */ }"),
            ("c/main.ts", "function init() {}"),
        ],
    );
    let report = pull_with(
        github.client(),
        &puller(),
        following("lpajunen/cleared", "cleared"),
    )
    .await
    .expect("refresh should succeed");
    assert_eq!(pulled(&report), ["cleared-a"]);

    // With nothing left, the follow goes, and the next pull starts over.
    aiwebengine::git_sync::unbind("cleared-a")
        .await
        .expect("unbind should succeed");
    assert_eq!(
        aiwebengine::git_sync::followed("lpajunen/cleared")
            .await
            .expect("follow should read")
            .map(|follow| follow.scripts),
        None
    );
}

// ============================================================================
// Following on a schedule
// ============================================================================

use aiwebengine::git_follow::{self, FollowRequest, Mode};

/// An account holding the editor role, and the context its own session has.
async fn an_editor(label: &str) -> (String, UserContext) {
    let user_id = aiwebengine::user_repository::upsert_internal_user(
        Some(format!("follow test {}", label)),
        "test".to_string(),
        format!("{}-{}", label, uuid::Uuid::new_v4()),
        "localhost".to_string(),
    )
    .await
    .expect("the account should be created");
    aiwebengine::user_repository::add_user_role(
        &user_id,
        aiwebengine::user_repository::UserRole::Editor,
    )
    .expect("the role should be granted");
    let context = UserContext::for_session(aiwebengine::security::SessionRoles {
        user_id: Some(&user_id),
        is_admin: false,
        is_editor: true,
        elevation: None,
        via_token: false,
    });
    (user_id, context)
}

fn every_ten(repo: &str, prefix: &str, scripts: Option<&[&str]>, mode: Mode) -> FollowRequest {
    FollowRequest {
        repo: repo.to_string(),
        git_ref: None,
        prefix: Some(prefix.to_string()),
        scripts: scripts.map(|names| names.iter().map(|name| name.to_string()).collect()),
        every_minutes: 10,
        mode,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_scheduled_follow_pulls_as_the_account_that_set_it() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(THREE_SCRIPTS)
        .await
        .expect("fixture should start");
    let (user_id, editor) = an_editor("apply").await;

    let follow = git_follow::schedule_with(
        github.client(),
        &editor,
        every_ten("lpajunen/sched", "sched", Some(&["a"]), Mode::Apply),
    )
    .await
    .expect("an editor may schedule a follow");
    assert_eq!(follow.acting_user.as_deref(), Some(user_id.as_str()));
    assert!(follow.next_due_at.is_some(), "due straight away");

    let git_follow::Ran { state, outcome, .. } =
        git_follow::run(&follow, Some(github.client())).await;
    let ok = state == git_follow::State::Ok;
    assert!(ok, "{outcome}");
    assert!(outcome.starts_with("Pulled"), "{outcome}");
    assert!(repository::fetch_script("sched-a").is_some());
    assert!(repository::fetch_script("sched-b").is_none());
    assert!(
        aiwebengine::engine_api::can_write_script(&editor, "sched-a"),
        "the script belongs to the account the follow ran as"
    );
}

/// Claiming moves the next run on, so a second claim at the same moment —
/// another instance, or the next tick — finds nothing due.
#[tokio::test(flavor = "multi_thread")]
async fn a_due_follow_is_claimed_once() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(THREE_SCRIPTS)
        .await
        .expect("fixture should start");
    let (_, editor) = an_editor("claim").await;
    git_follow::schedule_with(
        github.client(),
        &editor,
        every_ten("lpajunen/claimed", "claimed", None, Mode::Check),
    )
    .await
    .expect("schedule should succeed");

    let first = git_follow::claim_due().await.expect("claim should run");
    assert_eq!(first.len(), 1);
    let second = git_follow::claim_due().await.expect("claim should run");
    assert!(second.is_empty(), "already claimed until the next interval");
}

/// The next run steps from when this one was due, so a tick that claims it a
/// little late does not push every later run back by a tick; a run missed by
/// more than an interval starts again from now rather than running to catch up.
#[tokio::test(flavor = "multi_thread")]
async fn a_claim_keeps_the_interval_and_a_long_miss_starts_from_now() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(THREE_SCRIPTS)
        .await
        .expect("fixture should start");
    let (_, editor) = an_editor("cadence").await;
    git_follow::schedule_with(
        github.client(),
        &editor,
        every_ten("lpajunen/cadence", "cadence", None, Mode::Check),
    )
    .await
    .expect("schedule should succeed");
    let pool = common::test_pool().await;
    let due_at = |offset: &'static str| {
        let pool = pool.clone();
        async move {
            let row = sqlx::query(
                "UPDATE git_follows SET next_due_at = NOW() - $1::interval \
                 WHERE uri_base = 'cadence' RETURNING next_due_at",
            )
            .bind(offset)
            .fetch_one(&pool)
            .await
            .expect("the follow should exist");
            sqlx::Row::get::<chrono::DateTime<chrono::Utc>, _>(&row, "next_due_at")
        }
    };
    let claimed = || async {
        git_follow::claim_due()
            .await
            .expect("claim should run")
            .into_iter()
            .find(|follow| follow.uri_base == "cadence")
            .expect("the follow is due")
    };

    let was_due = due_at("30 seconds").await;
    let next = claimed().await.next_due_at.expect("still scheduled");
    assert_eq!(next, was_due + chrono::Duration::minutes(10));

    due_at("1 day").await;
    let next = claimed().await.next_due_at.expect("still scheduled");
    assert!(next > chrono::Utc::now() + chrono::Duration::minutes(9));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_follow_writes_only_scripts_its_account_owns() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(THREE_SCRIPTS)
        .await
        .expect("fixture should start");
    // An administrator pulled it first, so the scripts are theirs.
    pull_with(
        github.client(),
        &puller(),
        selecting("lpajunen/theirs", "theirs", &["a"]),
    )
    .await
    .expect("the administrator's pull should succeed");

    let (_, editor) = an_editor("not-owner").await;
    let follow = git_follow::schedule_with(
        github.client(),
        &editor,
        every_ten("lpajunen/theirs", "theirs", None, Mode::Apply),
    )
    .await
    .expect("scheduling decides nothing about ownership yet");

    github.push(
        "9999999999999999999999999999999999999999",
        &[("a/main.ts", "function init() { /* v2 */ }")],
    );
    let git_follow::Ran { state, outcome, .. } =
        git_follow::run(&follow, Some(github.client())).await;
    let ok = state == git_follow::State::Ok;
    assert!(!ok, "{outcome}");
    assert!(outcome.contains("do not own"), "{outcome}");
    assert_eq!(
        asset_text("theirs-a", "main.ts").as_deref(),
        Some("function init() {}")
    );
}

/// Background work acting for somebody holds what its grant says until the
/// grant ends; a follow it set up would outlive that.
#[tokio::test(flavor = "multi_thread")]
async fn only_the_person_can_schedule_a_follow() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(THREE_SCRIPTS)
        .await
        .expect("fixture should start");
    let (_, mut delegated) = an_editor("delegated").await;
    delegated.acting.kind = aiwebengine::security::ActingKind::Delegated;

    let error = git_follow::schedule_with(
        github.client(),
        &delegated,
        every_ten("lpajunen/delegated", "delegated", None, Mode::Apply),
    )
    .await
    .expect_err("a delegated run cannot schedule");
    assert!(
        matches!(error, aiwebengine::git_sync::SyncError::AccessDenied(_)),
        "{error}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn another_account_cannot_take_over_a_follow() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(THREE_SCRIPTS)
        .await
        .expect("fixture should start");
    let (_, first) = an_editor("first").await;
    let (_, second) = an_editor("second").await;
    git_follow::schedule_with(
        github.client(),
        &first,
        every_ten("lpajunen/taken", "taken", None, Mode::Apply),
    )
    .await
    .expect("the first schedule should succeed");

    let error = git_follow::schedule_with(
        github.client(),
        &second,
        every_ten("lpajunen/taken", "taken", None, Mode::Apply),
    )
    .await
    .expect_err("it runs as somebody else");
    assert!(error.to_string().contains("another account"), "{error}");
}

/// Changing what an account may do pauses its follows, as it withdraws its
/// delegations.
#[tokio::test(flavor = "multi_thread")]
async fn a_role_change_pauses_the_accounts_follows() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(THREE_SCRIPTS)
        .await
        .expect("fixture should start");
    let (user_id, editor) = an_editor("demoted").await;
    git_follow::schedule_with(
        github.client(),
        &editor,
        every_ten("lpajunen/demoted", "demoted", None, Mode::Apply),
    )
    .await
    .expect("schedule should succeed");

    aiwebengine::user_repository::remove_user_role(
        &user_id,
        &aiwebengine::user_repository::UserRole::Editor,
    )
    .expect("the role should be removed");

    let follow = git_follow::get("lpajunen/demoted")
        .await
        .expect("follow should read")
        .expect("the follow is kept");
    assert_eq!(follow.every_minutes, None, "no longer scheduled");
    assert_eq!(follow.acting_user, None, "and acts for nobody");
    assert!(
        follow
            .last_outcome
            .as_deref()
            .is_some_and(|outcome| outcome.starts_with("Paused")),
        "{:?}",
        follow.last_outcome
    );
}

/// An account that can no longer write scripts stops the follow at the run,
/// even when nothing paused it beforehand.
#[tokio::test(flavor = "multi_thread")]
async fn a_run_for_an_account_that_cannot_write_pauses_it() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(THREE_SCRIPTS)
        .await
        .expect("fixture should start");
    let (user_id, editor) = an_editor("gone").await;
    let follow = git_follow::schedule_with(
        github.client(),
        &editor,
        every_ten("lpajunen/gone", "gone", None, Mode::Apply),
    )
    .await
    .expect("schedule should succeed");

    aiwebengine::user_repository::delete_user(&user_id).expect("the account should go");

    let git_follow::Ran { state, outcome, .. } =
        git_follow::run(&follow, Some(github.client())).await;
    let ok = state == git_follow::State::Ok;
    assert!(!ok);
    assert!(outcome.contains("no longer exists"), "{outcome}");
    assert!(repository::fetch_script("gone-a").is_none());
}

#[tokio::test(flavor = "multi_thread")]
async fn a_check_follow_reports_and_writes_nothing() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(THREE_SCRIPTS)
        .await
        .expect("fixture should start");
    let (_, editor) = an_editor("check").await;
    let follow = git_follow::schedule_with(
        github.client(),
        &editor,
        every_ten("lpajunen/checked", "checked", Some(&["a"]), Mode::Check),
    )
    .await
    .expect("schedule should succeed");

    let git_follow::Ran { state, outcome, .. } =
        git_follow::run(&follow, Some(github.client())).await;
    let ok = state == git_follow::State::Ok;
    assert!(ok, "{outcome}");
    assert!(outcome.starts_with("Behind"), "{outcome}");
    assert!(
        repository::fetch_script("checked-a").is_none(),
        "nothing written"
    );

    pull_with(
        github.client(),
        &editor,
        following("lpajunen/checked", "checked"),
    )
    .await
    .expect("a pull by hand should succeed");
    let git_follow::Ran { state, outcome, .. } =
        git_follow::run(&follow, Some(github.client())).await;
    let ok = state == git_follow::State::Ok;
    assert!(ok, "{outcome}");
    assert!(outcome.starts_with("Up to date"), "{outcome}");
}

/// A staged pull lands in head while each script keeps serving what it served.
#[tokio::test(flavor = "multi_thread")]
async fn a_stage_follow_pulls_without_going_live() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(THREE_SCRIPTS)
        .await
        .expect("fixture should start");
    let (_, editor) = an_editor("stage").await;
    pull_with(
        github.client(),
        &editor,
        selecting("lpajunen/staged", "staged", &["a"]),
    )
    .await
    .expect("the first pull should succeed");
    let served = aiwebengine::revisions::current("staged-a").expect("a revision");

    let follow = git_follow::schedule_with(
        github.client(),
        &editor,
        every_ten("lpajunen/staged", "staged", None, Mode::Stage),
    )
    .await
    .expect("schedule should succeed");
    github.push(
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        &[("a/main.ts", "function init() { /* v2 */ }")],
    );

    let git_follow::Ran { state, outcome, .. } =
        git_follow::run(&follow, Some(github.client())).await;
    let ok = state == git_follow::State::Ok;
    assert!(ok, "{outcome}");
    assert!(outcome.contains("pinned 1"), "{outcome}");
    assert_eq!(
        aiwebengine::deployments::pinned("staged-a"),
        Some(served),
        "still serving what it served"
    );
    assert_eq!(
        asset_text("staged-a", "main.ts").as_deref(),
        Some("function init() { /* v2 */ }"),
        "the pull is in head, for deploy_script"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_names_a_sensible_interval() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(THREE_SCRIPTS)
        .await
        .expect("fixture should start");
    let (_, editor) = an_editor("interval").await;
    for every_minutes in [1, 9, 7 * 24 * 60 + 1, -5] {
        let mut request = every_ten("lpajunen/interval", "interval", None, Mode::Apply);
        request.every_minutes = every_minutes;
        let error = git_follow::schedule_with(github.client(), &editor, request)
            .await
            .expect_err("out of range");
        assert!(
            matches!(error, aiwebengine::git_sync::SyncError::Layout(_)),
            "{every_minutes}: {error}"
        );
    }
}

/// A repository that names a newer script API than this engine's is refused
/// before anything is written; force takes it anyway.
#[tokio::test(flavor = "multi_thread")]
async fn a_repository_needing_a_newer_engine_is_not_pulled() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(&[
        (
            ".aiwebengine-requires",
            "# what these scripts are written against\n2999-01-01\n",
        ),
        ("a/main.ts", "function init() {}"),
    ])
    .await
    .expect("fixture should start");

    let error = pull_with(
        github.client(),
        &puller(),
        following("lpajunen/newer", "newer"),
    )
    .await
    .expect_err("an engine too old for the repository should refuse it");
    assert!(
        matches!(error, aiwebengine::git_sync::SyncError::Incompatible(_)),
        "{error}"
    );
    assert!(
        error.to_string().contains(aiwebengine::SCRIPT_API),
        "{error}"
    );
    assert!(repository::fetch_script("newer-a").is_none());

    let mut forced = following("lpajunen/newer", "newer");
    forced.force = true;
    pull_with(github.client(), &puller(), forced)
        .await
        .expect("force should take it anyway");
    assert!(repository::fetch_script("newer-a").is_some());
    assert!(
        repository::fetch_asset("newer-a", ".aiwebengine-requires").is_none(),
        "the requirement is the repository's, not one of the script's files"
    );
}

/// A follow refused over a change made here records that it is blocked, says
/// so once in the script's log rather than every run, says so again when it
/// goes through, and a write to the script says what it does to the follow.
#[tokio::test(flavor = "multi_thread")]
async fn a_blocked_follow_says_so_in_the_log_once() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(THREE_SCRIPTS)
        .await
        .expect("fixture should start");
    let (_, editor) = an_editor("blocked").await;
    let scheduled = git_follow::schedule_with(
        github.client(),
        &editor,
        every_ten("lpajunen/blocked", "blocked", Some(&["a"]), Mode::Apply),
    )
    .await
    .expect("schedule should succeed");
    let current = || async {
        git_follow::get(&scheduled.remote)
            .await
            .expect("read should succeed")
            .expect("the follow exists")
    };
    let run_once = || async {
        let follow = current().await;
        let ran = git_follow::run(&follow, Some(github.client())).await;
        git_follow::record(&follow, &ran)
            .await
            .expect("record should succeed");
        ran
    };
    let warnings = || {
        repository::fetch_log_messages("blocked-a")
            .into_iter()
            .filter(|entry| {
                entry.message.contains("Following") && entry.message.contains("stopped")
            })
            .count()
    };

    assert_eq!(run_once().await.state, git_follow::State::Ok);

    let written = tokio::task::spawn_blocking(|| {
        aiwebengine::engine_api::execute_native_mcp_tool(
            "write_file",
            &json!({ "script": "blocked-a", "path": "notes.txt", "content": "mine" }),
            &UserContext::admin("local-editor".to_string()),
        )
    })
    .await
    .expect("the write should not panic")
    .expect("write_file is an operation");
    assert!(written.get("error").is_none(), "{written}");
    assert!(
        written["followed"]
            .as_str()
            .is_some_and(|note| note.contains("push_to_git")),
        "{written}"
    );

    github.push(
        "5555555555555555555555555555555555555555",
        &[("a/main.ts", "function init() { /* moved */ }")],
    );
    let ran = run_once().await;
    assert_eq!(ran.state, git_follow::State::Blocked, "{}", ran.outcome);
    assert_eq!(ran.about, ["blocked-a"]);
    assert_eq!(warnings(), 1);
    assert_eq!(current().await.last_state, Some(git_follow::State::Blocked));

    assert_eq!(run_once().await.state, git_follow::State::Blocked);
    assert_eq!(warnings(), 1, "said once, not every run");

    write_here("blocked-a", "notes.txt", "").await;
    let removed = tokio::task::spawn_blocking(|| {
        aiwebengine::engine_api::execute_native_mcp_tool(
            "delete_file",
            &json!({ "script": "blocked-a", "path": "notes.txt" }),
            &UserContext::admin("local-editor".to_string()),
        )
    })
    .await
    .expect("the delete should not panic")
    .expect("delete_file is an operation");
    assert!(removed.get("error").is_none(), "{removed}");
    let ran = run_once().await;
    assert_eq!(ran.state, git_follow::State::Ok, "{}", ran.outcome);
    assert!(
        repository::fetch_log_messages("blocked-a")
            .iter()
            .any(|entry| entry.message.contains("again")),
        "going through again is said too"
    );
}

/// A file at a path the repository's ignore file excludes is the engine's own:
/// a pull neither counts it as a change made here nor removes it, so a script
/// can keep what it writes for itself and still follow its repository.
#[tokio::test(flavor = "multi_thread")]
async fn files_the_repository_ignores_are_kept_and_never_block_a_pull() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let github = FakeGitHub::start(&[
        (".aiwebengineignore", "a/learned/\n"),
        ("a/main.ts", "function init() {}"),
    ])
    .await
    .expect("fixture should start");
    pull_with(
        github.client(),
        &puller(),
        following("lpajunen/kept", "kept"),
    )
    .await
    .expect("first pull should succeed");

    write_here("kept-a", "learned/refunds/SKILL.md", "written here").await;
    assert!(aiwebengine::git_sync::is_kept_here("kept-a", "learned/refunds/SKILL.md").await);
    assert!(!aiwebengine::git_sync::is_kept_here("kept-a", "main.ts").await);
    let status = aiwebengine::git_sync::status_with(github.client(), "kept-a")
        .await
        .expect("status should read");
    assert_eq!(
        status.state,
        aiwebengine::git_sync::SyncState::InSync,
        "a file the engine keeps is not a change to push"
    );

    github.push(
        "6666666666666666666666666666666666666666",
        &[
            (".aiwebengineignore", "a/learned/\n"),
            ("a/main.ts", "function init() { /* moved */ }"),
        ],
    );
    let report = pull_with(
        github.client(),
        &puller(),
        following("lpajunen/kept", "kept"),
    )
    .await
    .expect("a file the repository ignores does not block the pull");
    assert_eq!(pulled(&report), ["kept-a"]);
    assert_eq!(
        asset_text("kept-a", "main.ts").as_deref(),
        Some("function init() { /* moved */ }")
    );
    assert_eq!(
        asset_text("kept-a", "learned/refunds/SKILL.md").as_deref(),
        Some("written here"),
        "the pull leaves the engine's own file where it is"
    );
}

// ============================================================================
// Installing at startup
// ============================================================================

use aiwebengine::config::GitInstall;
use aiwebengine::git_install::{self, Installed};

fn agent_at(git_ref: Option<&str>) -> GitInstall {
    GitInstall {
        repo: "lpajunen/agent".to_string(),
        git_ref: git_ref.map(str::to_string),
        ..GitInstall::default()
    }
}

/// A fresh engine gets the repository once; a restart leaves it alone, and
/// what it created is administrators' to manage.
#[tokio::test(flavor = "multi_thread")]
async fn a_configured_repository_is_installed_once_and_owned_by_nobody() {
    let _guard = test_mutex().lock().await;
    setup_env().await;
    let github = tagged_then_moved().await;

    let installed = git_install::install_one(&agent_at(Some("v1")), Some(github.client())).await;
    assert_eq!(installed, Installed::Pulled(vec!["agent".to_string()]));
    assert_eq!(asset_text("agent", "main.ts").as_deref(), Some("// v1"));
    assert!(
        repository::get_script_owners("agent")
            .expect("owners should read")
            .is_empty(),
        "nobody owns what the engine installed"
    );

    let (_, editor) = an_editor("install").await;
    assert!(!aiwebengine::engine_api::can_write_script(&editor, "agent"));
    assert!(aiwebengine::engine_api::can_write_script(
        &puller(),
        "agent"
    ));

    let again = git_install::install_one(&agent_at(Some("v1")), Some(github.client())).await;
    assert!(matches!(again, Installed::Skipped(_)), "{again:?}");

    // An administrator upgrades it the ordinary way.
    let mut upgrade = at(V2, "");
    upgrade.repo = "lpajunen/agent".to_string();
    upgrade.prefix = None;
    pull_with(github.client(), &puller(), upgrade)
        .await
        .expect("an administrator may move what was installed");
    assert_eq!(asset_text("agent", "main.ts").as_deref(), Some("// v2"));

    let restart = git_install::install_one(&agent_at(Some("v1")), Some(github.client())).await;
    assert!(matches!(restart, Installed::Skipped(_)), "{restart:?}");
    assert_eq!(
        asset_text("agent", "main.ts").as_deref(),
        Some("// v2"),
        "a restart does not put the configured ref back"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn instances_starting_together_install_once() {
    let _guard = test_mutex().lock().await;
    setup_env().await;
    let github = tagged_then_moved().await;

    let install = agent_at(None);
    let (first, second) = tokio::join!(
        git_install::install_one(&install, Some(github.client())),
        git_install::install_one(&install, Some(github.client())),
    );
    let pulled = [&first, &second]
        .iter()
        .filter(|outcome| matches!(outcome, Installed::Pulled(_)))
        .count();
    assert_eq!(pulled, 1, "{first:?} / {second:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_install_never_takes_over_a_script_of_the_same_name() {
    let _guard = test_mutex().lock().await;
    setup_env().await;
    let github = tagged_then_moved().await;

    aiwebengine::engine_api::upsert_script_authorized(&puller(), "agent", "// mine", Some("test"))
        .expect("a script written here");

    let installed = git_install::install_one(&agent_at(None), Some(github.client())).await;
    let Installed::Failed(why) = &installed else {
        panic!("expected a refusal, got {installed:?}");
    };
    assert!(why.contains("did not put there"), "{why}");
    let root = repository::fetch_assets("agent")
        .into_iter()
        .find(|(name, _)| aiwebengine::module_loader::is_root_module_name(name))
        .map(|(_, asset)| String::from_utf8_lossy(&asset.content).into_owned());
    assert_eq!(root.as_deref(), Some("// mine"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_repository_that_cannot_be_read_is_reported_and_left() {
    let _guard = test_mutex().lock().await;
    setup_env().await;
    let github = tagged_then_moved().await;

    let installed =
        git_install::install_one(&agent_at(Some("no-such-tag")), Some(github.client())).await;
    assert!(matches!(installed, Installed::Failed(_)), "{installed:?}");
    assert!(repository::fetch_script("agent").is_none());

    let retried = git_install::install_one(&agent_at(Some("v1")), Some(github.client())).await;
    assert_eq!(
        retried,
        Installed::Pulled(vec!["agent".to_string()]),
        "a failed install leaves nothing that stops the next start trying"
    );
}
