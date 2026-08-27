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
    RevertOutcome, RevertRefusal, delete_asset_authorized, revert_authorized,
    upsert_asset_authorized,
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

fn stored_text(script_uri: &str, asset_uri: &str) -> String {
    let asset = repository::fetch_asset(script_uri, asset_uri).expect("asset should be stored");
    String::from_utf8(asset.content).expect("asset should be UTF-8")
}

fn admin() -> UserContext {
    UserContext::admin("reviser".to_string())
}

/// Store a script and record the revision that write produced.
///
/// Deliberately not through `upsert_script_authorized`: that spawns the
/// script's `init()`, which records its outcome against whatever is head when
/// it finishes. A test that sets init outcomes of its own would be racing it,
/// and a test that does not would still have a background task writing to the
/// history it is asserting on.
///
/// The script is deleted rather than overwritten, so each test starts from no
/// history at all — revisions cascade with the script. Otherwise a test
/// asserting on the shape of a history would be reading every previous run's
/// as well.
async fn deploy(script_uri: &str, content: &str) {
    clear_script_state(script_uri).await;

    let (uri, content) = (script_uri.to_string(), content.to_string());
    tokio::task::spawn_blocking({
        let uri = uri.clone();
        move || repository::upsert_script(&uri, &content).expect("script should be stored")
    })
    .await
    .expect("join");

    // Through the async form so a failure says what went wrong. The blocking
    // wrapper reports failure as `None`, which is also what "nothing changed"
    // looks like — indistinguishable in a test that only knows it got nothing.
    // `None` here means the content already had a revision — an engine
    // instance sharing this database can backfill a baseline for a script it
    // finds without one, and between the write above and this call is exactly
    // when it would. Either way the script must end up with a history.
    revisions::record(&uri, revisions::Origin::Script, Some("reviser"))
        .await
        .expect("recording the deploy should succeed");

    revisions::head(&uri)
        .await
        .expect("head should read")
        .expect("a deployed script has a history");
}

/// Remove every trace of a script: its row, its files, its history and its log.
///
/// Spelled out rather than left to `delete_script`'s cascade. That path drops
/// the script's tables first, which takes ACCESS EXCLUSIVE locks and can fail
/// under load — and it reports failure by returning `false`, so a test built on
/// it starts from whatever the previous run left. Logs are not cascaded at all,
/// which a test asserting on log contents notices immediately.
async fn clear_script_state(script_uri: &str) {
    let pool = aiwebengine::database::get_global_database()
        .expect("database should be initialized")
        .pool()
        .clone();

    for statement in [
        "DELETE FROM logs WHERE script_uri = $1",
        "DELETE FROM script_revisions WHERE script_uri = $1",
        "DELETE FROM assets WHERE script_uri = $1",
        "DELETE FROM scripts WHERE uri = $1",
    ] {
        sqlx::query(statement)
            .bind(script_uri)
            .execute(&pool)
            .await
            .unwrap_or_else(|e| panic!("clearing '{}' should succeed: {}", statement, e));
    }

    // The in-memory caches still hold what was just deleted.
    revisions::forget_current(script_uri);
}

/// Deploy a script from nothing: clear its state, then store and record it.
async fn deploy_over_fresh(script_uri: &str, content: &str) {
    deploy(script_uri, content).await;
}

/// Rewrite a deployed script's root without clearing its history.
async fn deploy_over(script_uri: &str, content: &str) {
    let (uri, content) = (script_uri.to_string(), content.to_string());
    tokio::task::spawn_blocking(move || {
        repository::upsert_script(&uri, &content).expect("script should be stored");
        revisions::record_blocking(&uri, revisions::Origin::Script, Some("reviser"))
    })
    .await
    .expect("join")
    .expect("a rewritten root records a revision");
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

// ============================================================================
// Reverting
// ============================================================================

async fn revert(script_uri: &str, spec: &str, dry_run: bool) -> RevertOutcome {
    revert_authorized(&admin(), script_uri, spec, dry_run, false)
        .await
        .unwrap_or_else(|_| panic!("revert should be permitted"))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revert_restores_content_and_removes_what_the_target_never_had() {
    setup_env().await;
    let uri = "test://revisions/revert/main.ts";
    deploy(uri, "function init() {}").await;

    write_asset(uri, "server/keep.ts", "export const v = 1;").await;
    let target = write_asset(uri, "server/gone-later.ts", "export const g = 1;")
        .await
        .expect("write records a revision");

    // Move on: change one file, remove another, add a third.
    write_asset(uri, "server/keep.ts", "export const v = 2;").await;
    delete_asset(uri, "server/gone-later.ts").await;
    write_asset(uri, "server/added-later.ts", "export const a = 1;").await;

    let outcome = revert(uri, &target.to_string(), false).await;

    assert_eq!(outcome.target, target);
    assert!(
        outcome.revision.is_some(),
        "a revert that changed files records a revision"
    );

    assert_eq!(stored_text(uri, "server/keep.ts"), "export const v = 1;");
    assert_eq!(
        stored_text(uri, "server/gone-later.ts"),
        "export const g = 1;",
        "a file the target held is restored"
    );
    assert!(
        repository::fetch_asset(uri, "server/added-later.ts").is_none(),
        "a file the target never had is removed, or the tree is neither version"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revert_is_a_forward_write_that_names_what_it_restored() {
    setup_env().await;
    let uri = "test://revisions/revert/forward.ts";
    deploy(uri, "function init() {}").await;

    let target = write_asset(uri, "server/a.ts", "export const a = 1;")
        .await
        .expect("write records a revision");
    let head_before = write_asset(uri, "server/a.ts", "export const a = 2;")
        .await
        .expect("write records a revision");

    let outcome = revert(uri, &target.to_string(), false).await;
    let recorded = outcome.revision.expect("the revert records a revision");

    assert!(
        recorded > head_before,
        "a revert moves history forward ({} should follow {})",
        recorded,
        head_before
    );

    let revision = aiwebengine::revisions::get(uri, recorded)
        .await
        .expect("revision should read")
        .expect("revision should exist");
    assert_eq!(revision.origin, "revert");
    assert_eq!(
        revision.parent,
        Some(target),
        "the parent is what it restored, not what it followed — that is what \
         makes the history a graph rather than a line that doubles back"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dry_run_reports_the_change_without_making_it() {
    setup_env().await;
    let uri = "test://revisions/revert/dry.ts";
    deploy(uri, "function init() {}").await;

    let target = write_asset(uri, "server/a.ts", "export const a = 1;")
        .await
        .expect("write records a revision");
    write_asset(uri, "server/a.ts", "export const a = 2;").await;

    let outcome = revert(uri, &target.to_string(), true).await;

    assert!(outcome.dry_run);
    assert_eq!(outcome.written, vec!["server/a.ts".to_string()]);
    assert_eq!(outcome.revision, None, "a dry run records nothing");
    assert_eq!(
        stored_text(uri, "server/a.ts"),
        "export const a = 2;",
        "a dry run leaves the deployed file alone"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn reverting_to_what_is_already_deployed_changes_nothing() {
    setup_env().await;
    let uri = "test://revisions/revert/noop.ts";
    deploy(uri, "function init() {}").await;

    let head = write_asset(uri, "server/a.ts", "export const a = 1;")
        .await
        .expect("write records a revision");

    let outcome = revert(uri, &head.to_string(), false).await;

    assert!(outcome.written.is_empty());
    assert!(outcome.deleted.is_empty());
    assert_eq!(
        outcome.revision, None,
        "restoring what is already there is not a change, so it is not a revision"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revert_to_a_revision_that_does_not_bundle_is_refused() {
    setup_env().await;
    let uri = "test://revisions/revert/broken/main.ts";
    // Revision 1: the root imports a module that was never written.
    deploy(
        uri,
        "import { missing } from './server/missing.ts';\nfunction init() { missing(); }",
    )
    .await;
    let broken = aiwebengine::revisions::head(uri)
        .await
        .expect("head should read")
        .expect("deploying records a revision");

    let refusal = revert_authorized(&admin(), uri, &broken.to_string(), false, false).await;
    assert!(
        matches!(refusal, Err(RevertRefusal::WillNotBuild(_))),
        "a revert onto a tree that cannot be bundled should be refused before it lands"
    );

    // The engine can only know this because it can build from a revision
    // without deploying it — so `force` is there for the caller who means it.
    let forced = revert_authorized(&admin(), uri, &broken.to_string(), true, true).await;
    assert!(forced.is_ok(), "force says the caller meant it");
}

#[tokio::test(flavor = "multi_thread")]
async fn last_good_names_the_newest_revision_that_initialised() {
    setup_env().await;
    let uri = "test://revisions/revert/last-good.ts";
    let (good, bad) = deploy_unnoticed(
        uri,
        "function init() {}",
        ("server/a.ts", "export const a = 1;"),
    )
    .await;

    set_init(uri, good, true, None).await;
    set_init(uri, bad, false, Some("TypeError: boom")).await;

    assert_eq!(
        aiwebengine::revisions::last_good(uri).await.unwrap(),
        Some(good),
        "revision {} initialised and {} did not, so {} is where a rollback lands",
        good,
        bad,
        good
    );
}

// ============================================================================
// Retention
// ============================================================================

/// Age every revision of one script, so a retention window that counts days
/// has something to act on without the test waiting for one.
async fn age_revisions(script_uri: &str, days: i32) {
    let pool = aiwebengine::database::get_global_database()
        .expect("database should be initialized")
        .pool()
        .clone();
    sqlx::query(
        "UPDATE script_revisions SET created_at = NOW() - make_interval(days => $2)
         WHERE script_uri = $1",
    )
    .bind(script_uri)
    .bind(days)
    .execute(&pool)
    .await
    .expect("ageing revisions should succeed");
}

/// Record an init outcome against a named revision.
///
/// The engine's own path attaches to head, which is right in production — the
/// write that triggered the init is the newest — and unusable in a test that
/// wants two revisions to have different outcomes.
async fn set_init(script_uri: &str, revision: i32, ok: bool, error: Option<&str>) {
    aiwebengine::revisions::set_init_outcome(script_uri, revision, ok, error)
        .await
        .expect("recording the init outcome should succeed");
}

/// Store a script and one asset without telling the cluster.
///
/// Every write through the repository sends a `script_upserted` notification,
/// and any engine instance sharing this database answers it by re-running the
/// script's `init()` and recording the outcome against head. That is correct,
/// and it makes a test that asserts on init outcomes a race against a peer.
/// Writing the rows directly leaves no notification for anyone to answer.
async fn deploy_unnoticed(script_uri: &str, content: &str, asset: (&str, &str)) -> (i32, i32) {
    clear_script_state(script_uri).await;

    let pool = aiwebengine::database::get_global_database()
        .expect("database should be initialized")
        .pool()
        .clone();

    sqlx::query("INSERT INTO scripts (uri, content, name) VALUES ($1, $2, $3)")
        .bind(script_uri)
        .bind(content)
        .bind(script_uri.rsplit('/').next().unwrap_or(script_uri))
        .execute(&pool)
        .await
        .expect("storing the script should succeed");

    let first = aiwebengine::revisions::record(script_uri, revisions::Origin::Script, None)
        .await
        .expect("recording should succeed")
        .expect("a new script records a revision");

    sqlx::query(
        "INSERT INTO assets (uri, name, mimetype, content, script_uri)
         VALUES ($1, $1, 'text/typescript', $2, $3)",
    )
    .bind(asset.0)
    .bind(asset.1.as_bytes())
    .bind(script_uri)
    .execute(&pool)
    .await
    .expect("storing the asset should succeed");

    let second = aiwebengine::revisions::record(script_uri, revisions::Origin::Post, None)
        .await
        .expect("recording should succeed")
        .expect("a new file records a revision");

    (first, second)
}

async fn revision_numbers(script_uri: &str) -> Vec<i32> {
    aiwebengine::revisions::list(script_uri, 1000)
        .await
        .expect("history should read")
        .into_iter()
        .map(|revision| revision.revision)
        .collect()
}

/// Keep nothing for its age or its recency, so what survives is only what the
/// policy protects outright.
const KEEP_NOTHING: aiwebengine::revisions::Retention = aiwebengine::revisions::Retention {
    keep_days: 0,
    keep_per_script: 1,
    // Blobs keep their production grace here. The sweep is over the whole
    // table however narrow the scope, so a test that collected everything
    // young would be reaching into whatever another test process is writing
    // at that moment. Only the test that is about collection lifts it.
    blob_grace_secs: 3600.0,
};

#[tokio::test(flavor = "multi_thread")]
async fn pruning_keeps_the_newest_revision_and_drops_the_churn() {
    setup_env().await;
    let uri = "test://revisions/prune/churn.ts";
    deploy(uri, "function init() {}").await;

    for value in 1..=5 {
        write_asset(uri, "server/a.ts", &format!("export const a = {};", value)).await;
    }
    age_revisions(uri, 90).await;

    let before = revision_numbers(uri).await;
    assert!(before.len() > 1, "the test needs some history to prune");

    let outcome = aiwebengine::revisions::prune(Some(uri), KEEP_NOTHING)
        .await
        .expect("prune should succeed");
    assert!(outcome.revisions > 0, "old churn should be collectable");

    let newest = *before.first().expect("history is not empty");
    let after = revision_numbers(uri).await;

    assert!(
        after.contains(&newest),
        "the newest revision is never collected: it is what the script is"
    );
    assert!(
        after.len() < before.len(),
        "the churn between the protected revisions should have gone: {:?}",
        after
    );
    // Deliberately not an assertion on the exact set. Which revisions are
    // protected depends on which one is last-good, and that is not this
    // test's to fix: writing an asset notifies the cluster, and any engine
    // instance sharing this database re-runs init() and records the outcome
    // against head. Both halves of what this test is named for — the newest
    // survives, the churn goes — hold regardless.
}

#[tokio::test(flavor = "multi_thread")]
async fn pruning_keeps_a_labelled_revision_however_old() {
    setup_env().await;
    let uri = "test://revisions/prune/labelled.ts";
    deploy(uri, "function init() {}").await;

    let marked = write_asset(uri, "server/a.ts", "export const a = 1;")
        .await
        .expect("write records a revision");
    for value in 2..=5 {
        write_asset(uri, "server/a.ts", &format!("export const a = {};", value)).await;
    }
    aiwebengine::revisions::set_label(uri, marked, Some("before-the-refactor"))
        .await
        .expect("labelling should succeed");
    age_revisions(uri, 90).await;

    aiwebengine::revisions::prune(Some(uri), KEEP_NOTHING)
        .await
        .expect("prune should succeed");

    assert!(
        revision_numbers(uri).await.contains(&marked),
        "a label is someone saying this one is worth returning to; retention \
         does not get to disagree"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn pruning_keeps_the_revision_a_rollback_would_land_on() {
    setup_env().await;
    let uri = "test://revisions/prune/last-good.ts";
    deploy(uri, "function init() {}").await;

    let good = write_asset(uri, "server/a.ts", "export const a = 1;")
        .await
        .expect("write records a revision");

    for value in 2..=5 {
        write_asset(uri, "server/a.ts", &format!("export const a = {};", value)).await;
    }
    for revision in revision_numbers(uri).await {
        let ok = revision == good;
        set_init(uri, revision, ok, (!ok).then_some("TypeError: boom")).await;
    }
    age_revisions(uri, 90).await;

    aiwebengine::revisions::prune(Some(uri), KEEP_NOTHING)
        .await
        .expect("prune should succeed");

    // The invariant is that a rollback always has somewhere to land — not that
    // any particular revision is where it lands. Which revision that is can
    // legitimately change under this test: writing an asset notifies the
    // cluster, and any engine instance sharing this database will re-run the
    // script's init() and record the outcome against head. That is the right
    // behaviour, so the assertion is on the property rather than on the number.
    let floor = aiwebengine::revisions::last_good(uri)
        .await
        .expect("last good should read")
        .expect("some revision of this script initialised");

    let surviving = revision_numbers(uri).await;
    assert!(
        surviving.contains(&floor),
        "retention collected the revision a rollback would land on: floor {}, \
         survivors {:?}",
        floor,
        surviving
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn recent_history_survives_a_pruning_pass() {
    setup_env().await;
    let uri = "test://revisions/prune/recent.ts";
    deploy(uri, "function init() {}").await;

    for value in 1..=4 {
        write_asset(uri, "server/a.ts", &format!("export const a = {};", value)).await;
    }
    let before = revision_numbers(uri).await;

    // Nothing here is older than a day, so the age clause protects all of it
    // even though the count clause would not.
    let outcome = aiwebengine::revisions::prune(
        Some(uri),
        aiwebengine::revisions::Retention {
            keep_days: 30,
            keep_per_script: 1,
            ..KEEP_NOTHING
        },
    )
    .await
    .expect("prune should succeed");

    assert_eq!(outcome.revisions, 0);
    assert_eq!(
        revision_numbers(uri).await,
        before,
        "age and count both have to agree before anything goes"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_blob_survives_while_another_revision_still_cites_it() {
    setup_env().await;
    let uri = "test://revisions/prune/blobs.ts";
    deploy(uri, "function init() {}").await;

    // The same content in two files: one blob, two manifest rows.
    let shared = "export const shared = 1;";
    write_asset(uri, "server/one.ts", shared).await;
    let both = write_asset(uri, "server/two.ts", shared)
        .await
        .expect("write records a revision");

    let digest = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(shared.as_bytes());
        hex::encode(hasher.finalize())
    };

    // Remove one of the two files, then collect. The other still holds the
    // content, so the blob has to stay.
    delete_asset(uri, "server/one.ts").await;
    aiwebengine::revisions::prune(Some(uri), KEEP_NOTHING)
        .await
        .expect("prune should succeed");

    let pool = aiwebengine::database::get_global_database()
        .expect("database should be initialized")
        .pool()
        .clone();
    let survives: bool =
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM asset_blobs WHERE sha256 = $1)")
            .bind(&digest)
            .fetch_one(&pool)
            .await
            .expect("blob lookup should succeed");

    assert!(
        survives,
        "revision {} still lists this content under server/two.ts; blobs are \
         shared, so one revision going away says nothing about the bytes",
        both
    );
    assert_eq!(stored_text(uri, "server/two.ts"), shared);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_blob_is_collected_once_no_revision_names_it() {
    setup_env().await;
    let uri = "test://revisions/prune/orphan.ts";
    deploy(uri, "function init() {}").await;

    // Content unique to this test, so nothing else in the database cites it.
    let doomed = format!("export const doomed = {:?};", uri);
    write_asset(uri, "server/doomed.ts", &doomed).await;
    write_asset(uri, "server/doomed.ts", "export const doomed = 'replaced';").await;

    let digest = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(doomed.as_bytes());
        hex::encode(hasher.finalize())
    };

    let pool = aiwebengine::database::get_global_database()
        .expect("database should be initialized")
        .pool()
        .clone();
    let exists = |digest: String, pool: sqlx::PgPool| async move {
        sqlx::query_scalar::<_, bool>("SELECT EXISTS (SELECT 1 FROM asset_blobs WHERE sha256 = $1)")
            .bind(&digest)
            .fetch_one(&pool)
            .await
            .expect("blob lookup should succeed")
    };

    assert!(
        exists(digest.clone(), pool.clone()).await,
        "the revision that held this content still exists"
    );

    age_revisions(uri, 90).await;

    // Collection is best-effort: the sweep is over the whole table, and a
    // writer claiming a blob between the decision and the delete makes the
    // foreign key refuse it. That is the contract — "an orphan that survives
    // one pass is collected by the next" — so the test takes it at its word
    // rather than demanding the first pass succeed against a database other
    // tests are writing to.
    let collecting = aiwebengine::revisions::Retention {
        blob_grace_secs: 0.0,
        ..KEEP_NOTHING
    };
    let mut collected = false;
    for _ in 0..10 {
        aiwebengine::revisions::prune(Some(uri), collecting)
            .await
            .expect("prune should succeed");
        if !exists(digest.clone(), pool.clone()).await {
            collected = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    assert!(
        collected,
        "with the revision that cited it gone, the bytes are nobody's"
    );
}

// ============================================================================
// Checking a version other than the deployed one
// ============================================================================

fn check_at(script_uri: &str, view: aiwebengine::source_view::SourceView) -> String {
    let report = aiwebengine::script_check::check_blocking(
        aiwebengine::script_check::CheckRequest {
            script_uri: script_uri.to_string(),
            content: None,
            rollback: true,
            timeout_ms: Some(5_000),
            view,
        },
        5_000,
    );
    if report.ok {
        return String::new();
    }
    report
        .diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.code != "no-init")
        .map(|diagnostic| format!("{}: {}", diagnostic.code, diagnostic.message))
        .collect::<Vec<_>>()
        .join("; ")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_check_at_a_revision_runs_init_against_that_revision_s_modules() {
    setup_env().await;
    let uri = "test://revisions/check-at/main.ts";
    let root = "import { help } from './server/help.ts';\nfunction init() { return help(); }";
    deploy(uri, root).await;

    let with_helper = write_asset(
        uri,
        "server/help.ts",
        "export function help() { return {}; }",
    )
    .await
    .expect("write records a revision");

    // Head is broken: the module the root imports is gone.
    delete_asset(uri, "server/help.ts").await;

    assert!(
        !check_at(uri, aiwebengine::source_view::SourceView::Live).is_empty(),
        "the deployed tree is missing the module the root imports"
    );

    // Both halves of a check have to read from the same version. Bundling the
    // probe at the revision and then running init() against the deployed
    // modules would check a program made of one version's root and another's
    // imports — which is to say, one that exists nowhere.
    let failures = check_at(
        uri,
        aiwebengine::source_view::SourceView::Revision(with_helper),
    );
    assert!(
        failures.is_empty(),
        "revision {} still holds the module, so it checks clean: {}",
        with_helper,
        failures
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_candidate_can_be_checked_against_the_revision_it_was_written_for() {
    setup_env().await;
    let uri = "test://revisions/check-overlay/main.ts";
    let root = "import { help } from './server/help.ts';\nfunction init() { return help(); }";
    deploy(uri, root).await;

    let written_for = write_asset(
        uri,
        "server/help.ts",
        "export function help() { return {}; }",
    )
    .await
    .expect("write records a revision");

    // Head has moved on and the module is gone, so the change cannot be
    // checked against head at all.
    delete_asset(uri, "server/help.ts").await;

    let mut files = std::collections::BTreeMap::new();
    files.insert(
        "server/extra.ts".to_string(),
        aiwebengine::source_view::OverlayEntry::Written(
            aiwebengine::source_view::SourceFile::text(
                "export const extra = 1;",
                "text/typescript",
            ),
        ),
    );

    let over_head = aiwebengine::source_view::SourceView::overlay(files.clone());
    assert!(
        !check_at(uri, over_head).is_empty(),
        "over head, the change is checked against a tree that is already broken"
    );

    let over_revision = aiwebengine::source_view::SourceView::overlay_on(
        aiwebengine::source_view::SourceView::Revision(written_for),
        files,
    );
    assert!(
        check_at(uri, over_revision).is_empty(),
        "over the revision it was written for, the same change checks clean"
    );
}

// ============================================================================
// Labels and diffs
// ============================================================================

#[tokio::test(flavor = "multi_thread")]
async fn a_diff_shows_the_lines_a_revision_changed() {
    setup_env().await;
    let uri = "test://revisions/diff/lines.ts";
    deploy(uri, "function init() {}").await;

    write_asset(
        uri,
        "server/speed.ts",
        "export const SPEED = 4;\nexport const X = 1;\n",
    )
    .await;
    let after = write_asset(
        uri,
        "server/speed.ts",
        "export const SPEED = 6;\nexport const X = 1;\n",
    )
    .await
    .expect("write records a revision");

    let diff = aiwebengine::revisions::diff(uri, after - 1, after, 3)
        .await
        .expect("diff should read")
        .expect("both revisions exist");

    assert_eq!(diff.files.len(), 1, "only one file moved: {:?}", diff.files);
    let file = &diff.files[0];
    assert_eq!(file.uri, "server/speed.ts");
    assert_eq!(file.status, "modified");

    let rendered = file.diff.as_deref().expect("a text file gets a diff");
    assert!(
        rendered.contains("-export const SPEED = 4;"),
        "{}",
        rendered
    );
    assert!(
        rendered.contains("+export const SPEED = 6;"),
        "{}",
        rendered
    );
    assert!(
        !rendered.contains("-export const X = 1;"),
        "an unchanged line is context, not a change: {}",
        rendered
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_diff_reports_files_added_and_removed() {
    setup_env().await;
    let uri = "test://revisions/diff/shape.ts";
    deploy(uri, "function init() {}").await;

    let before = write_asset(uri, "server/leaving.ts", "export const a = 1;\n")
        .await
        .expect("write records a revision");
    write_asset(uri, "server/arriving.ts", "export const b = 2;\n").await;
    let after = delete_asset(uri, "server/leaving.ts")
        .await
        .expect("a deletion is a revision");

    let diff = aiwebengine::revisions::diff(uri, before, after, 3)
        .await
        .expect("diff should read")
        .expect("both revisions exist");

    let by_path: std::collections::BTreeMap<&str, &str> = diff
        .files
        .iter()
        .map(|file| (file.uri.as_str(), file.status))
        .collect();

    assert_eq!(by_path.get("server/arriving.ts"), Some(&"added"));
    assert_eq!(by_path.get("server/leaving.ts"), Some(&"removed"));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_diff_leaves_untouched_files_out() {
    setup_env().await;
    let uri = "test://revisions/diff/quiet.ts";
    deploy(uri, "function init() {}").await;

    write_asset(uri, "server/still.ts", "export const still = 1;\n").await;
    let before = write_asset(uri, "server/moving.ts", "export const moving = 1;\n")
        .await
        .expect("write records a revision");
    let after = write_asset(uri, "server/moving.ts", "export const moving = 2;\n")
        .await
        .expect("write records a revision");

    let diff = aiwebengine::revisions::diff(uri, before, after, 3)
        .await
        .expect("diff should read")
        .expect("both revisions exist");

    assert_eq!(
        diff.files
            .iter()
            .map(|file| file.uri.as_str())
            .collect::<Vec<_>>(),
        vec!["server/moving.ts"],
        "equal digests are equal bytes; there is nothing to render for the rest"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_diff_covers_the_root_source() {
    setup_env().await;
    let uri = "test://revisions/diff/root/main.ts";
    deploy(uri, "function init() { return 1; }\n").await;
    let before = aiwebengine::revisions::head(uri)
        .await
        .expect("head should read")
        .expect("deploying records a revision");

    deploy_over(uri, "function init() { return 2; }\n").await;
    let after = aiwebengine::revisions::head(uri)
        .await
        .expect("head should read")
        .expect("the rewrite records a revision");

    let diff = aiwebengine::revisions::diff(uri, before, after, 3)
        .await
        .expect("diff should read")
        .expect("both revisions exist");

    let root = diff
        .files
        .iter()
        .find(|file| file.uri == "main.ts")
        .expect("the root is a file of the script like any other");
    let rendered = root.diff.as_deref().expect("the root is text");
    assert!(
        rendered.contains("+function init() { return 2; }"),
        "{}",
        rendered
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_label_survives_and_names_a_revision_to_restore() {
    setup_env().await;
    let uri = "test://revisions/label/restore.ts";
    deploy(uri, "function init() {}").await;

    let marked = write_asset(uri, "server/a.ts", "export const a = 1;")
        .await
        .expect("write records a revision");
    write_asset(uri, "server/a.ts", "export const a = 2;").await;

    aiwebengine::revisions::set_label(uri, marked, Some("known-good"))
        .await
        .expect("labelling should succeed");

    // The label is a name for the revision everywhere a revision is named.
    let outcome = revert(uri, "known-good", false).await;
    assert_eq!(outcome.target, marked);
    assert_eq!(stored_text(uri, "server/a.ts"), "export const a = 1;");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_bare_diff_question_answers_with_the_newest_change() {
    setup_env().await;
    let uri = "test://revisions/diff/default.ts";
    deploy(uri, "function init() {}").await;

    write_asset(uri, "server/a.ts", "export const a = 1;\n").await;
    write_asset(uri, "server/a.ts", "export const a = 2;\n").await;
    let head = write_asset(uri, "server/b.ts", "export const b = 1;\n")
        .await
        .expect("write records a revision");

    let result = aiwebengine::engine_api::execute_native_mcp_tool(
        "diff_revisions",
        &serde_json::json!({ "script": uri }),
        &admin(),
    )
    .expect("diff_revisions is a native tool");

    assert_eq!(result["to"], serde_json::json!(head));
    assert_eq!(result["from"], serde_json::json!(head - 1));
    assert_eq!(
        result["files"]
            .as_array()
            .expect("files should be a list")
            .iter()
            .map(|file| file["uri"].as_str().unwrap_or_default().to_string())
            .collect::<Vec<_>>(),
        vec!["server/b.ts".to_string()],
        "asking what changed, with nothing else said, means the newest change"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_first_revision_has_nothing_before_it_to_compare() {
    setup_env().await;
    let uri = "test://revisions/diff/first.ts";
    deploy(uri, "function init() {}").await;

    let result = aiwebengine::engine_api::execute_native_mcp_tool(
        "diff_revisions",
        &serde_json::json!({ "script": uri }),
        &admin(),
    )
    .expect("diff_revisions is a native tool");

    assert_eq!(result["success"], serde_json::json!(true));
    assert!(
        result["message"]
            .as_str()
            .unwrap_or_default()
            .contains("nothing before it"),
        "a script with one revision is not an error, it is a short history: {:?}",
        result
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn the_history_tools_are_exposed_over_mcp() {
    let descriptors = aiwebengine::engine_api::native_mcp_tool_descriptors();
    for name in [
        "list_revisions",
        "revert_script",
        "diff_revisions",
        "label_revision",
    ] {
        assert!(
            descriptors.iter().any(|tool| tool.name == name),
            "an agent editing without a checkout cannot use '{}' it is not told about",
            name
        );
    }
}

// ============================================================================
// Attributing output to a version
// ============================================================================

async fn logs_at(script_uri: &str, revision: Option<i32>) -> Vec<String> {
    let query = aiwebengine::repository::LogQuery {
        script_uri: Some(script_uri.to_string()),
        revision,
        limit: Some(100),
        ..Default::default()
    };
    aiwebengine::engine_api::query_log_entries_authorized(&admin(), &query)
        .expect("reading logs should succeed")
        .into_iter()
        .map(|entry| entry.message)
        .collect()
}

async fn log_line(script_uri: &str, message: &str) {
    let context = aiwebengine::js_engine::HandlerInvocationKind::HttpRoute.log_context(
        script_uri,
        "test-invocation",
        Some("/probe".to_string()),
    );
    use aiwebengine::repository::Repository as _;
    aiwebengine::repository::get_repository()
        .insert_log(script_uri, message, "INFO", &context)
        .await
        .expect("writing a log line should succeed");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_log_line_names_the_revision_that_was_running() {
    setup_env().await;
    let uri = "test://revisions/logs/attribution.ts";
    let (first, second) = deploy_unnoticed(
        uri,
        "function init() {}",
        ("server/a.ts", "export const a = 1;"),
    )
    .await;

    // The second write is what this instance is now running, so a line written
    // now belongs to it rather than to the version it replaced.
    log_line(uri, "after the second write").await;

    let at_second = logs_at(uri, Some(second)).await;
    assert!(
        at_second.contains(&"after the second write".to_string()),
        "the line should be filed under revision {}: {:?}",
        second,
        at_second
    );
    assert!(
        !logs_at(uri, Some(first))
            .await
            .contains(&"after the second write".to_string()),
        "and not under the revision it replaced"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_write_moves_later_output_to_the_new_revision() {
    setup_env().await;
    let uri = "test://revisions/logs/moves.ts";
    let (_, before) = deploy_unnoticed(
        uri,
        "function init() {}",
        ("server/a.ts", "export const a = 1;"),
    )
    .await;

    log_line(uri, "written before the change").await;

    write_asset(uri, "server/a.ts", "export const a = 2;").await;
    let after = aiwebengine::revisions::head(uri)
        .await
        .expect("head should read")
        .expect("the write records a revision");

    log_line(uri, "written after the change").await;

    assert_eq!(
        logs_at(uri, Some(before)).await,
        vec!["written before the change".to_string()],
        "output written under the old version stays with it"
    );
    assert_eq!(
        logs_at(uri, Some(after)).await,
        vec!["written after the change".to_string()],
        "which is what makes 'the errors started at revision {}' answerable",
        after
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_invocation_keeps_the_revision_it_started_under() {
    setup_env().await;
    let uri = "test://revisions/logs/spanning.ts";
    let (_, started_under) = deploy_unnoticed(
        uri,
        "function init() {}",
        ("server/a.ts", "export const a = 1;"),
    )
    .await;

    // A handler resolves its context once, at the start.
    let context = aiwebengine::js_engine::HandlerInvocationKind::HttpRoute.log_context(
        uri,
        "long-running",
        Some("/probe".to_string()),
    );

    // A write lands while it is still running.
    write_asset(uri, "server/a.ts", "export const a = 2;").await;

    use aiwebengine::repository::Repository as _;
    aiwebengine::repository::get_repository()
        .insert_log(uri, "logged after the write landed", "INFO", &context)
        .await
        .expect("writing a log line should succeed");

    assert!(
        logs_at(uri, Some(started_under))
            .await
            .contains(&"logged after the write landed".to_string()),
        "every line an invocation wrote came from the version it started under, \
         whatever was deployed by the time it got there"
    );
}

// ============================================================================
// Schema drift
// ============================================================================

/// Declare a table for a script.
///
/// Writes `script_tables` directly rather than creating a physical table. What
/// a revision records is that metadata, and what a revert compares is that
/// metadata against itself — the physical table is neither read nor touched by
/// any of it, and creating one per test would leave real tables behind in a
/// shared database.
async fn declare_table(script_uri: &str, name: &str, columns: &[&str]) {
    let pool = aiwebengine::database::get_global_database()
        .expect("database should be initialized")
        .pool()
        .clone();

    let schema = serde_json::json!({
        "columns": columns
            .iter()
            .map(|column| serde_json::json!({ "name": column, "type": "TEXT" }))
            .collect::<Vec<_>>()
    });

    sqlx::query(
        "INSERT INTO script_tables (script_uri, logical_table_name, physical_table_name, schema_json)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (script_uri, logical_table_name)
         DO UPDATE SET schema_json = EXCLUDED.schema_json",
    )
    .bind(script_uri)
    .bind(name)
    .bind(format!("t_{}_{}", name, uuid_like(script_uri, name)))
    .bind(schema)
    .execute(&pool)
    .await
    .expect("declaring the table should succeed");
}

async fn drop_declared_table(script_uri: &str, name: &str) {
    let pool = aiwebengine::database::get_global_database()
        .expect("database should be initialized")
        .pool()
        .clone();
    sqlx::query("DELETE FROM script_tables WHERE script_uri = $1 AND logical_table_name = $2")
        .bind(script_uri)
        .bind(name)
        .execute(&pool)
        .await
        .expect("dropping the table should succeed");
}

/// Physical table names are globally unique, so they have to differ per test.
fn uuid_like(script_uri: &str, name: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(script_uri.as_bytes());
    hasher.update(name.as_bytes());
    hex::encode(hasher.finalize())[..12].to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revision_records_the_tables_the_script_had() {
    setup_env().await;
    let uri = "test://revisions/schema/records.ts";
    deploy(uri, "function init() {}").await;
    declare_table(uri, "matches", &["id", "score"]).await;

    let revision = write_asset(uri, "server/a.ts", "export const a = 1;")
        .await
        .expect("write records a revision");

    let recorded = aiwebengine::revisions::schema_at(uri, revision)
        .await
        .expect("schema should read")
        .expect("the revision recorded a fingerprint");

    assert!(
        recorded.get("matches").is_some(),
        "the revision should know the script had a `matches` table: {}",
        recorded
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revert_warns_about_a_column_added_since() {
    setup_env().await;
    let uri = "test://revisions/schema/column.ts";
    deploy(uri, "function init() {}").await;
    declare_table(uri, "matches", &["id"]).await;

    let target = write_asset(uri, "server/a.ts", "export const a = 1;")
        .await
        .expect("write records a revision");

    // The change that followed added a column and the code that reads it.
    declare_table(uri, "matches", &["id", "round"]).await;
    write_asset(uri, "server/a.ts", "export const a = 2;").await;

    let outcome = revert(uri, &target.to_string(), true).await;

    assert!(
        outcome
            .schema_warnings
            .iter()
            .any(|warning| warning.contains("`round`")),
        "a dry run is where this belongs — while there is still the option not \
         to: {:?}",
        outcome.schema_warnings
    );
    // And nothing was done about it.
    let still_there = aiwebengine::revisions::schema_now(uri)
        .await
        .expect("schema should read");
    assert!(
        still_there["matches"]["columns"]
            .as_array()
            .expect("columns")
            .iter()
            .any(|column| column["name"] == "round"),
        "the column stays: dropping it to match old code would destroy data in \
         order to restore code"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revert_warns_when_a_table_the_revision_expects_is_gone() {
    setup_env().await;
    let uri = "test://revisions/schema/missing.ts";
    deploy(uri, "function init() {}").await;
    declare_table(uri, "matches", &["id"]).await;

    let target = write_asset(uri, "server/a.ts", "export const a = 1;")
        .await
        .expect("write records a revision");

    drop_declared_table(uri, "matches").await;
    write_asset(uri, "server/a.ts", "export const a = 2;").await;

    let outcome = revert(uri, &target.to_string(), true).await;

    assert!(
        outcome
            .schema_warnings
            .iter()
            .any(|warning| warning.contains("does not now")),
        "restoring modules that read a table which is gone is the case worth \
         hearing about: {:?}",
        outcome.schema_warnings
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revert_onto_the_same_schema_says_nothing_about_it() {
    setup_env().await;
    let uri = "test://revisions/schema/quiet.ts";
    deploy(uri, "function init() {}").await;
    declare_table(uri, "matches", &["id"]).await;

    let target = write_asset(uri, "server/a.ts", "export const a = 1;")
        .await
        .expect("write records a revision");
    write_asset(uri, "server/a.ts", "export const a = 2;").await;

    let outcome = revert(uri, &target.to_string(), true).await;

    assert!(
        outcome.schema_warnings.is_empty(),
        "the data is the one this revision ran against: {:?}",
        outcome.schema_warnings
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_backfilled_baseline_records_the_schema_it_found() {
    setup_env().await;
    let uri = "test://revisions/schema/backfill.ts";
    clear_script_state(uri).await;

    let pool = aiwebengine::database::get_global_database()
        .expect("database should be initialized")
        .pool()
        .clone();
    sqlx::query("INSERT INTO scripts (uri, content, name) VALUES ($1, 'function init() {}', $2)")
        .bind(uri)
        .bind("backfill.ts")
        .execute(&pool)
        .await
        .expect("storing the script should succeed");
    declare_table(uri, "matches", &["id"]).await;

    // Scoped to this script. The unscoped pass is what startup runs, and
    // calling it here would hand a baseline to every other test's script in
    // the moment between its own write and its own recording — which is a race
    // this test would be creating rather than observing.
    aiwebengine::revisions::backfill_missing(Some(uri)).await;

    let recorded = aiwebengine::revisions::schema_at(uri, 1)
        .await
        .expect("schema should read")
        .expect("the baseline recorded a fingerprint");

    assert!(
        recorded.get("matches").is_some(),
        "a baseline with no fingerprint would tell every revert to it that the \
         comparison was impossible, which is the one answer nobody can act on: {}",
        recorded
    );

    clear_script_state(uri).await;
}

// ============================================================================
// Deployments
// ============================================================================

async fn deploy_revision(script_uri: &str, revision: i32) {
    aiwebengine::deployments::deploy(script_uri, revision, Some("deployer"))
        .await
        .unwrap_or_else(|e| panic!("deploying should succeed: {:?}", e));
}

/// What the engine would run for this script, as its own module resolution
/// sees it: the served root bundled against the modules that root belongs to.
async fn serving_program(script_uri: &str) -> Result<String, String> {
    let view = aiwebengine::deployments::serving_view(script_uri);
    let root = aiwebengine::repository::read_served_source(script_uri)
        .await
        .ok_or_else(|| "no source".to_string())?;
    let uri = script_uri.to_string();
    tokio::task::spawn_blocking(move || {
        aiwebengine::module_loader::prepare_executable_program_in(&uri, &root, &view)
            .map(|prepared| prepared.code)
            .map_err(|e| format!("{:?}", e))
    })
    .await
    .expect("join")
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unpinned_script_serves_whatever_was_written_last() {
    setup_env().await;
    let uri = "test://deployments/follow/main.ts";
    deploy_over_fresh(uri, "function init() { return 1; }\n").await;

    deploy_over(uri, "function init() { return 2; }\n").await;

    assert!(
        serving_program(uri)
            .await
            .expect("the program builds")
            .contains("return 2"),
        "with no pin, a write is a deployment — which is what every script \
         does today and must keep doing"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_write_to_a_pinned_script_does_not_change_what_it_serves() {
    setup_env().await;
    let uri = "test://deployments/pinned/main.ts";
    deploy_over_fresh(uri, "function init() { return 1; }\n").await;
    let deployed = aiwebengine::revisions::head(uri)
        .await
        .expect("head should read")
        .expect("deploying records a revision");

    deploy_revision(uri, deployed).await;

    // The change lands, and head moves.
    deploy_over(uri, "function init() { return 2; }\n").await;
    let head = aiwebengine::revisions::head(uri)
        .await
        .expect("head should read")
        .expect("the write records a revision");
    assert!(head > deployed, "the write still records a revision");

    assert!(
        serving_program(uri)
            .await
            .expect("the program builds")
            .contains("return 1"),
        "the deployment is what is served; the write is history until someone \
         deploys it"
    );

    aiwebengine::deployments::unpin(uri).await.expect("unpin");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pinned_script_resolves_imports_from_its_own_revision() {
    setup_env().await;
    let uri = "test://deployments/modules/main.ts";
    let root = "import { rate } from './server/rate.ts';\nfunction init() { return rate(); }";
    deploy_over_fresh(uri, root).await;
    write_asset(
        uri,
        "server/rate.ts",
        "export function rate() { return 1; }",
    )
    .await;

    let deployed = aiwebengine::revisions::head(uri)
        .await
        .expect("head should read")
        .expect("the write records a revision");
    deploy_revision(uri, deployed).await;

    // Head loses the module the deployed root imports.
    delete_asset(uri, "server/rate.ts").await;

    assert!(
        serving_program(uri).await.is_ok(),
        "a deployment is a whole version: its root and the modules that root \
         was written against, not one of each"
    );

    aiwebengine::deployments::unpin(uri).await.expect("unpin");
}

#[tokio::test(flavor = "multi_thread")]
async fn deploying_head_takes_the_writes_that_accumulated() {
    setup_env().await;
    let uri = "test://deployments/take/main.ts";
    deploy_over_fresh(uri, "function init() { return 1; }\n").await;
    let first = aiwebengine::revisions::head(uri)
        .await
        .expect("head should read")
        .expect("deploying records a revision");
    deploy_revision(uri, first).await;

    deploy_over(uri, "function init() { return 2; }\n").await;
    let head = aiwebengine::revisions::head(uri)
        .await
        .expect("head should read")
        .expect("the write records a revision");

    deploy_revision(uri, head).await;
    aiwebengine::repository::refresh_served_source(uri).await;

    assert!(
        serving_program(uri)
            .await
            .expect("the program builds")
            .contains("return 2"),
        "the accumulated writes are taken when, and only when, they are deployed"
    );

    aiwebengine::deployments::unpin(uri).await.expect("unpin");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pin_names_a_number_rather_than_following_head() {
    setup_env().await;
    let uri = "test://deployments/number/main.ts";
    deploy_over_fresh(uri, "function init() { return 1; }\n").await;
    let head = aiwebengine::revisions::head(uri)
        .await
        .expect("head should read")
        .expect("deploying records a revision");

    deploy_revision(uri, head).await;
    deploy_over(uri, "function init() { return 2; }\n").await;

    let deployment = aiwebengine::deployments::get(uri)
        .await
        .expect("deployment should read")
        .expect("the script is pinned");
    assert_eq!(
        deployment.revision, head,
        "a pin that meant 'newest' would move with the next write, which is \
         the one thing it exists not to do"
    );

    aiwebengine::deployments::unpin(uri).await.expect("unpin");
}

#[tokio::test(flavor = "multi_thread")]
async fn retention_will_not_collect_the_revision_a_script_serves() {
    setup_env().await;
    let uri = "test://deployments/retention/main.ts";
    deploy_over_fresh(uri, "function init() { return 1; }\n").await;
    let deployed = aiwebengine::revisions::head(uri)
        .await
        .expect("head should read")
        .expect("deploying records a revision");
    deploy_revision(uri, deployed).await;

    for value in 2..=6 {
        write_asset(uri, "server/a.ts", &format!("export const a = {};", value)).await;
    }
    age_revisions(uri, 90).await;

    aiwebengine::revisions::prune(Some(uri), KEEP_NOTHING)
        .await
        .expect("prune should succeed");

    assert!(
        revision_numbers(uri).await.contains(&deployed),
        "collecting what a script is serving would leave it running code with \
         no history behind it"
    );

    aiwebengine::deployments::unpin(uri).await.expect("unpin");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_deployment_tools_are_exposed_over_mcp() {
    let descriptors = aiwebengine::engine_api::native_mcp_tool_descriptors();
    for name in ["deploy_script", "get_deployment"] {
        assert!(
            descriptors.iter().any(|tool| tool.name == name),
            "an agent that can write a script but not choose what it serves is \
             back to every write being a deployment: '{}' missing",
            name
        );
    }
}
