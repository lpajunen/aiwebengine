use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use tracing::debug;

/// Fetch scripts from a repository.
/// For now this returns hard-coded scripts, excluding test scripts unless in test mode.
pub fn fetch_scripts() -> HashMap<String, String> {
    let mut m = HashMap::new();

    // Always include core functionality scripts
    let core = include_str!("../scripts/core.js");
    let asset_mgmt = include_str!("../scripts/asset_mgmt.js");
    let editor = include_str!("../scripts/editor.js");

    m.insert("https://example.com/core".to_string(), core.to_string());
    m.insert(
        "https://example.com/asset_mgmt".to_string(),
        asset_mgmt.to_string(),
    );
    m.insert("https://example.com/editor".to_string(), editor.to_string());

    // Only include test scripts when running tests or when explicitly requested
    let include_test_scripts =
        std::env::var("AIWEBENGINE_INCLUDE_TEST_SCRIPTS").is_ok() || cfg!(test);

    if include_test_scripts {
        // Include GraphQL test script for testing GraphiQL integration
        let graphql_test = include_str!("../scripts/graphql_test.js");
        m.insert(
            "https://example.com/graphql_test".to_string(),
            graphql_test.to_string(),
        );

        // Note: test_editor.js and test_editor_api.js are now loaded dynamically via upsert_script
        // in test setup functions rather than being included statically here
    }

    // merge in any dynamically upserted scripts
    if let Some(store) = DYNAMIC_SCRIPTS.get() {
        let guard = store.lock().expect("dynamic scripts mutex poisoned");
        for (k, v) in guard.iter() {
            m.insert(k.clone(), v.clone());
        }
    }

    m
}

/// Fetch a single script by its resource URI.
/// Returns `Some(script_content)` when the URI is known, otherwise `None`.
pub fn fetch_script(uri: &str) -> Option<String> {
    // check dynamic store first
    if let Some(store) = DYNAMIC_SCRIPTS.get() {
        let guard = store.lock().expect("dynamic scripts mutex poisoned");
        if let Some(v) = guard.get(uri) {
            return Some(v.clone());
        }
    }

    match uri {
        "https://example.com/core" => Some(include_str!("../scripts/core.js").to_string()),
        "https://example.com/asset_mgmt" => {
            Some(include_str!("../scripts/asset_mgmt.js").to_string())
        }
        "https://example.com/editor" => Some(include_str!("../scripts/editor.js").to_string()),
        // Note: test_editor and test_editor_api are now loaded dynamically via upsert_script
        _ => None,
    }
}

static DYNAMIC_SCRIPTS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

// simple in-memory log store
static DYNAMIC_LOGS: OnceLock<Mutex<HashMap<String, Vec<String>>>> = OnceLock::new();

/// Insert a log message into the in-memory log store for a specific script URI.
pub fn insert_log_message(uri: &str, msg: &str) {
    let store = DYNAMIC_LOGS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = store.lock().expect("dynamic logs mutex poisoned");
    guard
        .entry(uri.to_string())
        .or_insert_with(Vec::new)
        .push(msg.to_string());
}

/// Fetch all log messages currently stored for a specific script URI. Returns a vector of messages.
pub fn fetch_log_messages(uri: &str) -> Vec<String> {
    if let Some(store) = DYNAMIC_LOGS.get() {
        let guard = store.lock().expect("dynamic logs mutex poisoned");
        if let Some(logs) = guard.get(uri) {
            return logs.clone();
        }
    }
    Vec::new()
}

/// Keep only the latest `limit` log messages (default 20) for each script URI and remove older ones.
pub fn prune_log_messages() {
    const LIMIT: usize = 20;
    if let Some(store) = DYNAMIC_LOGS.get() {
        let mut guard = store.lock().expect("dynamic logs mutex poisoned");
        for logs in guard.values_mut() {
            if logs.len() > LIMIT {
                let remove = logs.len() - LIMIT;
                // remove older entries from the front
                logs.drain(0..remove);
            }
        }
    }
}

/// Insert or update a script dynamically at runtime.
pub fn upsert_script(uri: &str, script_content: &str) {
    let store = DYNAMIC_SCRIPTS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = store.lock().expect("dynamic scripts mutex poisoned");
    debug!("repository::upsert_script called for {}", uri);
    guard.insert(uri.to_string(), script_content.to_string());
}

/// Delete a dynamically upserted script. Returns true if a script was removed.
pub fn delete_script(uri: &str) -> bool {
    if let Some(store) = DYNAMIC_SCRIPTS.get() {
        let mut guard = store.lock().expect("dynamic scripts mutex poisoned");
        let existed = guard.remove(uri).is_some();
        debug!(
            "repository::delete_script called for {} -> existed={}",
            uri, existed
        );
        return existed;
    }
    false
}

/// Load test scripts dynamically using upsert_script.
/// This function should only be called during tests to avoid loading test scripts in production.
pub fn load_test_scripts() {
    // Only load test scripts when running tests or when explicitly requested
    let include_test_scripts =
        std::env::var("AIWEBENGINE_INCLUDE_TEST_SCRIPTS").is_ok() || cfg!(test);

    if include_test_scripts {
        upsert_script(
            "https://example.com/test_editor",
            include_str!("../scripts/test_editor.js"),
        );
        upsert_script(
            "https://example.com/test_editor_api",
            include_str!("../scripts/test_editor_api.js"),
        );
    }
}

#[derive(Clone)]
pub struct Asset {
    pub public_path: String,
    pub mimetype: String,
    pub content: Vec<u8>,
}

static DYNAMIC_ASSETS: OnceLock<Mutex<HashMap<String, Asset>>> = OnceLock::new();

/// Fetch all assets from the repository.
/// Returns a HashMap of public_path to Asset, including static and dynamic assets.
pub fn fetch_assets() -> HashMap<String, Asset> {
    let mut m = get_static_assets();
    // merge in any dynamically upserted assets
    if let Some(store) = DYNAMIC_ASSETS.get() {
        let guard = store.lock().expect("dynamic assets mutex poisoned");
        for (k, v) in guard.iter() {
            m.insert(k.clone(), v.clone());
        }
    }
    m
}

/// Fetch a single asset by its public path.
/// Returns `Some(asset)` when the path is known, otherwise `None`.
pub fn fetch_asset(public_path: &str) -> Option<Asset> {
    // check dynamic store first
    if let Some(store) = DYNAMIC_ASSETS.get() {
        let guard = store.lock().expect("dynamic assets mutex poisoned");
        if let Some(v) = guard.get(public_path) {
            return Some(v.clone());
        }
    }

    // check static assets
    match public_path {
        "/logo.svg" => {
            let content = include_bytes!("../assets/logo.svg").to_vec();
            Some(Asset {
                public_path: "/logo.svg".to_string(),
                mimetype: "image/svg+xml".to_string(),
                content,
            })
        }
        "/editor.html" => {
            let content = include_bytes!("../assets/editor.html").to_vec();
            Some(Asset {
                public_path: "/editor.html".to_string(),
                mimetype: "text/html".to_string(),
                content,
            })
        }
        "/editor.css" => {
            let content = include_bytes!("../assets/editor.css").to_vec();
            Some(Asset {
                public_path: "/editor.css".to_string(),
                mimetype: "text/css".to_string(),
                content,
            })
        }
        "/editor.js" => {
            let content = include_bytes!("../assets/editor.js").to_vec();
            Some(Asset {
                public_path: "/editor.js".to_string(),
                mimetype: "application/javascript".to_string(),
                content,
            })
        }
        _ => None,
    }
}

/// Insert or update an asset dynamically at runtime.
pub fn upsert_asset(asset: Asset) {
    let store = DYNAMIC_ASSETS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = store.lock().expect("dynamic assets mutex poisoned");
    debug!("repository::upsert_asset called for {}", asset.public_path);
    guard.insert(asset.public_path.clone(), asset);
}

/// Delete a dynamically upserted asset. Returns true if an asset was removed.
pub fn delete_asset(public_path: &str) -> bool {
    if let Some(store) = DYNAMIC_ASSETS.get() {
        let mut guard = store.lock().expect("dynamic assets mutex poisoned");
        let existed = guard.remove(public_path).is_some();
        debug!(
            "repository::delete_asset called for {} -> existed={}",
            public_path, existed
        );
        return existed;
    }
    false
}

/// Helper function to get static assets embedded at compile time.
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
