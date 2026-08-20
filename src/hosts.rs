//! Which hostnames the engine serves, and which of them a script answers on.
//!
//! A deployment serves one default host (`server.base_url`) plus any extras in
//! `server.additional_base_urls`. A script's registrations — HTTP routes, asset
//! routes, streams, GraphQL operations and MCP tools — are published on the
//! default host unless it is bound to specific hosts, so a single-host
//! deployment behaves exactly as it did before hosts existed.
//!
//! Binding is stored per script in the `script_hosts` table and edited through
//! the admin-only management APIs; this module only answers questions about it.

use std::collections::HashSet;
use std::sync::OnceLock;

/// Stored host value meaning "every configured host". A script bound to this
/// publishes everywhere, which is what a shared page like `about.js` wants.
pub const ALL_HOSTS: &str = "*";

/// The hosts this engine serves, resolved at startup.
///
/// All the real logic lives here rather than on the global below, so it can be
/// exercised against an explicit instance.
#[derive(Debug, Clone, Default)]
pub struct HostConfig {
    /// Host that a script with no explicit binding publishes on, and the
    /// fallback for a request whose Host header names somewhere unconfigured.
    default_host: String,
    /// Every host the engine serves, default first.
    all_hosts: Vec<String>,
}

impl HostConfig {
    /// Build from the base URL and any additional base URLs, in the Host-header
    /// form requests carry. Entries that name no host are skipped.
    pub fn new(base_url: &str, additional_base_urls: &[String]) -> Self {
        let default_host = crate::config::base_url_authority(base_url).unwrap_or_default();

        let mut all_hosts = Vec::new();
        if !default_host.is_empty() {
            all_hosts.push(default_host.clone());
        }
        for extra in additional_base_urls {
            if let Some(host) = crate::config::base_url_authority(extra)
                && !all_hosts.contains(&host)
            {
                all_hosts.push(host);
            }
        }

        Self {
            default_host,
            all_hosts,
        }
    }

    pub fn default_host(&self) -> &str {
        &self.default_host
    }

    pub fn all_hosts(&self) -> &[String] {
        &self.all_hosts
    }

    /// Whether `host` is one of the hosts this engine is configured to serve.
    pub fn is_configured(&self, host: &str) -> bool {
        self.all_hosts.iter().any(|h| h == host)
    }

    /// Map a request's Host header onto one of the configured hosts.
    ///
    /// Anything unrecognised — a direct IP, a port-forward, a tunnel hostname,
    /// or a dev machine whose name differs from `base_url` — resolves to the
    /// default host, so scripts stay reachable the way they were before host
    /// binding existed.
    pub fn canonical_host(&self, request_host: Option<&str>) -> String {
        match request_host {
            Some(host) => {
                let host = host.trim().to_lowercase();
                if self.is_configured(&host) {
                    host
                } else {
                    self.default_host.clone()
                }
            }
            None => self.default_host.clone(),
        }
    }

    /// Expand a script's stored host bindings into the concrete hosts it serves.
    ///
    /// No rows means the default host. A `*` row means every configured host.
    /// Bindings naming a host the engine does not serve are dropped: they
    /// cannot match a request, and keeping them would imply reachability that
    /// is not there. An empty result is possible and simply means the script
    /// serves nothing over HTTP.
    pub fn effective_hosts(&self, stored: &[String]) -> Vec<String> {
        if stored.is_empty() {
            return if self.default_host.is_empty() {
                Vec::new()
            } else {
                vec![self.default_host.clone()]
            };
        }
        if stored.iter().any(|host| host == ALL_HOSTS) {
            return self.all_hosts.clone();
        }

        let configured: HashSet<&String> = self.all_hosts.iter().collect();
        let mut hosts: Vec<String> = Vec::new();
        for host in stored {
            let host = host.trim().to_lowercase();
            if configured.contains(&host) && !hosts.contains(&host) {
                hosts.push(host);
            }
        }
        hosts
    }

    /// Whether a script with these stored bindings serves `host`.
    pub fn serves_host(&self, stored: &[String], host: &str) -> bool {
        self.effective_hosts(stored).iter().any(|h| h == host)
    }
}

static HOST_CONFIG: OnceLock<HostConfig> = OnceLock::new();

/// Record the engine's host configuration. Called once at startup, before
/// anything can read it; later calls are ignored so the set of served hosts
/// cannot change under a running server.
pub fn init(config: HostConfig) {
    let _ = HOST_CONFIG.set(config);
}

/// The engine's host configuration, or an empty one when startup has not set
/// it (unit tests that never touch hosts, mainly). With an empty config every
/// script resolves to no hosts, so callers treat it as "unconfigured" and skip
/// host filtering entirely.
pub fn config() -> &'static HostConfig {
    HOST_CONFIG.get_or_init(HostConfig::default)
}

/// Whether host binding is in force. False before startup configures it, and
/// on a deployment with no usable base URL; callers then serve every script
/// everywhere, matching the behaviour from before host binding existed.
pub fn is_configured() -> bool {
    !config().all_hosts.is_empty()
}

/// The host a script with no explicit binding publishes on.
pub fn default_host() -> String {
    config().default_host.clone()
}

/// Every host the engine serves.
pub fn all_hosts() -> Vec<String> {
    config().all_hosts.clone()
}

/// Map a request's Host header onto a configured host. See
/// [`HostConfig::canonical_host`].
pub fn canonical_host(request_host: Option<&str>) -> String {
    config().canonical_host(request_host)
}

/// The hosts a script with these stored bindings serves.
pub fn effective_hosts(stored: &[String]) -> Vec<String> {
    config().effective_hosts(stored)
}

/// Whether a script with these stored bindings serves `host`.
///
/// Answers true when host binding is not configured at all, so a deployment
/// that never sets a base URL keeps serving every script on every host.
pub fn serves_host(stored: &[String], host: &str) -> bool {
    if !is_configured() {
        return true;
    }
    config().serves_host(stored, host)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> HostConfig {
        HostConfig::new(
            "https://softagen.com",
            &[
                "https://manage.softagen.com".to_string(),
                "https://world.softagen.com".to_string(),
            ],
        )
    }

    #[test]
    fn default_host_leads_the_configured_list() {
        let cfg = test_config();
        assert_eq!(cfg.default_host(), "softagen.com");
        assert_eq!(
            cfg.all_hosts(),
            [
                "softagen.com".to_string(),
                "manage.softagen.com".to_string(),
                "world.softagen.com".to_string()
            ]
        );
    }

    #[test]
    fn base_url_repeated_in_extras_is_not_duplicated() {
        let cfg = HostConfig::new(
            "https://softagen.com",
            &[
                "https://softagen.com".to_string(),
                "https://manage.softagen.com".to_string(),
            ],
        );
        assert_eq!(
            cfg.all_hosts(),
            [
                "softagen.com".to_string(),
                "manage.softagen.com".to_string()
            ]
        );
    }

    #[test]
    fn unconfigured_request_hosts_resolve_to_the_default() {
        let cfg = test_config();
        assert_eq!(
            cfg.canonical_host(Some("world.softagen.com")),
            "world.softagen.com"
        );
        assert_eq!(
            cfg.canonical_host(Some("WORLD.Softagen.com")),
            "world.softagen.com"
        );
        // A direct IP, a tunnel, or a dev box name all keep working.
        assert_eq!(cfg.canonical_host(Some("127.0.0.1:3000")), "softagen.com");
        assert_eq!(cfg.canonical_host(None), "softagen.com");
    }

    #[test]
    fn unbound_scripts_serve_the_default_host_only() {
        let cfg = test_config();
        assert_eq!(cfg.effective_hosts(&[]), vec!["softagen.com".to_string()]);
        assert!(cfg.serves_host(&[], "softagen.com"));
        assert!(!cfg.serves_host(&[], "manage.softagen.com"));
    }

    #[test]
    fn wildcard_binding_serves_every_configured_host() {
        let cfg = test_config();
        let stored = vec![ALL_HOSTS.to_string()];
        assert_eq!(cfg.effective_hosts(&stored), cfg.all_hosts());
        assert!(cfg.serves_host(&stored, "softagen.com"));
        assert!(cfg.serves_host(&stored, "world.softagen.com"));
    }

    #[test]
    fn explicit_binding_serves_only_those_hosts() {
        let cfg = test_config();
        let stored = vec!["manage.softagen.com".to_string()];
        assert!(cfg.serves_host(&stored, "manage.softagen.com"));
        assert!(!cfg.serves_host(&stored, "softagen.com"));
        assert!(!cfg.serves_host(&stored, "world.softagen.com"));
    }

    #[test]
    fn bindings_naming_unserved_hosts_are_dropped() {
        let cfg = test_config();
        // Left behind by a config change; it can never match a request.
        let stored = vec!["retired.softagen.com".to_string()];
        assert!(cfg.effective_hosts(&stored).is_empty());
        assert!(!cfg.serves_host(&stored, "softagen.com"));
    }

    #[test]
    fn binding_is_case_insensitive_and_deduplicated() {
        let cfg = test_config();
        let stored = vec![
            "MANAGE.softagen.com".to_string(),
            "manage.softagen.com".to_string(),
        ];
        assert_eq!(
            cfg.effective_hosts(&stored),
            vec!["manage.softagen.com".to_string()]
        );
    }

    #[test]
    fn an_empty_config_leaves_host_filtering_off() {
        // Before startup configures hosts, nothing should be filtered out.
        let cfg = HostConfig::default();
        assert!(cfg.all_hosts().is_empty());
        assert!(cfg.effective_hosts(&[]).is_empty());
    }
}
