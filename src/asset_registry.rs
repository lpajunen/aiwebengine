/// Asset Path Registry
///
/// This module manages runtime registration of public HTTP paths to asset names.
/// Assets are stored by name in the repository, and scripts can register them to
/// specific HTTP paths using routeRegistry.registerAssetRoute() in their init() functions.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tracing::{debug, info, warn};

/// Global asset path registry
pub static GLOBAL_ASSET_REGISTRY: OnceLock<AssetRegistry> = OnceLock::new();

/// Get or initialize the global asset registry
pub fn get_global_registry() -> &'static AssetRegistry {
    GLOBAL_ASSET_REGISTRY.get_or_init(AssetRegistry::new)
}

/// Stores registration information for a public asset path
#[derive(Debug, Clone)]
pub struct AssetPathRegistration {
    /// The name of the asset in the repository
    pub asset_name: String,
    /// The script URI that registered this path
    pub script_uri: String,
}

/// Registry for managing public asset path registrations
#[derive(Debug)]
pub struct AssetRegistry {
    /// Map of HTTP path -> asset registration
    paths: Arc<Mutex<HashMap<String, AssetPathRegistration>>>,
}

impl AssetRegistry {
    /// Create a new asset registry
    pub fn new() -> Self {
        Self {
            paths: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Register a public path for an asset
    ///
    /// # Arguments
    /// * `path` - The HTTP path (e.g., "/logo.svg", "/css/main.css")
    /// * `asset_name` - The name of the asset in the repository (e.g., "logo.svg", "main.css")
    /// * `script_uri` - The URI of the script registering this path
    pub fn register_path(
        &self,
        path: &str,
        asset_name: &str,
        script_uri: &str,
    ) -> Result<(), String> {
        match self.paths.lock() {
            Ok(mut paths) => {
                if let Some(existing) = paths.get(path) {
                    if existing.asset_name != asset_name || existing.script_uri != script_uri {
                        warn!(
                            "Overwriting asset path '{}': was {} from {}, now {} from {}",
                            path, existing.asset_name, existing.script_uri, asset_name, script_uri
                        );
                    } else {
                        debug!(
                            "Asset path '{}' already registered to {} from {}",
                            path, asset_name, script_uri
                        );
                        return Ok(());
                    }
                }

                info!(
                    "Registering asset path '{}' -> asset '{}' (from script '{}')",
                    path, asset_name, script_uri
                );

                paths.insert(
                    path.to_string(),
                    AssetPathRegistration {
                        asset_name: asset_name.to_string(),
                        script_uri: script_uri.to_string(),
                    },
                );
                Ok(())
            }
            Err(e) => {
                let err_msg = format!("Failed to lock asset registry: {}", e);
                warn!("{}", err_msg);
                Err(err_msg)
            }
        }
    }

    /// Get the asset name for a given HTTP path
    pub fn get_asset_name(&self, path: &str) -> Option<String> {
        match self.paths.lock() {
            Ok(paths) => paths.get(path).map(|reg| reg.asset_name.clone()),
            Err(e) => {
                warn!("Failed to lock asset registry for lookup: {}", e);
                None
            }
        }
    }

    /// Get the full asset registration (asset name and script URI) for a given HTTP path
    pub fn get_asset_registration(&self, path: &str) -> Option<AssetPathRegistration> {
        match self.paths.lock() {
            Ok(paths) => paths.get(path).cloned(),
            Err(e) => {
                warn!("Failed to lock asset registry for lookup: {}", e);
                None
            }
        }
    }

    /// Check if a path is registered
    pub fn is_path_registered(&self, path: &str) -> bool {
        match self.paths.lock() {
            Ok(paths) => paths.contains_key(path),
            Err(e) => {
                warn!("Failed to lock asset registry for check: {}", e);
                false
            }
        }
    }

    /// Unregister a path
    pub fn unregister_path(&self, path: &str) -> bool {
        match self.paths.lock() {
            Ok(mut paths) => {
                let existed = paths.remove(path).is_some();
                if existed {
                    info!("Unregistered asset path: {}", path);
                } else {
                    debug!("Attempted to unregister non-existent path: {}", path);
                }
                existed
            }
            Err(e) => {
                warn!("Failed to lock asset registry for unregistration: {}", e);
                false
            }
        }
    }

    /// Get all registered paths
    pub fn list_paths(&self) -> Vec<String> {
        match self.paths.lock() {
            Ok(paths) => paths.keys().cloned().collect(),
            Err(e) => {
                warn!("Failed to lock asset registry for listing: {}", e);
                Vec::new()
            }
        }
    }

    /// Get all asset path registrations with their details
    pub fn get_all_registrations(&self) -> Vec<(String, AssetPathRegistration)> {
        match self.paths.lock() {
            Ok(paths) => paths
                .iter()
                .map(|(path, reg)| (path.clone(), reg.clone()))
                .collect(),
            Err(e) => {
                warn!(
                    "Failed to lock asset registry for getting all registrations: {}",
                    e
                );
                Vec::new()
            }
        }
    }

    /// Clear all registrations (typically used for testing or reinitialization)
    pub fn clear(&self) {
        match self.paths.lock() {
            Ok(mut paths) => {
                let count = paths.len();
                paths.clear();
                info!("Cleared {} asset path registrations", count);
            }
            Err(e) => {
                warn!("Failed to lock asset registry for clearing: {}", e);
            }
        }
    }

    /// Get all registrations for a specific script
    pub fn get_paths_for_script(&self, script_uri: &str) -> Vec<String> {
        match self.paths.lock() {
            Ok(paths) => paths
                .iter()
                .filter(|(_, reg)| reg.script_uri == script_uri)
                .map(|(path, _)| path.clone())
                .collect(),
            Err(e) => {
                warn!("Failed to lock asset registry for script lookup: {}", e);
                Vec::new()
            }
        }
    }
}

impl Default for AssetRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_asset_registry() {
        let registry = AssetRegistry::new();

        // Register an asset path
        assert!(
            registry
                .register_path("/logo.svg", "logo.svg", "test_script")
                .is_ok()
        );

        // Check if path is registered
        assert!(registry.is_path_registered("/logo.svg"));
        assert_eq!(
            registry.get_asset_name("/logo.svg"),
            Some("logo.svg".to_string())
        );

        // List paths
        let paths = registry.list_paths();
        assert_eq!(paths.len(), 1);
        assert!(paths.contains(&"/logo.svg".to_string()));

        // Unregister
        assert!(registry.unregister_path("/logo.svg"));
        assert!(!registry.is_path_registered("/logo.svg"));
    }

    #[test]
    fn test_get_paths_for_script() {
        let registry = AssetRegistry::new();

        registry
            .register_path("/logo.svg", "logo.svg", "script1")
            .unwrap();
        registry
            .register_path("/favicon.ico", "favicon.ico", "script1")
            .unwrap();
        registry
            .register_path("/app.css", "app.css", "script2")
            .unwrap();

        let script1_paths = registry.get_paths_for_script("script1");
        assert_eq!(script1_paths.len(), 2);

        let script2_paths = registry.get_paths_for_script("script2");
        assert_eq!(script2_paths.len(), 1);
    }

    #[test]
    fn test_get_all_registrations() {
        let registry = AssetRegistry::new();

        registry
            .register_path("/logo.svg", "logo.svg", "script1")
            .unwrap();
        registry
            .register_path("/favicon.ico", "favicon.ico", "script1")
            .unwrap();
        registry
            .register_path("/app.css", "app.css", "script2")
            .unwrap();

        let all_registrations = registry.get_all_registrations();
        assert_eq!(all_registrations.len(), 3);

        // Verify the registrations contain expected data
        let logo_reg = all_registrations
            .iter()
            .find(|(path, _)| path == "/logo.svg")
            .unwrap();
        assert_eq!(logo_reg.1.asset_name, "logo.svg");
        assert_eq!(logo_reg.1.script_uri, "script1");

        let css_reg = all_registrations
            .iter()
            .find(|(path, _)| path == "/app.css")
            .unwrap();
        assert_eq!(css_reg.1.asset_name, "app.css");
        assert_eq!(css_reg.1.script_uri, "script2");
    }
}
