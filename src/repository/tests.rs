use super::*;
use std::sync::{Arc, Once, OnceLock};

static INIT: Once = Once::new();
static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fn setup_db() {
    INIT.call_once(|| {
        // Skip when the database server will not answer.
        let Some(url) = crate::test_db::connection_string_blocking() else {
            return;
        };

        let pool = sqlx::PgPool::connect_lazy(url).unwrap();
        let db = Arc::new(crate::database::Database::from_pool(pool.clone()));
        crate::database::initialize_global_database(db);

        // Generate and initialize server ID
        let server_id = crate::notifications::generate_server_id();
        crate::notifications::initialize_server_id(server_id.clone());

        // Initialize PostgresRepository with pool and server_id
        let repo = crate::repository::PostgresRepository::new(pool, server_id);
        crate::repository::initialize_repository(repo);
    });
}

fn get_runtime() -> &'static tokio::runtime::Runtime {
    RUNTIME.get_or_init(|| tokio::runtime::Runtime::new().unwrap())
}

// Helper to check if we should skip database-dependent tests
fn should_skip_db_tests() -> bool {
    crate::test_db::connection_string_blocking().is_none()
}

fn initialized_metadata_with_route(uri: &str, content: &str) -> ScriptMetadata {
    let mut metadata = ScriptMetadata::new(uri.to_string(), content.to_string());
    let mut registrations = RouteRegistrations::new();
    registrations.insert(
        ("/keep-me".to_string(), "GET".to_string()),
        RouteMetadata::simple("keepMeHandler".to_string()),
    );
    metadata.mark_initialized_with_registrations(registrations);
    metadata
}

#[test]
fn update_content_keeps_routes_serving_until_reinit() {
    let mut metadata = initialized_metadata_with_route("test://redeploy", "v1");
    metadata.init_error = Some("previous failure".to_string());

    metadata.update_content("v2".to_string());

    assert_eq!(metadata.content, "v2", "should serve the new source");
    assert!(
        metadata.initialized,
        "routes must keep serving while the re-init runs"
    );
    assert!(
        metadata
            .registrations
            .contains_key(&("/keep-me".to_string(), "GET".to_string())),
        "upserting source must not drop the route table"
    );
    assert!(
        metadata.init_error.is_none(),
        "the error describes the previous source"
    );
}

/// `update_script_init_status` only touches the in-memory metadata map, so a
/// lazily-connected pool is enough — nothing in these tests reaches the
/// database.
fn metadata_only_repository() -> PostgresRepository {
    let pool = sqlx::PgPool::connect_lazy("postgresql://unused@localhost/unused")
        .expect("lazy pool should be constructible without connecting");
    PostgresRepository::new(pool, "test".to_string())
}

fn route_table(handler: &str) -> RouteRegistrations {
    let mut registrations = RouteRegistrations::new();
    registrations.insert(
        ("/probe".to_string(), "GET".to_string()),
        RouteMetadata::simple(handler.to_string()),
    );
    registrations
}

fn seed_metadata(uri: &str, metadata: ScriptMetadata) {
    safe_lock_scripts()
        .expect("scripts lock")
        .insert(uri.to_string(), metadata);
}

fn seeded_metadata(uri: &str) -> ScriptMetadata {
    safe_lock_scripts()
        .expect("scripts lock")
        .get(uri)
        .cloned()
        .expect("metadata should still be cached")
}

fn handler_at(metadata: &ScriptMetadata, path: &str, method: &str) -> Option<String> {
    metadata
        .registrations
        .get(&(path.to_string(), method.to_string()))
        .map(|route| route.handler_name.clone())
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_init_keeps_the_route_table_that_is_already_serving() {
    let uri = "test://failed-init-keeps-table";
    let mut metadata = ScriptMetadata::new(uri.to_string(), "v1".to_string());
    metadata.mark_initialized_with_registrations(route_table("v1Handler"));
    seed_metadata(uri, metadata);

    metadata_only_repository()
        .update_script_init_status(
            uri,
            false,
            Some("init threw".to_string()),
            // Partial registrations from the failed attempt
            Some(route_table("v2Handler")),
        )
        .await
        .expect("status update should succeed");

    let metadata = seeded_metadata(uri);
    assert_eq!(
        handler_at(&metadata, "/probe", "GET").as_deref(),
        Some("v1Handler"),
        "a partial table from a failed init must not replace one that works"
    );
    assert!(metadata.initialized, "routes must keep serving");
    assert!(metadata.init_error.is_some(), "failure must be recorded");
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_init_installs_partial_routes_when_nothing_is_serving() {
    let uri = "test://failed-init-installs-partial";
    seed_metadata(uri, ScriptMetadata::new(uri.to_string(), "v1".to_string()));

    metadata_only_repository()
        .update_script_init_status(
            uri,
            false,
            Some("init threw after registering".to_string()),
            Some(route_table("registeredBeforeFailing")),
        )
        .await
        .expect("status update should succeed");

    let metadata = seeded_metadata(uri);
    assert_eq!(
        handler_at(&metadata, "/probe", "GET").as_deref(),
        Some("registeredBeforeFailing"),
        "routes registered before the failure are better than no routes"
    );
    assert!(
        metadata.initialized,
        "routing skips scripts that are not marked initialized"
    );
    assert!(metadata.init_error.is_some(), "failure must be recorded");
}

#[tokio::test(flavor = "multi_thread")]
async fn successful_init_replaces_the_previous_route_table() {
    let uri = "test://successful-init-replaces-table";
    let mut metadata = ScriptMetadata::new(uri.to_string(), "v1".to_string());
    metadata.mark_initialized_with_registrations(route_table("v1Handler"));
    seed_metadata(uri, metadata);

    metadata_only_repository()
        .update_script_init_status(uri, true, None, Some(route_table("v2Handler")))
        .await
        .expect("status update should succeed");

    let metadata = seeded_metadata(uri);
    assert_eq!(
        handler_at(&metadata, "/probe", "GET").as_deref(),
        Some("v2Handler"),
        "a successful init swaps in the new table"
    );
    assert!(metadata.initialized);
    assert!(metadata.init_error.is_none());
    assert!(metadata.last_init_time.is_some());
}

#[test]
fn update_content_on_never_initialized_script_registers_nothing() {
    let mut metadata = ScriptMetadata::new("test://fresh".to_string(), "v1".to_string());

    metadata.update_content("v2".to_string());

    assert!(!metadata.initialized);
    assert!(metadata.registrations.is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_script_operations() {
    if should_skip_db_tests() {
        return;
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();
    let uri = "test://example";
    let content = "console.log('test')";

    // Test upsert
    assert!(upsert_script(uri, content).is_ok());

    // Test fetch
    let fetched = fetch_script(uri);
    assert_eq!(fetched, Some(content.to_string()));

    // Test delete
    assert!(delete_script(uri));
    assert!(!delete_script(uri)); // Should return false for non-existent
}

#[tokio::test(flavor = "multi_thread")]
async fn test_script_properties_operations() {
    if should_skip_db_tests() {
        return;
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();
    let script_uri = "test://storage-script";
    upsert_script(script_uri, "// test").expect("script should store");
    let key = "test_key";
    let value = "test_value";

    // Test set item
    assert!(set_script_properties_item(script_uri, key, value).is_ok());

    // Test get item
    let retrieved = get_script_properties_item(script_uri, key);
    assert_eq!(retrieved, Some(value.to_string()));

    // Test remove item
    assert!(remove_script_properties_item(script_uri, key));

    // Verify item is gone
    let retrieved_after_remove = get_script_properties_item(script_uri, key);
    assert_eq!(retrieved_after_remove, None);

    // Test clear storage
    assert!(set_script_properties_item(script_uri, "key1", "value1").is_ok());
    assert!(set_script_properties_item(script_uri, "key2", "value2").is_ok());

    assert!(clear_script_properties(script_uri).is_ok());

    // Verify both items are gone
    assert_eq!(get_script_properties_item(script_uri, "key1"), None);
    assert_eq!(get_script_properties_item(script_uri, "key2"), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_script_properties_validation() {
    if should_skip_db_tests() {
        return;
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();
    // Test empty script URI
    assert!(set_script_properties_item("", "key", "value").is_err());

    // Test empty key
    assert!(set_script_properties_item("test://script", "", "value").is_err());

    // Test oversized value (simulate by creating a large string)
    let large_value = "x".repeat(1_000_001); // Just over 1MB
    assert!(set_script_properties_item("test://script", "key", &large_value).is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_user_properties_operations() {
    if should_skip_db_tests() {
        return;
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();
    let script_uri = "test://personal-storage-script";
    upsert_script(script_uri, "// test").expect("script should store");
    let user_id_1 = "user123";
    let user_id_2 = "user456";
    let key = "test_key";
    let value1 = "test_value_1";
    let value2 = "test_value_2";

    // Test set item for user 1
    assert!(set_user_properties_item(script_uri, user_id_1, key, value1).is_ok());

    // Test get item for user 1
    let retrieved1 = get_user_properties_item(script_uri, user_id_1, key);
    assert_eq!(retrieved1, Some(value1.to_string()));

    // Test set item for user 2 (same script, same key, different user)
    assert!(set_user_properties_item(script_uri, user_id_2, key, value2).is_ok());

    // Test get item for user 2
    let retrieved2 = get_user_properties_item(script_uri, user_id_2, key);
    assert_eq!(retrieved2, Some(value2.to_string()));

    // Verify user 1's data is still separate
    let still_user1 = get_user_properties_item(script_uri, user_id_1, key);
    assert_eq!(still_user1, Some(value1.to_string()));

    // Test remove item for user 1
    assert!(remove_user_properties_item(script_uri, user_id_1, key));

    // Verify item is gone for user 1
    let retrieved_after_remove = get_user_properties_item(script_uri, user_id_1, key);
    assert_eq!(retrieved_after_remove, None);

    // Verify user 2's data is still there
    let user2_still_there = get_user_properties_item(script_uri, user_id_2, key);
    assert_eq!(user2_still_there, Some(value2.to_string()));

    // Test clear storage for user 2
    assert!(set_user_properties_item(script_uri, user_id_2, "key1", "value1").is_ok());
    assert!(set_user_properties_item(script_uri, user_id_2, "key2", "value2").is_ok());

    assert!(clear_user_properties(script_uri, user_id_2).is_ok());

    // Verify all items are gone for user 2
    assert_eq!(get_user_properties_item(script_uri, user_id_2, key), None);
    assert_eq!(
        get_user_properties_item(script_uri, user_id_2, "key1"),
        None
    );
    assert_eq!(
        get_user_properties_item(script_uri, user_id_2, "key2"),
        None
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_user_properties_validation() {
    if should_skip_db_tests() {
        return;
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();
    let script_uri = "test://script";
    let user_id = "user123";

    // Test empty script URI
    assert!(set_user_properties_item("", user_id, "key", "value").is_err());

    // Test empty user ID
    assert!(set_user_properties_item(script_uri, "", "key", "value").is_err());

    // Test empty key
    assert!(set_user_properties_item(script_uri, user_id, "", "value").is_err());

    // Test oversized value (simulate by creating a large string)
    let large_value = "x".repeat(1_000_001); // Just over 1MB
    assert!(set_user_properties_item(script_uri, user_id, "key", &large_value).is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_user_properties_user_isolation() {
    if should_skip_db_tests() {
        return;
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();
    let script_uri = "test://isolation-test";
    upsert_script(script_uri, "// test").expect("script should store");
    let user1 = "alice";
    let user2 = "bob";

    // Both users set the same key in the same script
    assert!(set_user_properties_item(script_uri, user1, "pref", "dark").is_ok());
    assert!(set_user_properties_item(script_uri, user2, "pref", "light").is_ok());

    // Each user should see only their own value
    assert_eq!(
        get_user_properties_item(script_uri, user1, "pref"),
        Some("dark".to_string())
    );
    assert_eq!(
        get_user_properties_item(script_uri, user2, "pref"),
        Some("light".to_string())
    );

    // Removing user1's data shouldn't affect user2
    assert!(remove_user_properties_item(script_uri, user1, "pref"));
    assert_eq!(get_user_properties_item(script_uri, user1, "pref"), None);
    assert_eq!(
        get_user_properties_item(script_uri, user2, "pref"),
        Some("light".to_string())
    );

    // Clearing user2's data shouldn't affect anything (since user1 already removed)
    assert!(clear_user_properties(script_uri, user2).is_ok());
    assert_eq!(get_user_properties_item(script_uri, user2, "pref"), None);
    assert_eq!(get_user_properties_item(script_uri, user1, "pref"), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_script_secrets_operations() {
    if should_skip_db_tests() {
        return;
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();
    let script_uri = "test://secrets-script";
    upsert_script(script_uri, "// test").expect("script should store");
    let key = "api_key";
    let value = "super_secret_value";

    // Test set item
    assert!(set_script_secret_item(script_uri, key, value).is_ok());

    // Test get item
    let retrieved = get_script_secret_item(script_uri, key);
    assert_eq!(retrieved, Some(value.to_string()));

    // Test overwrite
    let new_value = "updated_secret";
    assert!(set_script_secret_item(script_uri, key, new_value).is_ok());
    let retrieved_updated = get_script_secret_item(script_uri, key);
    assert_eq!(retrieved_updated, Some(new_value.to_string()));

    // Test remove item
    assert!(remove_script_secret_item(script_uri, key));

    // Verify item is gone
    let retrieved_after_remove = get_script_secret_item(script_uri, key);
    assert_eq!(retrieved_after_remove, None);

    // Test clear secrets
    assert!(set_script_secret_item(script_uri, "key1", "value1").is_ok());
    assert!(set_script_secret_item(script_uri, "key2", "value2").is_ok());

    assert!(clear_script_secrets(script_uri).is_ok());

    // Verify both items are gone
    assert_eq!(get_script_secret_item(script_uri, "key1"), None);
    assert_eq!(get_script_secret_item(script_uri, "key2"), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_script_secrets_validation() {
    if should_skip_db_tests() {
        return;
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();

    // Test empty script URI
    assert!(set_script_secret_item("", "key", "value").is_err());

    // Test empty key
    assert!(set_script_secret_item("test://script", "", "value").is_err());

    // Test oversized value
    let large_value = "x".repeat(1_000_001);
    assert!(set_script_secret_item("test://script", "key", &large_value).is_err());

    // Test secrets are scoped per script_uri
    let script_a = "test://secrets-scope-a";
    let script_b = "test://secrets-scope-b";
    upsert_script(script_a, "// test").expect("script should store");
    upsert_script(script_b, "// test").expect("script should store");
    assert!(set_script_secret_item(script_a, "scope_key", "value_a").is_ok());
    assert_eq!(
        get_script_secret_item(script_b, "scope_key"),
        None,
        "Secret from script_a must not be visible in script_b"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_user_secrets_operations() {
    if should_skip_db_tests() {
        return;
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();
    let script_uri = "test://user-secrets-script";
    upsert_script(script_uri, "// test").expect("script should store");
    let user_id_1 = "user_secrets_123";
    let user_id_2 = "user_secrets_456";
    let key = "token";
    let value1 = "token_for_user1";
    let value2 = "token_for_user2";

    // Test set item for user 1
    assert!(set_user_secret_item(script_uri, user_id_1, key, value1).is_ok());

    // Test get item for user 1
    let retrieved1 = get_user_secret_item(script_uri, user_id_1, key);
    assert_eq!(retrieved1, Some(value1.to_string()));

    // Test set item for user 2 (same script, same key, different user)
    assert!(set_user_secret_item(script_uri, user_id_2, key, value2).is_ok());

    // Test get item for user 2
    let retrieved2 = get_user_secret_item(script_uri, user_id_2, key);
    assert_eq!(retrieved2, Some(value2.to_string()));

    // Verify user 1's data is still separate
    let still_user1 = get_user_secret_item(script_uri, user_id_1, key);
    assert_eq!(still_user1, Some(value1.to_string()));

    // Test overwrite for user 1
    let updated_value1 = "updated_token_for_user1";
    assert!(set_user_secret_item(script_uri, user_id_1, key, updated_value1).is_ok());
    assert_eq!(
        get_user_secret_item(script_uri, user_id_1, key),
        Some(updated_value1.to_string())
    );

    // Test remove item for user 1
    assert!(remove_user_secret_item(script_uri, user_id_1, key));

    // Verify item is gone for user 1
    let retrieved_after_remove = get_user_secret_item(script_uri, user_id_1, key);
    assert_eq!(retrieved_after_remove, None);

    // Verify user 2's data is still there
    let user2_still_there = get_user_secret_item(script_uri, user_id_2, key);
    assert_eq!(user2_still_there, Some(value2.to_string()));

    // Test clear for user 2
    assert!(set_user_secret_item(script_uri, user_id_2, "key1", "value1").is_ok());
    assert!(set_user_secret_item(script_uri, user_id_2, "key2", "value2").is_ok());

    assert!(clear_user_secrets(script_uri, user_id_2).is_ok());

    // Verify all items are gone for user 2
    assert_eq!(get_user_secret_item(script_uri, user_id_2, key), None);
    assert_eq!(get_user_secret_item(script_uri, user_id_2, "key1"), None);
    assert_eq!(get_user_secret_item(script_uri, user_id_2, "key2"), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_user_secrets_validation() {
    if should_skip_db_tests() {
        return;
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();
    let script_uri = "test://script";
    let user_id = "user_sec_123";

    // Test empty script URI
    assert!(set_user_secret_item("", user_id, "key", "value").is_err());

    // Test empty user ID
    assert!(set_user_secret_item(script_uri, "", "key", "value").is_err());

    // Test empty key
    assert!(set_user_secret_item(script_uri, user_id, "", "value").is_err());

    // Test oversized value
    let large_value = "x".repeat(1_000_001);
    assert!(set_user_secret_item(script_uri, user_id, "key", &large_value).is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn test_user_secrets_user_isolation() {
    if should_skip_db_tests() {
        return;
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();
    let script_uri = "test://user-secrets-isolation";
    upsert_script(script_uri, "// test").expect("script should store");
    let alice = "alice_secrets";
    let bob = "bob_secrets";

    // Both users set the same key in the same script
    assert!(set_user_secret_item(script_uri, alice, "api_token", "alice_token").is_ok());
    assert!(set_user_secret_item(script_uri, bob, "api_token", "bob_token").is_ok());

    // Each user should see only their own value
    assert_eq!(
        get_user_secret_item(script_uri, alice, "api_token"),
        Some("alice_token".to_string())
    );
    assert_eq!(
        get_user_secret_item(script_uri, bob, "api_token"),
        Some("bob_token".to_string())
    );

    // Removing alice's secret shouldn't affect bob
    assert!(remove_user_secret_item(script_uri, alice, "api_token"));
    assert_eq!(get_user_secret_item(script_uri, alice, "api_token"), None);
    assert_eq!(
        get_user_secret_item(script_uri, bob, "api_token"),
        Some("bob_token".to_string())
    );

    // Clearing bob's secrets
    assert!(clear_user_secrets(script_uri, bob).is_ok());
    assert_eq!(get_user_secret_item(script_uri, bob, "api_token"), None);
    assert_eq!(get_user_secret_item(script_uri, alice, "api_token"), None);
}

#[tokio::test(flavor = "multi_thread")]
async fn test_script_ownership_assignment() {
    if should_skip_db_tests() {
        return;
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();
    // Test that new scripts get assigned an owner
    let script_uri = "test://owned-script";
    let owner_user_id = "test-user-123";
    let script_code = "console.log('owned script');";

    // Create a new script with an owner
    let result = upsert_script_with_owner(script_uri, script_code, Some(owner_user_id));
    assert!(
        result.is_ok(),
        "Should successfully create script with owner"
    );

    // Verify the script was created
    let fetched = fetch_script(script_uri);
    assert_eq!(
        fetched,
        Some(script_code.to_string()),
        "Script should exist"
    );

    // Verify the owner was assigned
    let owners = get_script_owners(script_uri).expect("Should get owners");
    assert_eq!(owners.len(), 1, "Script should have exactly one owner");
    assert_eq!(
        owners[0], owner_user_id,
        "Owner should be the specified user"
    );

    // Verify user_owns_script returns true
    let owns = user_owns_script(script_uri, owner_user_id).expect("Should check ownership");
    assert!(owns, "User should own the script");

    // Verify count_script_owners returns 1
    let count = count_script_owners(script_uri).expect("Should count owners");
    assert_eq!(count, 1, "Should have exactly 1 owner");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_script_ownership_backfill() {
    if should_skip_db_tests() {
        return;
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();
    // Test that existing scripts without owners get backfilled
    let script_uri = "test://backfill-script";
    let script_code = "console.log('backfill test');";

    // Clean up any stale state from a prior run
    let _ = delete_script(script_uri);

    // First, create script without owner (simulate old script)
    let result = upsert_script(script_uri, script_code);
    assert!(result.is_ok(), "Should create script");

    // Verify it has no owners
    let owners_before = get_script_owners(script_uri).expect("Should get owners");
    assert_eq!(
        owners_before.len(),
        0,
        "Script should have no owners initially"
    );

    // Now update the script with a user (simulating editing in the editor)
    let editor_user_id = "editor-user-456";
    let updated_code = "console.log('backfill test - updated');";
    let result = upsert_script_with_owner(script_uri, updated_code, Some(editor_user_id));
    assert!(result.is_ok(), "Should update script with owner backfill");

    // Verify the owner was backfilled
    let owners_after = get_script_owners(script_uri).expect("Should get owners");
    assert_eq!(owners_after.len(), 1, "Script should now have one owner");
    assert_eq!(
        owners_after[0], editor_user_id,
        "Owner should be the editor"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_script_ownership_no_duplicate() {
    if should_skip_db_tests() {
        return;
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();
    // Test that scripts with existing owners don't get duplicates
    let script_uri = "test://no-duplicate-script";
    let owner_user_id = "original-owner";
    let script_code = "console.log('original');";

    // Create script with owner
    let result = upsert_script_with_owner(script_uri, script_code, Some(owner_user_id));
    assert!(result.is_ok(), "Should create script with owner");

    // Update script with same user
    let updated_code = "console.log('updated');";
    let result = upsert_script_with_owner(script_uri, updated_code, Some(owner_user_id));
    assert!(result.is_ok(), "Should update script");

    // Verify still only one owner
    let owners = get_script_owners(script_uri).expect("Should get owners");
    assert_eq!(owners.len(), 1, "Should still have exactly one owner");
    assert_eq!(owners[0], owner_user_id, "Owner should be unchanged");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_script_ownership_add_remove() {
    if should_skip_db_tests() {
        return;
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();
    // Test adding and removing owners
    let script_uri = "test://multi-owner-script";
    let owner1 = "owner-one";
    let owner2 = "owner-two";
    let script_code = "console.log('multi-owner');";

    // Clean up any stale state from a prior run
    let _ = delete_script(script_uri);

    // Create script with first owner
    upsert_script_with_owner(script_uri, script_code, Some(owner1)).expect("Should create script");

    // Add second owner
    let result = add_script_owner(script_uri, owner2);
    assert!(result.is_ok(), "Should add second owner");

    // Verify both owners exist
    let owners = get_script_owners(script_uri).expect("Should get owners");
    assert_eq!(owners.len(), 2, "Should have two owners");
    assert!(
        owners.contains(&owner1.to_string()),
        "Should contain owner1"
    );
    assert!(
        owners.contains(&owner2.to_string()),
        "Should contain owner2"
    );

    // Remove first owner
    let result = remove_script_owner(script_uri, owner1);
    assert!(result.is_ok(), "Should remove first owner");

    // Verify only second owner remains
    let owners_after = get_script_owners(script_uri).expect("Should get owners");
    assert_eq!(owners_after.len(), 1, "Should have one owner");
    assert_eq!(owners_after[0], owner2, "Only owner2 should remain");
}

#[tokio::test(flavor = "multi_thread")]
async fn test_script_ownership_without_user() {
    if should_skip_db_tests() {
        return;
    }
    let rt = get_runtime();
    let _guard = rt.enter();
    setup_db();
    // Test that scripts created without a user_id don't crash
    let script_uri = "test://no-user-script";
    let script_code = "console.log('no user');";

    // Create script without owner (None user_id)
    let result = upsert_script_with_owner(script_uri, script_code, None);
    assert!(result.is_ok(), "Should create script even without user_id");

    // Verify script exists but has no owners
    let fetched = fetch_script(script_uri);
    assert_eq!(
        fetched,
        Some(script_code.to_string()),
        "Script should exist"
    );

    let owners = get_script_owners(script_uri).expect("Should get owners");
    assert_eq!(owners.len(), 0, "Script should have no owners");
}
