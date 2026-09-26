//! A script and its assets are one tree.
//!
//! The root source used to be `scripts.content`, a column, while every other
//! file of a script was a row of `assets`. These tests are about the seam that
//! is no longer there: the entrypoint is the file named `main.*` in the tree,
//! read, written, listed, versioned and invalidated by exactly the machinery
//! that carries the modules beside it.

mod common;

use aiwebengine::js_engine::execute_script_secure;
use aiwebengine::repository;
use aiwebengine::security::{Capability, UserContext};
use common::{setup_env, test_mutex};

fn deploy(script_uri: &str, content: &str) {
    repository::upsert_script(script_uri, content).expect("script should be stored");
}

fn store(script_uri: &str, path: &str, content: &str) {
    let now = std::time::SystemTime::now();
    repository::upsert_asset(repository::Asset {
        uri: path.to_string(),
        name: Some(path.to_string()),
        mimetype: "text/typescript".to_string(),
        content: content.as_bytes().to_vec(),
        created_at: now,
        updated_at: now,
        script_uri: script_uri.to_string(),
    })
    .expect("file should be stored");
}

fn text(script_uri: &str, path: &str) -> Option<String> {
    repository::fetch_asset(script_uri, path)
        .and_then(|asset| String::from_utf8(asset.content).ok())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_scripts_source_is_a_file_of_its_tree() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://tree/one-tree/app.ts";
    let source = "export function init() { /* one tree */ }\n";
    deploy(uri, source);

    assert_eq!(
        text(uri, "main.ts").as_deref(),
        Some(source),
        "the root is readable by path, like any other file"
    );
    assert!(
        repository::fetch_assets(uri).contains_key("main.ts"),
        "and it is listed with them: a listing that left it out would be a \
         listing of part of the script"
    );
}

/// The extension is the one thing the script URI was ever load-bearing for —
/// [`aiwebengine::transpiler`] reads it and never reads the stem — so a first
/// write carries it over into the file name the tree will keep.
#[tokio::test(flavor = "multi_thread")]
async fn a_first_write_names_the_root_after_the_uris_extension() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    for (uri, expected) in [
        ("test://tree/ext/typescript.ts", "main.ts"),
        ("test://tree/ext/javascript.js", "main.js"),
        ("test://tree/ext/react.tsx", "main.tsx"),
        // Nothing the bundler distinguishes, so plain JavaScript — which is
        // what a script under such a URI already was.
        ("test://tree/ext/nothing", "main.js"),
    ] {
        deploy(uri, "export function init() {}\n");
        let stored: Vec<String> = repository::fetch_assets(uri).into_keys().collect();
        assert_eq!(stored, vec![expected.to_string()], "for {}", uri);
    }
}

/// Writing `main.*` through the ordinary file-write path has to change what
/// the script *runs*, not just what is stored. The in-memory metadata cache
/// holds the root source and is what every execution path reads it from, so a
/// write that invalidated only the module caches would leave requests running
/// the previous entrypoint until the entry aged out — which it never does.
#[tokio::test(flavor = "multi_thread")]
async fn writing_the_root_as_a_file_changes_what_the_script_runs() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://tree/invalidation/app.ts";
    deploy(uri, "export function init() { return 1; }\n");

    // Warm the cache the execution path reads from, so this test is about the
    // write invalidating it rather than about it never having been filled.
    repository::get_script_metadata(uri).expect("metadata should read");

    store(uri, "main.ts", "export function init() { return 2; }\n");

    assert_eq!(
        repository::fetch_script(uri).as_deref(),
        Some("export function init() { return 2; }\n"),
        "the cached source must follow the file that now holds it"
    );
}

/// A tree holding two candidate roots has one root, and it is the first of
/// `ROOT_MODULE_NAMES`. The rule has to be the same everywhere it is asked —
/// of the stored rows here, and of a manifest in `revisions` — or a revision
/// would build from a different entrypoint than the deployment it came from.
#[tokio::test(flavor = "multi_thread")]
async fn two_candidate_roots_resolve_to_one() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://tree/two-roots/app.ts";
    deploy(uri, "export function init() { return \"ts\"; }\n");
    store(
        uri,
        "main.js",
        "export function init() { return \"js\"; }\n",
    );

    assert_eq!(repository::find_root_asset(uri).as_deref(), Some("main.ts"));
    assert_eq!(
        repository::fetch_script(uri).as_deref(),
        Some("export function init() { return \"ts\"; }\n")
    );
}

/// Removing the entrypoint removes the script's source, so it takes what
/// removing a script takes. The write side has always drawn this line —
/// `WriteAssets` is not a way to write a root — and the merge would otherwise
/// be a way around it: what you could not overwrite, you could delete.
#[tokio::test(flavor = "multi_thread")]
async fn deleting_the_entrypoint_takes_more_than_deleting_a_module() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://tree/delete-root/app.ts";
    deploy(uri, "export function init() {}\n");
    store(uri, "lib/helper.ts", "export const helper = 1;\n");
    repository::add_script_owner(uri, "tree-editor").expect("ownership should be recorded");

    let asset_deleter = UserContext {
        user_id: Some("tree-editor".to_string()),
        is_authenticated: true,
        capabilities: [
            Capability::ReadScripts,
            Capability::ReadAssets,
            Capability::WriteAssets,
            Capability::DeleteAssets,
        ]
        .into_iter()
        .collect(),
        attenuated: false,
        network_scope: None,
    };

    let removed =
        aiwebengine::engine_api::delete_asset_authorized(&asset_deleter, uri, "lib/helper.ts");
    assert!(
        matches!(removed, Ok((true, _))),
        "deleting a module takes DeleteAssets, which this caller holds"
    );

    assert!(
        aiwebengine::engine_api::delete_asset_authorized(&asset_deleter, uri, "main.ts").is_err(),
        "DeleteAssets must not be a way to delete a script's source"
    );
    assert_eq!(
        text(uri, "main.ts").as_deref(),
        Some("export function init() {}\n"),
        "and nothing should have been removed"
    );
}

/// `assetStorage` is how a script reaches its own files, and merging the tree
/// put the entrypoint among them. Its writes are gated by `WriteAssets` and
/// `DeleteAssets` and by nothing else — a script only ever reaches its own
/// files, so there is no ownership question — which would have let a script
/// rewrite or delete its own program while serving a request from anyone
/// holding the editor tier. It could not do that before the merge, so it does
/// not do it now.
///
/// Asserted from inside the script, because `execute_script_secure` stores
/// the source it is given: the entrypoint this script would be overwriting is
/// the very snippet running, so the check has to happen while it runs.
#[tokio::test(flavor = "multi_thread")]
async fn a_script_cannot_rewrite_its_own_entrypoint_through_asset_storage() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://tree/self-write/app.ts";

    // An editor: everything an author holds, which is the most anyone
    // visiting a script's route brings with them.
    let editor = UserContext {
        user_id: Some("tree-editor-2".to_string()),
        is_authenticated: true,
        capabilities: [
            Capability::ReadScripts,
            Capability::ReadAssets,
            Capability::WriteScripts,
            Capability::WriteAssets,
            Capability::DeleteAssets,
            Capability::DeleteScripts,
        ]
        .into_iter()
        .collect(),
        attenuated: false,
        network_scope: None,
    };

    let attempt = r#"
        const before = assetStorage.fetchAsset("main.ts");
        if (before.startsWith("Error:") || before === "Asset 'main.ts' not found") {
            throw new Error("the entrypoint should be readable: " + before);
        }

        const wrote = assetStorage.upsertAsset(
            "main.ts",
            "text/typescript",
            "ZXhwb3J0IGZ1bmN0aW9uIGluaXQoKSB7fQ==",
        );
        if (!wrote.startsWith("Error:")) {
            throw new Error("upsertAsset should be refused, got: " + wrote);
        }

        const removed = assetStorage.deleteAsset("main.ts");
        if (!removed.startsWith("Error:")) {
            throw new Error("deleteAsset should be refused, got: " + removed);
        }

        const after = assetStorage.fetchAsset("main.ts");
        if (after !== before) {
            throw new Error("the entrypoint moved under a refused write");
        }
    "#;

    let result = execute_script_secure(uri, attempt, editor);
    assert!(
        result.success,
        "the entrypoint must survive both: {:?}",
        result.error
    );
}
