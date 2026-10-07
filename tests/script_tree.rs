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
use aiwebengine::security::{Capability, Principal, UserContext};
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

/// `files` is how a script reaches its own files, and merging the tree
/// put the entrypoint among them. Its writes are gated by `WriteAssets` and
/// `DeleteAssets` and by nothing else — a script only ever reaches its own
/// files, so there is no ownership question — which would have let a script
/// rewrite or delete its own program while serving a request from anyone
/// holding the editor tier. It could not do that before the merge, so it does
/// not do it now.
///
/// Asserted from inside the script, which is deployed as the very
/// entrypoint it then tries to overwrite and delete.
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
        const before = files.read("main.ts");
        if (before === null) {
            throw new Error("the entrypoint should be readable");
        }

        function refused(attempt) {
            try {
                attempt();
            } catch (e) {
                return e.message.includes("entrypoint");
            }
            return false;
        }
        if (!refused(() => files.write("main.ts", "export function init() {}"))) {
            throw new Error("files.write should refuse the entrypoint");
        }
        if (!refused(() => files.delete("main.ts"))) {
            throw new Error("files.delete should refuse the entrypoint");
        }

        const after = files.read("main.ts");
        if (after !== before) {
            throw new Error("the entrypoint moved under a refused write");
        }
    "#;

    deploy(uri, attempt);
    let result = execute_script_secure(uri, attempt, Principal::Caller(editor));
    assert!(
        result.success,
        "the entrypoint must survive both: {:?}",
        result.error
    );
}

/// Running a script is not writing it. Execution used to store the source it
/// was handed as the entrypoint, which after the merge meant writing a *file*
/// named from the URI — so every boot rewrote every script, and a TypeScript
/// script whose URI had no extension grew a second entrypoint, `main.js`,
/// holding its TypeScript.
#[tokio::test(flavor = "multi_thread")]
async fn executing_a_script_leaves_its_tree_alone() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "tree-execute-is-read-only";
    let stored = "function init() { /* stored */ }\n";
    let owner = UserContext::admin("tree-exec-owner".to_string());
    aiwebengine::engine_api::upsert_root_authorized(&owner, uri, Some("main.ts"), stored, None)
        .expect("the entrypoint should be written");

    let result = execute_script_secure(
        uri,
        "function init() { /* ran */ }",
        Principal::Caller(owner),
    );
    assert!(result.success, "the script should run: {:?}", result.error);

    let names: Vec<String> = repository::fetch_assets(uri).into_keys().collect();
    assert_eq!(names, vec!["main.ts".to_string()], "no second entrypoint");
    assert_eq!(
        text(uri, "main.ts").as_deref(),
        Some(stored),
        "and no rewrite"
    );
}

/// A file written by name is that file. Writing `main.ts` used to go wherever
/// the tree's existing entrypoint was, or to the name the URI implies — so on
/// a script whose URI carries no extension the TypeScript landed in
/// `main.js` and was run untranspiled.
#[tokio::test(flavor = "multi_thread")]
async fn writing_an_entrypoint_by_name_is_how_its_language_changes() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "tree-rename-entrypoint";
    let owner = UserContext::admin("tree-rename-owner".to_string());
    let js = "function init() {}\n";
    aiwebengine::engine_api::write_file_bytes_authorized(
        &owner,
        uri,
        "main.js",
        "text/javascript",
        js.as_bytes().to_vec(),
        false,
    )
    .expect("main.js should be written");
    assert_eq!(text(uri, "main.js").as_deref(), Some(js));

    let ts = "interface Greeting { text: string }\nfunction init(): void {}\n";
    aiwebengine::engine_api::write_file_bytes_authorized(
        &owner,
        uri,
        "main.ts",
        "text/typescript",
        ts.as_bytes().to_vec(),
        false,
    )
    .expect("main.ts should be written");

    let names: Vec<String> = repository::fetch_assets(uri).into_keys().collect();
    assert_eq!(
        names,
        vec!["main.ts".to_string()],
        "the entrypoint was renamed, not duplicated"
    );
    assert_eq!(text(uri, "main.ts").as_deref(), Some(ts));

    // And it is built as what it now says it is: TypeScript that ran
    // untranspiled would fail on the interface.
    let result = execute_script_secure(uri, ts, Principal::Caller(owner));
    assert!(
        result.success,
        "main.ts should transpile: {:?}",
        result.error
    );
}

/// `files` answers in values, not in sentences: text out, `null` for nothing
/// there, base64 when asked for, and a throw for a file that is not text.
/// Each of these used to be a string the caller had to tell apart from
/// content — `fetchAsset` answered a missing file with a sentence that was
/// not base64, so the decode was the error check.
#[tokio::test(flavor = "multi_thread")]
async fn files_answers_in_values() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://tree/files-values";
    deploy(uri, "function init() {}");
    let now = std::time::SystemTime::now();
    repository::upsert_asset(repository::Asset {
        uri: "public/dot.png".to_string(),
        name: None,
        mimetype: "image/png".to_string(),
        content: vec![0x89, 0x50, 0x4e, 0x47, 0xff, 0xfe],
        created_at: now,
        updated_at: now,
        script_uri: uri.to_string(),
    })
    .expect("binary file should be stored");

    let script = r##"
        function check(condition, what) {
            if (!condition) throw new Error(what);
        }

        files.write("skills/refund.md", "# Refunds\nWithin 30 days.");
        check(files.read("skills/refund.md") === "# Refunds\nWithin 30 days.", "text round trip");
        check(files.read("skills/missing.md") === null, "missing is null");

        const listed = files.list().map((f) => f.path);
        check(JSON.stringify(listed) === JSON.stringify(
            ["main.js", "public/dot.png", "skills/refund.md"]), "sorted listing: " + listed);
        const md = files.list().find((f) => f.path === "skills/refund.md");
        check(md.mimetype === "text/markdown", "mimetype from the extension: " + md.mimetype);

        check(files.read("public/dot.png", { encoding: "base64" }) === "iVBOR//+", "base64 read");
        let threw = null;
        try { files.read("public/dot.png"); } catch (e) { threw = e; }
        check(threw && threw.name === "TypeError", "binary read as text throws a TypeError");

        files.write("public/copy.png", "iVBOR//+", { encoding: "base64" });
        check(files.read("public/copy.png", { encoding: "base64" }) === "iVBOR//+", "base64 write");

        check(files.delete("skills/refund.md") === true, "delete answers true");
        check(files.delete("skills/refund.md") === false, "and false the second time");
    "##;
    let result = execute_script_secure(
        uri,
        script,
        Principal::Caller(UserContext::admin("tree-files".to_string())),
    );
    assert!(
        result.success,
        "files should answer in values: {:?}",
        result.error
    );
}

/// `files` is a common name for a script's own variable, so the global is
/// configurable: a top-level `const files` shadows it rather than being a
/// `SyntaxError` before the script runs a line.
#[tokio::test(flavor = "multi_thread")]
async fn a_script_may_name_its_own_variable_files() {
    let _guard = test_mutex().lock().await;
    setup_env().await;

    let uri = "test://tree/files-shadowed";
    let script = "const files = [1, 2, 3];\nfunction init() {}\n";
    deploy(uri, script);
    let result = execute_script_secure(
        uri,
        script,
        Principal::Caller(UserContext::admin("tree-files".to_string())),
    );
    assert!(
        result.success,
        "a top-level `const files` should load: {:?}",
        result.error
    );
}
