use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// Fetch scripts from a repository.
/// For now this returns two hard-coded scripts.
pub fn fetch_scripts() -> HashMap<String, String> {
    let mut m = HashMap::new();
    // embed scripts at compile time
    let core = include_str!("../scripts/core.js");
    let helloworld = include_str!("../scripts/helloworld.js");

    m.insert("https://example.com/core".to_string(), core.to_string());
    m.insert("https://example.com/helloworld".to_string(), helloworld.to_string());
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
        "https://example.com/helloworld" => Some(include_str!("../scripts/helloworld.js").to_string()),
        _ => None,
    }
}

static DYNAMIC_SCRIPTS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

// simple in-memory log store
static DYNAMIC_LOGS: OnceLock<Mutex<Vec<String>>> = OnceLock::new();

/// Insert a log message into the in-memory log store.
pub fn insert_log_message(msg: &str) {
    let store = DYNAMIC_LOGS.get_or_init(|| Mutex::new(Vec::new()));
    let mut guard = store.lock().expect("dynamic logs mutex poisoned");
    guard.push(msg.to_string());
}

/// Fetch all log messages currently stored. Returns a vector of messages.
pub fn fetch_log_messages() -> Vec<String> {
    if let Some(store) = DYNAMIC_LOGS.get() {
        let guard = store.lock().expect("dynamic logs mutex poisoned");
        return guard.clone();
    }
    Vec::new()
}

/// Keep only the latest `limit` log messages (default 20) and remove older ones.
pub fn prune_log_messages() {
    const LIMIT: usize = 20;
    if let Some(store) = DYNAMIC_LOGS.get() {
        let mut guard = store.lock().expect("dynamic logs mutex poisoned");
        if guard.len() > LIMIT {
            let remove = guard.len() - LIMIT;
            // remove older entries from the front
            guard.drain(0..remove);
        }
    }
}

/// Insert or update a script dynamically at runtime.
pub fn upsert_script(uri: &str, script_content: &str) {
    let store = DYNAMIC_SCRIPTS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = store.lock().expect("dynamic scripts mutex poisoned");
    guard.insert(uri.to_string(), script_content.to_string());
}

/// Delete a dynamically upserted script. Returns true if a script was removed.
pub fn delete_script(uri: &str) -> bool {
    if let Some(store) = DYNAMIC_SCRIPTS.get() {
        let mut guard = store.lock().expect("dynamic scripts mutex poisoned");
        return guard.remove(uri).is_some();
    }
    false
}
