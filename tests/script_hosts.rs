//! Tests for binding a script's registrations to hostnames.
//!
//! A script publishes on the default host unless an administrator binds it
//! elsewhere, so an admin-only script can be confined to the management host
//! while a shared page answers on all of them. These cover the authorization
//! layer both entry points (`/engine/script_hosts` and the `get_script_hosts` /
//! `set_script_hosts` MCP tools) call into, plus the storage round trip.

mod common;

use aiwebengine::engine_api::{
    ScriptHostError, execute_native_mcp_tool, get_script_hosts_authorized,
    native_mcp_tool_descriptors, set_script_hosts_authorized,
};
use aiwebengine::hosts::{self, ALL_HOSTS, HostConfig};
use aiwebengine::mcp;
use aiwebengine::repository;
use aiwebengine::security::{Capability, UserContext};
use common::should_skip_integration_tests;
use serde_json::json;
use tokio::sync::OnceCell;

static INIT: OnceCell<()> = OnceCell::const_new();

async fn setup_env() {
    INIT.get_or_init(|| async {
        let config = aiwebengine::config::AppConfig::test_config_postgres(0);
        if let Ok(db) = aiwebengine::database::Database::new(&config.repository).await {
            let db_arc = std::sync::Arc::new(db);
            aiwebengine::database::initialize_global_database(db_arc.clone());
            let repo = aiwebengine::repository::PostgresRepository::new(
                db_arc.pool().clone(),
                "test".to_string(),
            );
            aiwebengine::repository::initialize_repository(repo);
        }

        // The hosts this engine serves. Set once per test process; every test
        // here uses the same three so ordering does not matter.
        hosts::init(HostConfig::new(
            "https://softagen.com",
            &[
                "https://manage.softagen.com".to_string(),
                "https://world.softagen.com".to_string(),
            ],
        ));
    })
    .await;
}

fn admin() -> UserContext {
    UserContext::admin("test-admin".to_string())
}

/// A script to bind, unique per call so tests stay independent.
fn create_script(label: &str) -> String {
    let uri = format!("test://hosts-{}-{}", label, uuid::Uuid::new_v4());
    repository::upsert_script(&uri, "// host binding test").expect("script should be created");
    uri
}

#[tokio::test(flavor = "multi_thread")]
async fn unbound_scripts_publish_on_the_default_host() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;
    let uri = create_script("default");

    let (stored, effective) =
        get_script_hosts_authorized(&admin(), &uri).expect("admin may read bindings");

    assert!(stored.is_empty(), "a new script stores no binding");
    assert_eq!(
        effective,
        vec!["softagen.com".to_string()],
        "which resolves to the default host"
    );

    repository::delete_script(&uri);
}

#[tokio::test(flavor = "multi_thread")]
async fn binding_to_one_host_round_trips() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;
    let uri = create_script("manage");

    let (stored, effective) =
        set_script_hosts_authorized(&admin(), &uri, &["manage.softagen.com".to_string()])
            .expect("admin may set bindings");
    assert_eq!(stored, vec!["manage.softagen.com".to_string()]);
    assert_eq!(effective, vec!["manage.softagen.com".to_string()]);

    // Read back through a separate call, so this covers storage rather than
    // the value the setter happened to return.
    let (stored, _) = get_script_hosts_authorized(&admin(), &uri).expect("admin may read bindings");
    assert_eq!(stored, vec!["manage.softagen.com".to_string()]);

    repository::delete_script(&uri);
}

#[tokio::test(flavor = "multi_thread")]
async fn wildcard_binding_resolves_to_every_served_host() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;
    let uri = create_script("about");

    let (stored, effective) = set_script_hosts_authorized(&admin(), &uri, &[ALL_HOSTS.to_string()])
        .expect("admin may set bindings");

    // Stored as the wildcard, not expanded, so the binding keeps following the
    // configured set as hosts are added or removed.
    assert_eq!(stored, vec![ALL_HOSTS.to_string()]);
    assert_eq!(effective, hosts::all_hosts());
    assert_eq!(effective.len(), 3);

    repository::delete_script(&uri);
}

#[tokio::test(flavor = "multi_thread")]
async fn clearing_a_binding_returns_the_script_to_the_default_host() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;
    let uri = create_script("cleared");

    set_script_hosts_authorized(&admin(), &uri, &["world.softagen.com".to_string()])
        .expect("admin may set bindings");
    let (stored, effective) =
        set_script_hosts_authorized(&admin(), &uri, &[]).expect("admin may clear bindings");

    assert!(stored.is_empty());
    assert_eq!(effective, vec!["softagen.com".to_string()]);

    repository::delete_script(&uri);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_host_the_engine_does_not_serve_is_rejected() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;
    let uri = create_script("unknown-host");

    // Storing this would silently take the script's registrations offline.
    let error = set_script_hosts_authorized(&admin(), &uri, &["typo.softagen.com".to_string()])
        .expect_err("an unserved host must be refused");
    let ScriptHostError::Validation(message) = error else {
        panic!("expected a validation error naming the served hosts");
    };
    assert!(
        message.contains("typo.softagen.com") && message.contains("manage.softagen.com"),
        "the message should name both the bad host and the served ones: {}",
        message
    );

    // And nothing was stored.
    let (stored, _) = get_script_hosts_authorized(&admin(), &uri).expect("admin may read bindings");
    assert!(stored.is_empty());

    repository::delete_script(&uri);
}

#[tokio::test(flavor = "multi_thread")]
async fn binding_an_unknown_script_is_not_found() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    assert!(matches!(
        set_script_hosts_authorized(&admin(), "test://no-such-script", &[]),
        Err(ScriptHostError::ScriptNotFound(_))
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn host_bindings_are_denied_to_non_administrators() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;
    let uri = create_script("denied");

    for user in [
        UserContext::anonymous(),
        UserContext::authenticated("plain-user".to_string()),
    ] {
        assert!(
            matches!(
                get_script_hosts_authorized(&user, &uri),
                Err(ScriptHostError::AccessDenied)
            ),
            "reading where a script is published is administrator-only"
        );
        assert!(
            matches!(
                set_script_hosts_authorized(&user, &uri, &["manage.softagen.com".to_string()]),
                Err(ScriptHostError::AccessDenied)
            ),
            "moving a script between hosts is administrator-only"
        );
    }

    repository::delete_script(&uri);
}

/// Development mode grants anonymous callers `DeleteScripts`, the engine's
/// usual stand-in for "is an admin". Host binding must not accept that: an
/// unauthenticated caller could otherwise republish a script onto the
/// management host.
#[tokio::test(flavor = "multi_thread")]
async fn host_bindings_reject_an_unauthenticated_holder_of_delete_scripts() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;
    let uri = create_script("dev-anonymous");

    let dev_anonymous = UserContext {
        user_id: None,
        is_authenticated: false,
        capabilities: [Capability::DeleteScripts, Capability::ReadScripts]
            .into_iter()
            .collect(),
    };

    assert!(matches!(
        set_script_hosts_authorized(&dev_anonymous, &uri, &["manage.softagen.com".to_string()]),
        Err(ScriptHostError::AccessDenied)
    ));

    repository::delete_script(&uri);
}

#[tokio::test(flavor = "multi_thread")]
async fn ownership_alone_does_not_allow_republishing_a_script() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    // Owning a script lets you edit it; it must not let you move it onto the
    // management host, which is why this is an administrator's call.
    let uri = format!("test://hosts-owned-{}", uuid::Uuid::new_v4());
    repository::upsert_script_with_owner(&uri, "// owned", Some("owner-user"))
        .expect("script should be created");

    let owner = UserContext::authenticated("owner-user".to_string());
    assert!(matches!(
        set_script_hosts_authorized(&owner, &uri, &["manage.softagen.com".to_string()]),
        Err(ScriptHostError::AccessDenied)
    ));

    repository::delete_script(&uri);
}

#[tokio::test(flavor = "multi_thread")]
async fn host_tools_are_exposed_over_mcp() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let names: Vec<&str> = native_mcp_tool_descriptors()
        .into_iter()
        .map(|descriptor| descriptor.name)
        .collect();
    assert!(names.contains(&"get_script_hosts"));
    assert!(names.contains(&"set_script_hosts"));
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_set_script_hosts_binds_for_an_admin() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;
    let uri = create_script("mcp");

    let result = execute_native_mcp_tool(
        "set_script_hosts",
        &json!({ "uri": uri, "hosts": ["world.softagen.com"] }),
        &admin(),
    )
    .expect("set_script_hosts is a native tool");

    assert_eq!(result.get("success"), Some(&json!(true)));
    assert_eq!(
        result.get("publishedOn"),
        Some(&json!(["world.softagen.com"]))
    );

    let (stored, _) = get_script_hosts_authorized(&admin(), &uri).expect("admin may read bindings");
    assert_eq!(stored, vec!["world.softagen.com".to_string()]);

    repository::delete_script(&uri);
}

/// The engine's own MCP tools are the same management surface as `/engine/*`,
/// so they follow the same `server.management_hosts` setting rather than being
/// reachable from every host's `/mcp`.
#[tokio::test(flavor = "multi_thread")]
async fn native_mcp_tools_are_listed_only_where_management_is_allowed() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let listed = mcp::list_tools_for_host("manage.softagen.com", true).await;
    assert!(
        listed.iter().any(|tool| tool.name == "write_file"),
        "a management host must still offer the engine's tools"
    );

    let listed = mcp::list_tools_for_host("softagen.com", false).await;
    for name in [
        "write_file",
        "delete_file",
        "set_script_hosts",
        "list_users",
    ] {
        assert!(
            !listed.iter().any(|tool| tool.name == name),
            "{} must not be listed on a content host",
            name
        );
    }
    assert!(
        listed
            .iter()
            .all(|tool| tool.script_uri != mcp::NATIVE_TOOL_URI),
        "no native tool should survive the filter on a content host"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn native_mcp_tools_are_refused_at_dispatch_off_the_management_host() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    // Listing and dispatch must agree; a client that learned the name
    // elsewhere must not get through by naming it.
    for name in [
        "write_file",
        "delete_file",
        "set_script_hosts",
        "list_users",
    ] {
        assert!(
            mcp::tool_is_available_on_host(name, "manage.softagen.com", true).await,
            "{} should be callable on a management host",
            name
        );
        assert!(
            !mcp::tool_is_available_on_host(name, "softagen.com", false).await,
            "{} should be refused on a content host",
            name
        );
    }
}

/// Native tools win at dispatch, so a script registering a colliding name must
/// not be able to decide — via its own host binding — whether the native tool
/// runs. Without this the gate above could be walked around by publishing a
/// script named after a management tool on a content host.
#[tokio::test(flavor = "multi_thread")]
async fn a_script_cannot_reopen_a_native_tool_by_registering_its_name() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;

    let uri = create_script("collide");
    aiwebengine::mcp::register_mcp_tool(
        "write_file".to_string(),
        "impostor".to_string(),
        json!({ "type": "object" }),
        "handler".to_string(),
        uri.clone(),
    );

    // The script publishes on the default host, but the name is native, so the
    // management-host rule decides and the call is still refused there.
    assert!(
        !mcp::tool_is_available_on_host("write_file", "softagen.com", false).await,
        "a colliding script registration must not reopen the native tool"
    );

    aiwebengine::mcp::clear_script_mcp_registrations(&uri);
    repository::delete_script(&uri);
}

#[tokio::test(flavor = "multi_thread")]
async fn mcp_host_tools_enforce_admin() {
    if should_skip_integration_tests() {
        return;
    }
    setup_env().await;
    let uri = create_script("mcp-denied");

    let caller = UserContext::authenticated("plain-user".to_string());
    for tool in ["get_script_hosts", "set_script_hosts"] {
        let result = execute_native_mcp_tool(
            tool,
            &json!({ "uri": uri, "hosts": ["manage.softagen.com"] }),
            &caller,
        )
        .expect("tool should exist");
        let message = result
            .get("error")
            .and_then(|error| error.as_str())
            .unwrap_or_default();
        assert!(
            message.contains("Administrator"),
            "{} should refuse a non-admin, got {:?}",
            tool,
            result
        );
    }

    repository::delete_script(&uri);
}
