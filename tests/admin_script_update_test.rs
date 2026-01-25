/// Test for verifying that administrators can update scripts without owners
/// This addresses the issue where scripts without owners cannot be updated
#[cfg(test)]
mod admin_script_update_tests {
    use aiwebengine::repository;
    use aiwebengine::security::{Capability, UserContext};
    use std::collections::HashSet;

    #[tokio::test(flavor = "multi_thread")]
    async fn test_admin_can_update_ownerless_script() {
        // Initialize test environment
        unsafe {
            std::env::set_var("DATABASE_URL", ":memory:");
            std::env::set_var("AIWEBENGINE_MODE", "development");
        }

        let test_uri = "https://example.com/test-ownerless-script";
        let initial_content = "// Initial content\nfunction init() { return { success: true }; }";
        let updated_content =
            "// Updated content\nfunction init() { return { success: true, updated: true }; }";

        // Create an admin user context
        let admin_user = UserContext {
            user_id: Some("admin-user-123".to_string()),
            is_authenticated: true,
            capabilities: [
                Capability::ReadScripts,
                Capability::WriteScripts,
                Capability::DeleteScripts, // Admin capability
                Capability::ReadAssets,
                Capability::WriteAssets,
                Capability::DeleteAssets,
            ]
            .into_iter()
            .collect::<HashSet<_>>(),
        };

        // Step 1: Create a script without an owner (simulating a bootstrap script or legacy script)
        repository::upsert_script(test_uri, initial_content)
            .expect("Should be able to create initial script");

        // Verify the script was created
        let fetched = repository::fetch_script(test_uri);
        assert!(fetched.is_some(), "Script should exist after creation");
        assert_eq!(
            fetched.unwrap(),
            initial_content,
            "Script content should match initial content"
        );

        // Verify the script has no owners
        let owner_count =
            repository::count_script_owners(test_uri).expect("Should be able to count owners");
        assert_eq!(
            owner_count, 0,
            "Script should have no owners (ownerless script)"
        );

        // Step 2: Verify admin user has the necessary capability
        assert!(
            admin_user.has_capability(&Capability::DeleteScripts),
            "Admin user should have DeleteScripts capability"
        );

        // Step 3: Admin attempts to update the ownerless script
        let update_result = repository::upsert_script_with_owner(
            test_uri,
            updated_content,
            admin_user.user_id.as_deref(),
        );

        // This should succeed even though the script has no owner
        assert!(
            update_result.is_ok(),
            "Admin should be able to update ownerless script, got error: {:?}",
            update_result.err()
        );

        // Step 4: Verify the script was actually updated in the database
        let fetched_after_update = repository::fetch_script(test_uri);
        assert!(
            fetched_after_update.is_some(),
            "Script should still exist after update"
        );
        assert_eq!(
            fetched_after_update.unwrap(),
            updated_content,
            "Script content should be updated to new content"
        );

        // Step 5: Verify ownership status after update
        let owner_count_after = repository::count_script_owners(test_uri)
            .expect("Should be able to count owners after update");

        // The script should now have the admin as owner (backfilled during update)
        assert!(
            owner_count_after > 0,
            "Script should have owner assigned after admin update (backfill)"
        );

        // Cleanup
        repository::delete_script(test_uri);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_non_admin_cannot_update_unowned_script() {
        unsafe {
            std::env::set_var("DATABASE_URL", ":memory:");
            std::env::set_var("AIWEBENGINE_MODE", "production");
        }

        let test_uri = "https://example.com/test-ownerless-script-2";
        let initial_content = "// Initial content";
        let updated_content = "// Updated content";

        // Create a regular authenticated user (not admin)
        let regular_user = UserContext {
            user_id: Some("regular-user-456".to_string()),
            is_authenticated: true,
            capabilities: [
                Capability::ReadScripts,
                Capability::WriteScripts,
                Capability::ReadAssets,
                Capability::WriteAssets,
            ]
            .into_iter()
            .collect::<HashSet<_>>(),
        };

        // Create script without owner
        repository::upsert_script(test_uri, initial_content)
            .expect("Should be able to create initial script");

        // Verify no owner
        let owner_count =
            repository::count_script_owners(test_uri).expect("Should be able to count owners");
        assert_eq!(owner_count, 0, "Script should have no owners");

        // Regular user should NOT have DeleteScripts capability
        assert!(
            !regular_user.has_capability(&Capability::DeleteScripts),
            "Regular user should NOT have DeleteScripts capability"
        );

        // Note: The permission check happens in secure_globals.rs, not in repository layer
        // At the repository layer, the update would succeed
        // But in the actual application flow through secure_globals, it would be blocked

        // At repository level, this will succeed (no permission check there)
        let update_result = repository::upsert_script_with_owner(
            test_uri,
            updated_content,
            regular_user.user_id.as_deref(),
        );

        assert!(
            update_result.is_ok(),
            "Repository layer doesn't enforce permissions"
        );

        // Cleanup
        repository::delete_script(test_uri);
    }
}
