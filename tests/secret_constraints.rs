//! Tests for secret access control with URL and script URI constraints

use aiwebengine::secrets::{SecretAccessError, SecretsManager};

#[test]
fn test_unrestricted_secret_backward_compatibility() {
    let manager = SecretsManager::new();

    // Old format: unrestricted secret
    manager.set("old_secret".to_string(), "old_value".to_string());

    // Should work with any URL and script
    let result = manager.get_with_constraints(
        "old_secret",
        "https://example.com/api",
        Some("/any/script.js"),
    );
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "old_value");
}

#[test]
fn test_url_constraint_matching() {
    let manager = SecretsManager::new();

    // Simulate loading: SECRET_GITHUB_TOKEN__ALLOW_https://api.github.com/*__SCRIPT_*
    unsafe {
        std::env::set_var(
            "SECRET_TEST_GITHUB__ALLOW_https://api.github.com/*__SCRIPT_*",
            "ghp_test123",
        );
    }
    manager.load_from_env();
    unsafe {
        std::env::remove_var("SECRET_TEST_GITHUB__ALLOW_https://api.github.com/*__SCRIPT_*");
    }

    // Should succeed: matches URL pattern
    let result = manager.get_with_constraints(
        "test_github",
        "https://api.github.com/repos",
        Some("/scripts/test.js"),
    );
    assert!(result.is_ok());

    // Should fail: wrong URL
    let result = manager.get_with_constraints(
        "test_github",
        "https://attacker.com/steal",
        Some("/scripts/test.js"),
    );
    assert!(matches!(
        result,
        Err(SecretAccessError::UrlConstraintViolation { .. })
    ));
}

#[test]
fn test_script_constraint_matching() {
    let manager = SecretsManager::new();

    // Simulate loading: SECRET_API_KEY__ALLOW_*__SCRIPT_/scripts/integrations/*
    unsafe {
        std::env::set_var(
            "SECRET_TEST_API__ALLOW_*__SCRIPT_/scripts/integrations/*",
            "sk_test456",
        );
    }
    manager.load_from_env();
    unsafe {
        std::env::remove_var("SECRET_TEST_API__ALLOW_*__SCRIPT_/scripts/integrations/*");
    }

    // Should succeed: matches script pattern
    let result = manager.get_with_constraints(
        "test_api",
        "https://any.com/api",
        Some("/scripts/integrations/github.js"),
    );
    assert!(result.is_ok());

    // Should fail: wrong script URI
    let result = manager.get_with_constraints(
        "test_api",
        "https://any.com/api",
        Some("/scripts/malicious.js"),
    );
    assert!(matches!(
        result,
        Err(SecretAccessError::ScriptConstraintViolation { .. })
    ));
}

#[test]
fn test_url_normalization_case_insensitive() {
    let manager = SecretsManager::new();

    // Lowercase pattern
    unsafe {
        std::env::set_var(
            "SECRET_TEST_CASE__ALLOW_https://api.example.com/*__SCRIPT_*",
            "test_value",
        );
    }
    manager.load_from_env();
    unsafe {
        std::env::remove_var("SECRET_TEST_CASE__ALLOW_https://api.example.com/*__SCRIPT_*");
    }

    // Should match: case-insensitive domain matching
    let result = manager.get_with_constraints(
        "test_case",
        "https://API.EXAMPLE.COM/path",
        Some("/any/script.js"),
    );
    assert!(result.is_ok());
}

#[test]
fn test_script_uri_case_sensitive() {
    let manager = SecretsManager::new();

    // Lowercase script pattern
    unsafe {
        std::env::set_var(
            "SECRET_TEST_SCRIPT_CASE__ALLOW_*__SCRIPT_/scripts/lower/*",
            "test_value",
        );
    }
    manager.load_from_env();
    unsafe {
        std::env::remove_var("SECRET_TEST_SCRIPT_CASE__ALLOW_*__SCRIPT_/scripts/lower/*");
    }

    // Should succeed: exact case match
    let result = manager.get_with_constraints(
        "test_script_case",
        "https://any.com",
        Some("/scripts/lower/file.js"),
    );
    assert!(result.is_ok());

    // Should fail: wrong case (case-sensitive)
    let result = manager.get_with_constraints(
        "test_script_case",
        "https://any.com",
        Some("/scripts/LOWER/file.js"),
    );
    assert!(matches!(
        result,
        Err(SecretAccessError::ScriptConstraintViolation { .. })
    ));
}

#[test]
fn test_wildcard_patterns() {
    let manager = SecretsManager::new();

    // Multiple wildcard pattern
    unsafe {
        std::env::set_var(
            "SECRET_TEST_WILDCARD__ALLOW_https://*.anthropic.com/v*/messages__SCRIPT_/scripts/*/chat.js",
            "sk_test789",
        );
    }
    manager.load_from_env();
    unsafe {
        std::env::remove_var(
            "SECRET_TEST_WILDCARD__ALLOW_https://*.anthropic.com/v*/messages__SCRIPT_/scripts/*/chat.js",
        );
    }

    // Should match subdomain wildcard
    let result = manager.get_with_constraints(
        "test_wildcard",
        "https://api.anthropic.com/v1/messages",
        Some("/scripts/ai/chat.js"),
    );
    assert!(result.is_ok());

    // Should match path wildcard
    let result = manager.get_with_constraints(
        "test_wildcard",
        "https://api.anthropic.com/v2/messages",
        Some("/scripts/integrations/chat.js"),
    );
    assert!(result.is_ok());
}

#[test]
fn test_missing_script_uri_with_constraint() {
    let manager = SecretsManager::new();

    unsafe {
        std::env::set_var(
            "SECRET_TEST_NO_SCRIPT__ALLOW_*__SCRIPT_/required/*",
            "test_value",
        );
    }
    manager.load_from_env();
    unsafe {
        std::env::remove_var("SECRET_TEST_NO_SCRIPT__ALLOW_*__SCRIPT_/required/*");
    }

    // Should fail: script_uri is required but not provided
    let result = manager.get_with_constraints("test_no_script", "https://any.com", None);
    assert!(matches!(
        result,
        Err(SecretAccessError::ScriptConstraintViolation { .. })
    ));
}

#[test]
fn test_secret_not_found() {
    let manager = SecretsManager::new();

    let result =
        manager.get_with_constraints("nonexistent", "https://any.com", Some("/any/script.js"));
    assert!(matches!(result, Err(SecretAccessError::NotFound(_))));
}

#[test]
fn test_complex_real_world_scenario() {
    let manager = SecretsManager::new();

    // Anthropic API key - only for Claude API calls from AI scripts
    unsafe {
        std::env::set_var(
            "SECRET_ANTHROPIC_API_KEY__ALLOW_https://api.anthropic.com/*__SCRIPT_/scripts/ai/*",
            "sk-ant-real-key",
        );

        // GitHub token - only for GitHub API from integration scripts
        std::env::set_var(
            "SECRET_GITHUB_TOKEN__ALLOW_https://api.github.com/*__SCRIPT_/scripts/integrations/github*",
            "ghp_real_token",
        );

        // Unrestricted internal API key (backward compatible)
        std::env::set_var("SECRET_INTERNAL_KEY", "internal_value");
    }

    manager.load_from_env();

    // Clean up
    unsafe {
        std::env::remove_var(
            "SECRET_ANTHROPIC_API_KEY__ALLOW_https://api.anthropic.com/*__SCRIPT_/scripts/ai/*",
        );
        std::env::remove_var(
            "SECRET_GITHUB_TOKEN__ALLOW_https://api.github.com/*__SCRIPT_/scripts/integrations/github*",
        );
        std::env::remove_var("SECRET_INTERNAL_KEY");
    }

    // Valid: AI script accessing Anthropic API
    assert!(
        manager
            .get_with_constraints(
                "anthropic_api_key",
                "https://api.anthropic.com/v1/messages",
                Some("/scripts/ai/chat_handler.js"),
            )
            .is_ok()
    );

    // Invalid: AI script trying to exfiltrate to attacker
    assert!(matches!(
        manager.get_with_constraints(
            "anthropic_api_key",
            "https://attacker.com/steal",
            Some("/scripts/ai/chat_handler.js"),
        ),
        Err(SecretAccessError::UrlConstraintViolation { .. })
    ));

    // Invalid: Wrong script trying to use GitHub token
    assert!(matches!(
        manager.get_with_constraints(
            "github_token",
            "https://api.github.com/repos",
            Some("/scripts/malicious/steal.js"),
        ),
        Err(SecretAccessError::ScriptConstraintViolation { .. })
    ));

    // Valid: Internal key works everywhere (unrestricted)
    assert!(
        manager
            .get_with_constraints(
                "internal_key",
                "https://any-domain.com/api",
                Some("/any/script/anywhere.js"),
            )
            .is_ok()
    );
}
