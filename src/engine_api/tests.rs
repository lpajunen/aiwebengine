/// Every tool's description and schema is paid for in every agent's
/// context, listed or not used. A description says what the tool does and
/// the one rule that is not obvious; the rest is the schema's job.
#[test]
fn the_tool_listing_stays_small() {
    let tools = super::native_mcp_tool_descriptors();
    for tool in &tools {
        assert!(
            tool.description.len() <= 500,
            "{} has a {}-character description; keep it to what it does and one rule",
            tool.name,
            tool.description.len()
        );
    }
    let total: usize = tools
        .iter()
        .map(|tool| tool.description.len() + tool.input_schema.to_string().len())
        .sum();
    assert!(
        total <= 26_000,
        "the tool listing is {total} characters; cut before adding"
    );
}

use super::{host_is_allowed, reserved_route_prefix};

#[test]
fn reserved_prefixes_match_exact_and_subpaths() {
    assert_eq!(reserved_route_prefix("/engine"), Some("/engine"));
    assert_eq!(reserved_route_prefix("/engine/scripts"), Some("/engine"));
    assert_eq!(reserved_route_prefix("/auth/login"), Some("/auth"));
    assert_eq!(
        reserved_route_prefix("/.well-known/oauth-authorization-server"),
        Some("/.well-known")
    );
    assert_eq!(reserved_route_prefix("/health"), Some("/health"));
    assert_eq!(reserved_route_prefix("/mcp"), Some("/mcp"));
    assert_eq!(
        reserved_route_prefix("/engine/health/cluster"),
        Some("/engine")
    );
    // OAuth2 lives entirely under /auth, so it needs no prefix of its own.
    assert_eq!(reserved_route_prefix("/auth/oauth2/token"), Some("/auth"));
}

#[test]
fn non_reserved_paths_are_allowed() {
    assert_eq!(reserved_route_prefix("/"), None);
    assert_eq!(reserved_route_prefix("/favicon.ico"), None);
    assert_eq!(reserved_route_prefix("/engineering"), None);
    assert_eq!(reserved_route_prefix("/healthcheck"), None);
    assert_eq!(reserved_route_prefix("/authors"), None);
    assert_eq!(reserved_route_prefix("/my/app"), None);
    // The top-level OAuth2 endpoints were withdrawn once clients migrated
    // to /auth/oauth2/*, so scripts may claim these names themselves.
    assert_eq!(reserved_route_prefix("/authorize"), None);
    assert_eq!(reserved_route_prefix("/token"), None);
    assert_eq!(reserved_route_prefix("/oauth2/token"), None);
    assert_eq!(reserved_route_prefix("/oauth2"), None);
}

#[test]
fn empty_management_host_list_allows_every_host() {
    // A single-host deployment leaves the setting unset and must keep
    // serving the management APIs wherever it is reached.
    assert!(host_is_allowed(&[], Some("softagen.com")));
    assert!(host_is_allowed(&[], None));
}

#[test]
fn configured_management_hosts_match_exactly_and_case_insensitively() {
    let allowed = vec!["manage.softagen.com".to_string()];

    assert!(host_is_allowed(&allowed, Some("manage.softagen.com")));
    assert!(host_is_allowed(&allowed, Some("MANAGE.Softagen.com")));
    assert!(host_is_allowed(&allowed, Some("  manage.softagen.com  ")));

    assert!(!host_is_allowed(&allowed, Some("softagen.com")));
    assert!(!host_is_allowed(&allowed, Some("world.softagen.com")));
    // Not a suffix or prefix match: neither a parent domain nor an
    // attacker-controlled name that merely ends with the allowed host.
    assert!(!host_is_allowed(&allowed, Some("evil-manage.softagen.com")));
    assert!(!host_is_allowed(
        &allowed,
        Some("manage.softagen.com.evil.test")
    ));
}

#[test]
fn missing_host_header_is_refused_when_restricted() {
    let allowed = vec!["manage.softagen.com".to_string()];
    assert!(!host_is_allowed(&allowed, None));
}

#[test]
fn management_host_list_may_name_several_hosts() {
    let allowed = vec![
        "manage.softagen.com".to_string(),
        "localhost:3000".to_string(),
    ];
    assert!(host_is_allowed(&allowed, Some("manage.softagen.com")));
    assert!(host_is_allowed(&allowed, Some("localhost:3000")));
    assert!(!host_is_allowed(&allowed, Some("localhost")));
}
