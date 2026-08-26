//! What a script's files have been, and building from a version other than
//! the deployed one.
//!
//! Two things are under test here and they are halves of one idea. Every write
//! records what the script became, so a change made through `/engine/assets` —
//! increasingly a change made by an agent with no checkout — has something
//! behind it. And a build can name which of those recorded states it reads
//! from, so a revision can be inspected without being published and a broken
//! head does not make the version before it unreachable.

mod common;

use aiwebengine::engine_api::{
    delete_asset_authorized, upsert_asset_authorized, upsert_script_authorized,
};
use aiwebengine::repository;
use aiwebengine::revisions;
use aiwebengine::security::UserContext;
use aiwebengine::source_view::SourceView;
use base64::Engine as _;
use tokio::sync::OnceCell;

static INIT: OnceCell<()> = OnceCell::const_new();

async fn setup_env() {
    INIT.get_or_init(|| async {
        let config = aiwebengine::config::AppConfig::test_config_postgres(0);
        if let Ok(db) = aiwebengine::database::Database::new(&config.repository).await {
            let db_arc = std::sync::Arc::new(db);
            aiwebengine::database::initialize_global_database(db_arc.clone());
            repository::initialize_repository(repository::PostgresRepository::new(
                db_arc.pool().clone(),
                "test".to_string(),
            ));
        }
    })
    .await;
}

fn admin() -> UserContext {
    UserContext::admin("reviser".to_string())
}

/// Deploy a script through the authorized path, so the write is recorded the
/// way a caller's write is.
async fn deploy(script_uri: &str, content: &str) {
    // Delete rather than overwrite: revisions cascade with the script, so each
    // test starts from no history at all. Otherwise a test asserting on the
    // shape of a history would be reading every previous run's as well.
    let uri = script_uri.to_string();
    tokio::task::spawn_blocking(move || repository::delete_script(&uri))
        .await
        .expect("join");
    let (uri, content) = (script_uri.to_string(), content.to_string());
    tokio::task::spawn_blocking(move || {
        upsert_script_authorized(&admin(), &uri, &content, None).expect("script should be stored")
    })
    .await
    .expect("join");
}

async fn write_asset(script_uri: &str, asset_uri: &str, content: &str) -> Option<i32> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(content);
    let (script, asset) = (script_uri.to_string(), asset_uri.to_string());
    tokio::task::spawn_blocking(move || {
        upsert_asset_authorized(&admin(), &script, &asset, "text/typescript", &encoded)
            .unwrap_or_else(|_| panic!("asset should be stored"))
    })
    .await
    .expect("join")
}

async fn delete_asset(script_uri: &str, asset_uri: &str) -> Option<i32> {
    let (script, asset) = (script_uri.to_string(), asset_uri.to_string());
    tokio::task::spawn_blocking(move || {
        delete_asset_authorized(&admin(), &script, &asset)
            .unwrap_or_else(|_| panic!("delete should be authorized"))
    })
    .await
    .expect("join")
    .1
}

#[tokio::test(flavor = "multi_thread")]
async fn each_write_records_the_next_revision() {
    setup_env().await;
    let uri = "test://revisions/sequence";
    deploy(uri, "function init() {}").await;

    let first = write_asset(uri, "server/one.ts", "export const one = 1;").await;
    let second = write_asset(uri, "server/two.ts", "export const two = 2;").await;

    let first = first.expect("the first asset write records a revision");
    let second = second.expect("the second asset write records a revision");
    assert!(
        second > first,
        "revisions advance: {} did not follow {}",
        second,
        first
    );

    let head = revisions::head(uri).await.expect("head should read");
    assert_eq!(head, Some(second), "the newest write is head");
}

#[tokio::test(flavor = "multi_thread")]
async fn writing_the_same_content_again_records_nothing() {
    setup_env().await;
    let uri = "test://revisions/idempotent";
    deploy(uri, "function init() {}").await;

    let first = write_asset(uri, "server/same.ts", "export const same = 1;").await;
    assert!(first.is_some(), "the first write is a change");

    let again = write_asset(uri, "server/same.ts", "export const same = 1;").await;
    assert_eq!(
        again, None,
        "a write that changed nothing is not a revision: it would push real \
         history out of any retention window that counts rows"
    );

    let head = revisions::head(uri).await.expect("head should read");
    assert_eq!(head, first);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revision_holds_the_files_that_were_there() {
    setup_env().await;
    let uri = "test://revisions/manifest";
    deploy(uri, "function init() {}").await;

    write_asset(uri, "server/kept.ts", "export const kept = 1;").await;
    let two_files = write_asset(uri, "server/gone.ts", "export const gone = 2;")
        .await
        .expect("write records a revision");

    let after_delete = delete_asset(uri, "server/gone.ts")
        .await
        .expect("a deletion is a change worth recording");

    let before = revisions::files(uri, two_files)
        .await
        .expect("manifest should read")
        .expect("revision should exist");
    let after = revisions::files(uri, after_delete)
        .await
        .expect("manifest should read")
        .expect("revision should exist");

    let names =
        |files: &[revisions::RevisionFile]| files.iter().map(|f| f.uri.clone()).collect::<Vec<_>>();
    assert_eq!(names(&before), vec!["server/gone.ts", "server/kept.ts"]);
    assert_eq!(
        names(&after),
        vec!["server/kept.ts"],
        "a file a revision does not contain is absent from it, not tombstoned"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revision_reads_back_the_content_it_recorded() {
    setup_env().await;
    let uri = "test://revisions/content";
    deploy(uri, "function init() {}").await;

    let original = write_asset(uri, "server/value.ts", "export const value = 1;")
        .await
        .expect("write records a revision");
    write_asset(uri, "server/value.ts", "export const value = 999;").await;

    let (content, mimetype) = revisions::read_file(uri, original, "server/value.ts")
        .await
        .expect("read should succeed")
        .expect("the revision contained the file");

    assert_eq!(
        String::from_utf8(content).unwrap(),
        "export const value = 1;"
    );
    assert_eq!(mimetype, "text/typescript");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_files_history_skips_the_revisions_that_left_it_alone() {
    setup_env().await;
    let uri = "test://revisions/file-history";
    deploy(uri, "function init() {}").await;

    write_asset(uri, "server/watched.ts", "export const v = 1;").await;
    // Three revisions in which the watched file did not move.
    write_asset(uri, "server/other-a.ts", "export const a = 1;").await;
    write_asset(uri, "server/other-b.ts", "export const b = 1;").await;
    write_asset(uri, "server/other-c.ts", "export const c = 1;").await;
    write_asset(uri, "server/watched.ts", "export const v = 2;").await;

    let history = revisions::file_history(uri, "server/watched.ts", 100)
        .await
        .expect("history should read");

    assert_eq!(
        history.len(),
        2,
        "the file changed twice; the revisions between them say nothing about it: {:?}",
        history
            .iter()
            .map(|(revision, _)| *revision)
            .collect::<Vec<_>>()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_label_names_one_revision_and_moves_when_reused() {
    setup_env().await;
    let uri = "test://revisions/labels";
    deploy(uri, "function init() {}").await;

    let first = write_asset(uri, "server/a.ts", "export const a = 1;")
        .await
        .expect("write records a revision");
    let second = write_asset(uri, "server/a.ts", "export const a = 2;")
        .await
        .expect("write records a revision");

    revisions::set_label(uri, first, Some("before-the-change"))
        .await
        .expect("labelling should succeed");
    assert_eq!(
        revisions::by_label(uri, "before-the-change").await.unwrap(),
        Some(first)
    );

    // A label names a state worth returning to, and that state can be
    // superseded — so reusing the name moves it rather than being refused.
    revisions::set_label(uri, second, Some("before-the-change"))
        .await
        .expect("relabelling should succeed");
    assert_eq!(
        revisions::by_label(uri, "before-the-change").await.unwrap(),
        Some(second)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revision_builds_from_the_modules_it_had() {
    setup_env().await;
    let uri = "test://revisions/build/main.ts";
    let root = "import { help } from './server/help.ts';\nfunction init() { help(); }";
    deploy(uri, root).await;

    let with_helper = write_asset(uri, "server/help.ts", "export function help() {}")
        .await
        .expect("write records a revision");

    // Head is now broken: the root imports a module that is no longer there.
    delete_asset(uri, "server/help.ts").await;

    let live =
        aiwebengine::module_loader::prepare_executable_program_in(uri, root, &SourceView::Live);
    assert!(
        live.is_err(),
        "the deployed tree is missing the module the root imports"
    );

    let at_revision = aiwebengine::module_loader::prepare_executable_program_in(
        uri,
        root,
        &SourceView::Revision(with_helper),
    );
    assert!(
        at_revision.is_ok(),
        "revision {} still holds the module, so it still builds: {:?}",
        with_helper,
        at_revision.err()
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revision_discovers_the_tests_it_contained() {
    setup_env().await;
    let uri = "test://revisions/tests/main.ts";
    deploy(uri, "function init() {}").await;

    let with_test = write_asset(uri, "server/thing.test.ts", "// a test module")
        .await
        .expect("write records a revision");
    delete_asset(uri, "server/thing.test.ts").await;

    let live = aiwebengine::module_loader::discover_test_modules_in(uri, &SourceView::Live);
    let historic =
        aiwebengine::module_loader::discover_test_modules_in(uri, &SourceView::Revision(with_test));

    assert!(
        live.is_empty(),
        "the test file has been deleted from the deployed tree"
    );
    assert_eq!(
        historic,
        vec!["server/thing.test.ts".to_string()],
        "a revision's tests are the ones it contained"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_overlay_builds_from_files_that_were_never_stored() {
    setup_env().await;
    let uri = "test://revisions/overlay/main.ts";
    let root = "import { help } from './server/help.ts';\nfunction init() { help(); }";
    deploy(uri, root).await;

    // Nothing is written: the module exists only in the request.
    let mut files = std::collections::BTreeMap::new();
    files.insert(
        "server/help.ts".to_string(),
        aiwebengine::source_view::OverlayEntry::Written(
            aiwebengine::source_view::SourceFile::text(
                "export function help() {}",
                "text/typescript",
            ),
        ),
    );
    let candidate = SourceView::overlay(files);

    assert!(
        aiwebengine::module_loader::prepare_executable_program_in(uri, root, &SourceView::Live)
            .is_err(),
        "the module has not been written"
    );
    assert!(
        aiwebengine::module_loader::prepare_executable_program_in(uri, root, &candidate).is_ok(),
        "a candidate spanning several files is checkable before any of it lands"
    );
    assert!(
        repository::fetch_asset(uri, "server/help.ts").is_none(),
        "checking a candidate must not store it"
    );
}
