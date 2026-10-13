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

    assert_eq!(report.branch, "main");
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
            branch: Some("no-such-branch".to_string()),
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
            branch: None,
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
            branch: None,
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
// Taking part of a repository
// ============================================================================

/// A pull of `repo` under `prefix` that takes only the directories named.
fn selecting(repo: &str, prefix: &str, scripts: &[&str]) -> PullRequest {
    PullRequest {
        repo: repo.to_string(),
        branch: None,
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
        matches!(error, aiwebengine::git_sync::SyncError::Diverged(_)),
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
        aiwebengine::git_sync::followed("lpajunen/cleared", "main", "cleared")
            .await
            .expect("follow should read"),
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
        aiwebengine::git_sync::followed("lpajunen/cleared", "main", "cleared")
            .await
            .expect("follow should read"),
        None
    );
}
