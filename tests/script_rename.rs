//! A script's name is something it has, not something it is.
//!
//! Renaming is one `UPDATE scripts SET uri = ...`: every table that names a
//! script references it with `ON UPDATE CASCADE`, so the files, history, owners,
//! hosts, secrets, storage and queue follow in the same statement. What the
//! database cannot reach — `logs`, which the engine also writes under `server`,
//! and the in-memory state keyed by the name — the rename moves explicitly.

mod common;

use aiwebengine::engine_api::{execute_native_mcp_tool, rename_script_authorized};
use aiwebengine::repository;
use aiwebengine::security::UserContext;
use common::setup_env;
use serde_json::json;

fn name(label: &str) -> String {
    format!("rename-{}-{}", label, uuid::Uuid::new_v4())
}

fn admin() -> UserContext {
    UserContext::admin("renamer".to_string())
}

fn store(uri: &str) {
    repository::upsert_script(uri, "function init() {}\n").expect("script should store");
}

#[tokio::test(flavor = "multi_thread")]
async fn everything_that_names_a_script_follows_its_rename() {
    setup_env().await;
    let from = name("from");
    let to = name("to");
    store(&from);
    repository::add_script_owner(&from, "rename-owner").expect("owner");
    repository::set_script_properties_item(&from, "k", "v").expect("property");
    repository::set_script_secret_item(&from, "s", "secret").expect("secret");
    repository::insert_log_message(&from, "a line the old name wrote", "INFO");
    repository::create_script_table(&from, "notes").expect("table");

    rename_script_authorized(&admin(), &from, &to).expect("the rename should succeed");

    assert!(
        repository::fetch_script(&from).is_none(),
        "the old name is gone"
    );
    assert!(repository::fetch_script(&to).is_some());
    assert_eq!(
        repository::get_script_owners(&to).expect("owners"),
        vec!["rename-owner".to_string()]
    );
    assert_eq!(
        repository::get_script_properties_item(&to, "k").as_deref(),
        Some("v")
    );
    assert_eq!(
        repository::get_script_secret_item(&to, "s").as_deref(),
        Some("secret")
    );
    assert!(
        repository::fetch_log_messages(&to)
            .iter()
            .any(|line| line.message.contains("the old name wrote")),
        "logs have no foreign key, so the rename moves them itself"
    );
    assert!(repository::fetch_log_messages(&from).is_empty());
    assert_eq!(
        repository::list_script_tables(&to).expect("tables").len(),
        1,
        "its tables are its own under the new name"
    );
    assert!(repository::fetch_assets(&to).contains_key("main.js"));

    repository::delete_script(&to);
}

/// The reason physical table names come from `scripts.id`: a script that was
/// renamed away and a new one that takes the old name must not both want the
/// same `script_{hash}_notes`.
#[tokio::test(flavor = "multi_thread")]
async fn a_new_script_can_take_a_name_a_renamed_one_left() {
    setup_env().await;
    let old = name("old");
    let moved = name("moved");
    store(&old);
    let first = repository::create_script_table(&old, "notes").expect("first table");

    rename_script_authorized(&admin(), &old, &moved).expect("rename");

    store(&old);
    let second = repository::create_script_table(&old, "notes")
        .expect("the same logical table under the same name is another script's");
    assert_ne!(first, second, "two scripts, two physical tables");

    // And the renamed script's table is still reachable, under its new name.
    let tables = repository::list_script_tables(&moved).expect("tables");
    assert_eq!(tables.len(), 1);
    assert_eq!(tables[0].physical_name, first);

    repository::delete_script(&old);
    repository::delete_script(&moved);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rename_is_refused_where_it_would_do_harm() {
    setup_env().await;
    let a = name("a");
    let b = name("b");
    store(&a);
    store(&b);

    let taken = rename_script_authorized(&admin(), &a, &b).expect_err("that name is taken");
    assert!(taken.contains("already exists"), "{taken}");

    for bad in [
        "https://example.com/shop",
        "acme/shop",
        "Shop",
        "shop.ts",
        "core",
        "",
    ] {
        assert!(
            rename_script_authorized(&admin(), &a, bad).is_err(),
            "'{bad}' is not a name a script can have"
        );
    }

    let missing = rename_script_authorized(&admin(), &name("nope"), &name("x"))
        .expect_err("there is nothing to rename");
    assert!(missing.contains("not found"), "{missing}");

    assert!(
        repository::fetch_script(&a).is_some(),
        "a refused rename changes nothing"
    );

    // Writing a script takes ownership; renaming it is no different.
    let stranger = UserContext::editor("rename-stranger".to_string());
    assert!(rename_script_authorized(&stranger, &a, &name("mine")).is_err());

    repository::delete_script(&a);
    repository::delete_script(&b);
}

/// A script that exists keeps the name it has, whatever shape it is. The slug
/// rule is for the names scripts are given, not a verdict on the ones they hold.
#[tokio::test(flavor = "multi_thread")]
async fn a_script_with_a_legacy_name_goes_on_working_and_can_move_to_a_slug() {
    setup_env().await;
    let legacy = format!("https://example.com/legacy-{}.ts", uuid::Uuid::new_v4());
    store(&legacy);

    // Writing it is not refused for what it is called.
    aiwebengine::engine_api::upsert_script_authorized(
        &admin(),
        &legacy,
        "function init() {}",
        None,
    )
    .expect("an existing script is written whatever it is called");

    let slug = name("legacy");
    rename_script_authorized(&admin(), &legacy, &slug).expect("and renamed to a slug");
    assert!(repository::fetch_script(&slug).is_some());

    repository::delete_script(&slug);
}

#[tokio::test(flavor = "multi_thread")]
async fn the_operation_is_offered_over_the_tool_table() {
    setup_env().await;
    let from = name("tool");
    let to = name("tool-to");
    store(&from);

    let result =
        execute_native_mcp_tool("rename_script", &json!({ "uri": from, "to": to }), &admin())
            .expect("rename_script is an operation");
    assert_eq!(result["success"], json!(true), "{result}");
    assert_eq!(result["uri"], json!(to));
    assert_eq!(result["renamedFrom"], json!(from));

    repository::delete_script(&to);
}
