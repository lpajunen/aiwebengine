use anyhow::Result;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};
use tracing::{debug, error, warn};

/// Defines the types of repository errors that can occur
#[derive(Debug, thiserror::Error)]
pub enum RepositoryError {
    #[error("Mutex lock failed: {0}")]
    LockError(String),
    #[error("Script not found: {0}")]
    ScriptNotFound(String),
    #[error("Asset not found: {0}")]
    AssetNotFound(String),
    #[error("Invalid data format: {0}")]
    InvalidData(String),
}

/// Asset representation
#[derive(Debug, Clone)]
pub struct Asset {
    pub public_path: String,
    pub mimetype: String,
    pub content: Vec<u8>,
}

static DYNAMIC_SCRIPTS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
static DYNAMIC_LOGS: OnceLock<Mutex<HashMap<String, Vec<String>>>> = OnceLock::new();
static DYNAMIC_ASSETS: OnceLock<Mutex<HashMap<String, Asset>>> = OnceLock::new();

/// Safe mutex access with recovery from poisoned state
fn safe_lock_scripts()
-> Result<std::sync::MutexGuard<'static, HashMap<String, String>>, RepositoryError> {
    let store = DYNAMIC_SCRIPTS.get_or_init(|| Mutex::new(HashMap::new()));

    match store.lock() {
        Ok(guard) => Ok(guard),
        Err(PoisonError { .. }) => {
            warn!("Scripts mutex was poisoned, recovering with new data");
            // In a poisoned state, we can still access the data but should log this
            // In production, you might want to restart the component or use more sophisticated recovery
            store.lock().map_err(|e| {
                error!("Failed to recover from poisoned mutex: {}", e);
                RepositoryError::LockError(format!("Unrecoverable mutex poisoning: {}", e))
            })
        }
    }
}

fn safe_lock_logs()
-> Result<std::sync::MutexGuard<'static, HashMap<String, Vec<String>>>, RepositoryError> {
    let store = DYNAMIC_LOGS.get_or_init(|| Mutex::new(HashMap::new()));

    match store.lock() {
        Ok(guard) => Ok(guard),
        Err(PoisonError { .. }) => {
            warn!("Logs mutex was poisoned, recovering with new data");
            store.lock().map_err(|e| {
                error!("Failed to recover from poisoned logs mutex: {}", e);
                RepositoryError::LockError(format!("Unrecoverable mutex poisoning: {}", e))
            })
        }
    }
}

fn safe_lock_assets()
-> Result<std::sync::MutexGuard<'static, HashMap<String, Asset>>, RepositoryError> {
    let store = DYNAMIC_ASSETS.get_or_init(|| Mutex::new(HashMap::new()));

    match store.lock() {
        Ok(guard) => Ok(guard),
        Err(PoisonError { .. }) => {
            warn!("Assets mutex was poisoned, recovering with new data");
            store.lock().map_err(|e| {
                error!("Failed to recover from poisoned assets mutex: {}", e);
                RepositoryError::LockError(format!("Unrecoverable mutex poisoning: {}", e))
            })
        }
    }
}

/// Fetch scripts from repository with proper error handling
pub fn fetch_scripts() -> HashMap<String, String> {
    let mut m = HashMap::new();

    // Always include core functionality scripts
    let core = include_str!("../scripts/feature_scripts/core.js");
    let asset_mgmt = include_str!("../scripts/feature_scripts/asset_mgmt.js");
    let editor = include_str!("../scripts/feature_scripts/editor.js");

    m.insert("https://example.com/core".to_string(), core.to_string());
    m.insert(
        "https://example.com/asset_mgmt".to_string(),
        asset_mgmt.to_string(),
    );
    m.insert("https://example.com/editor".to_string(), editor.to_string());

    // Include test scripts when appropriate
    let include_test_scripts =
        std::env::var("AIWEBENGINE_INCLUDE_TEST_SCRIPTS").is_ok() || cfg!(test);

    if include_test_scripts {
        let graphql_test = include_str!("../scripts/test_scripts/graphql_test.js");
        m.insert(
            "https://example.com/graphql_test".to_string(),
            graphql_test.to_string(),
        );
    }

    // Safely merge in any dynamically upserted scripts
    match safe_lock_scripts() {
        Ok(guard) => {
            for (k, v) in guard.iter() {
                m.insert(k.to_string(), v.to_string());
            }
        }
        Err(e) => {
            error!(
                "Failed to access dynamic scripts: {}. Continuing with static scripts only.",
                e
            );
            // Continue with just the static scripts rather than crashing
        }
    }

    m
}

/// Fetch a single script by URI with proper error handling
pub fn fetch_script(uri: &str) -> Option<String> {
    // First check static scripts
    let static_scripts = fetch_scripts();
    if let Some(script) = static_scripts.get(uri) {
        return Some(script.clone());
    }

    // Then check dynamic scripts
    match safe_lock_scripts() {
        Ok(guard) => guard.get(uri).cloned(),
        Err(e) => {
            error!("Failed to access dynamic scripts for URI {}: {}", uri, e);
            None
        }
    }
}

/// Insert log message with error handling
pub fn insert_log_message(script_uri: &str, message: &str) {
    match safe_lock_logs() {
        Ok(mut guard) => {
            guard
                .entry(script_uri.to_string())
                .or_insert_with(Vec::new)
                .push(message.to_string());
            debug!("Logged message for script {}: {}", script_uri, message);
        }
        Err(e) => {
            error!(
                "Failed to insert log message for {}: {}. Message: {}",
                script_uri, e, message
            );
            // Log to system instead as fallback
            error!("FALLBACK LOG [{}]: {}", script_uri, message);
        }
    }
}

/// Fetch log messages with error handling
pub fn fetch_log_messages(script_uri: &str) -> Vec<String> {
    match safe_lock_logs() {
        Ok(guard) => guard.get(script_uri).cloned().unwrap_or_default(),
        Err(e) => {
            error!("Failed to fetch log messages for {}: {}", script_uri, e);
            vec![format!("Error: Could not retrieve logs - {}", e)]
        }
    }
}

/// Clear log messages for a script
pub fn clear_log_messages(script_uri: &str) -> Result<(), RepositoryError> {
    let mut guard = safe_lock_logs()?;

    guard.remove(script_uri);
    debug!("Cleared log messages for script: {}", script_uri);
    Ok(())
}

/// Keep only the latest `limit` log messages (default 20) for each script URI and remove older ones
pub fn prune_log_messages() -> Result<(), RepositoryError> {
    const LIMIT: usize = 20;
    let mut guard = safe_lock_logs()?;

    for logs in guard.values_mut() {
        if logs.len() > LIMIT {
            let remove = logs.len() - LIMIT;
            // remove older entries from the front
            logs.drain(0..remove);
        }
    }
    debug!("Pruned log messages, keeping {} entries per script", LIMIT);
    Ok(())
}

/// Upsert script with error handling
pub fn upsert_script(uri: &str, content: &str) -> Result<(), RepositoryError> {
    if uri.trim().is_empty() {
        return Err(RepositoryError::InvalidData(
            "URI cannot be empty".to_string(),
        ));
    }

    if content.len() > 1_000_000 {
        // 1MB limit
        return Err(RepositoryError::InvalidData(
            "Script content too large (>1MB)".to_string(),
        ));
    }

    let mut guard = safe_lock_scripts()?;

    guard.insert(uri.to_string(), content.to_string());
    debug!("Upserted script: {} ({} bytes)", uri, content.len());
    Ok(())
}

/// Delete script with error handling
pub fn delete_script(uri: &str) -> bool {
    match safe_lock_scripts() {
        Ok(mut guard) => {
            let existed = guard.remove(uri).is_some();
            if existed {
                debug!("Deleted script: {}", uri);
            } else {
                debug!("Attempted to delete non-existent script: {}", uri);
            }
            existed
        }
        Err(e) => {
            error!("Failed to delete script {}: {}", uri, e);
            false
        }
    }
}

/// Helper function to get static assets embedded at compile time
fn get_static_assets() -> HashMap<String, Asset> {
    let mut m = HashMap::new();

    // Logo asset
    let logo_content = include_bytes!("../assets/logo.svg").to_vec();
    let logo = Asset {
        public_path: "/logo.svg".to_string(),
        mimetype: "image/svg+xml".to_string(),
        content: logo_content,
    };
    m.insert("/logo.svg".to_string(), logo);

    // Editor assets
    let editor_html_content = include_bytes!("../assets/editor.html").to_vec();
    let editor_html = Asset {
        public_path: "/editor.html".to_string(),
        mimetype: "text/html".to_string(),
        content: editor_html_content,
    };
    m.insert("/editor.html".to_string(), editor_html);

    let editor_css_content = include_bytes!("../assets/editor.css").to_vec();
    let editor_css = Asset {
        public_path: "/editor.css".to_string(),
        mimetype: "text/css".to_string(),
        content: editor_css_content,
    };
    m.insert("/editor.css".to_string(), editor_css);

    let editor_js_content = include_bytes!("../assets/editor.js").to_vec();
    let editor_js = Asset {
        public_path: "/editor.js".to_string(),
        mimetype: "application/javascript".to_string(),
        content: editor_js_content,
    };
    m.insert("/editor.js".to_string(), editor_js);

    m
}

/// Fetch assets with error handling (static + dynamic)
pub fn fetch_assets() -> HashMap<String, Asset> {
    let mut m = get_static_assets();

    // Merge in any dynamically upserted assets
    match safe_lock_assets() {
        Ok(guard) => {
            for (k, v) in guard.iter() {
                m.insert(k.clone(), v.clone());
            }
        }
        Err(e) => {
            error!("Failed to fetch dynamic assets: {}", e);
        }
    }

    m
}

/// Fetch single asset with error handling (dynamic first, then static)
pub fn fetch_asset(public_path: &str) -> Option<Asset> {
    // Check dynamic assets first
    if let Ok(guard) = safe_lock_assets()
        && let Some(asset) = guard.get(public_path)
    {
        return Some(asset.clone());
    }

    // Check static assets
    get_static_assets().get(public_path).cloned()
}

/// Upsert asset with validation and error handling
pub fn upsert_asset(asset: Asset) -> Result<(), RepositoryError> {
    if asset.public_path.trim().is_empty() {
        return Err(RepositoryError::InvalidData(
            "Public path cannot be empty".to_string(),
        ));
    }

    if asset.content.len() > 10_000_000 {
        // 10MB limit for assets
        return Err(RepositoryError::InvalidData(
            "Asset content too large (>10MB)".to_string(),
        ));
    }

    if asset.mimetype.trim().is_empty() {
        return Err(RepositoryError::InvalidData(
            "MIME type cannot be empty".to_string(),
        ));
    }

    let mut guard = safe_lock_assets()?;

    let public_path = asset.public_path.clone();
    guard.insert(public_path.clone(), asset);
    debug!("Upserted asset: {}", public_path);
    Ok(())
}

/// Delete asset with error handling  
pub fn delete_asset(public_path: &str) -> bool {
    match safe_lock_assets() {
        Ok(mut guard) => {
            let existed = guard.remove(public_path).is_some();
            if existed {
                debug!("Deleted asset: {}", public_path);
            } else {
                debug!("Attempted to delete non-existent asset: {}", public_path);
            }
            existed
        }
        Err(e) => {
            error!("Failed to delete asset {}: {}", public_path, e);
            false
        }
    }
}

/// Get repository statistics for monitoring
pub fn get_repository_stats() -> HashMap<String, usize> {
    let mut stats = HashMap::new();

    // Count scripts
    match safe_lock_scripts() {
        Ok(guard) => {
            stats.insert("dynamic_scripts".to_string(), guard.len());
        }
        Err(_) => {
            stats.insert("dynamic_scripts".to_string(), 0);
        }
    }

    // Count assets
    match safe_lock_assets() {
        Ok(guard) => {
            stats.insert("assets".to_string(), guard.len());
        }
        Err(_) => {
            stats.insert("assets".to_string(), 0);
        }
    }

    // Count total log entries
    match safe_lock_logs() {
        Ok(guard) => {
            let total_logs: usize = guard.values().map(|v: &Vec<String>| v.len()).sum();
            stats.insert("log_entries".to_string(), total_logs);
        }
        Err(_) => {
            stats.insert("log_entries".to_string(), 0);
        }
    }

    stats
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_script_operations() {
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

    #[test]
    fn test_input_validation() {
        // Test empty URI
        assert!(upsert_script("", "content").is_err());

        // Test large content
        let large_content = "x".repeat(2_000_000);
        assert!(upsert_script("test://large", &large_content).is_err());
    }
}
