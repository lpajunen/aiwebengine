//! The secrets a script asks people for, declared in its `init()`.
//!
//! A person's secret for a script is entered on the engine's own page
//! (`/auth/secret`), so the value goes straight to `user_secrets` and no
//! script ever holds it. The page asks only for a name the script declared
//! with `secretStorage.declare(name, { label })`: a link cannot make it ask
//! for anything the script did not, and the label it shows is the script's
//! own words rather than whatever a URL carried.
//!
//! Process-local and rebuilt as scripts initialise, like
//! [`crate::exposure`]'s refusals: each instance runs every script's `init()`,
//! so each knows what its scripts declared. Cleared before every
//! registration pass, so a declaration removed from `init()` is gone.

use std::collections::HashMap;
use std::sync::RwLock;

/// The longest secret name a script may declare.
pub const MAX_NAME_CHARS: usize = 100;
/// The longest label, which the page shows as the thing being asked for.
pub const MAX_LABEL_CHARS: usize = 80;

/// What one script declared: (name, label), in the order declared.
type Declared = Vec<(String, String)>;

/// Each script's declarations, by script URI.
static DECLARED: RwLock<Option<HashMap<String, Declared>>> = RwLock::new(None);

/// Why a declaration was refused, in words for the script's author.
pub fn validate(name: &str, label: &str) -> Result<(), String> {
    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
        return Err(format!(
            "a secret's name is 1 to {} characters",
            MAX_NAME_CHARS
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '.')
    {
        return Err("a secret's name is letters, digits, '_', '-' and '.'".to_string());
    }
    let label = label.trim();
    if label.is_empty() || label.chars().count() > MAX_LABEL_CHARS {
        return Err(format!(
            "a secret's label is 1 to {} characters",
            MAX_LABEL_CHARS
        ));
    }
    if label.chars().any(char::is_control) {
        return Err("a secret's label is one line of text".to_string());
    }
    Ok(())
}

/// Record that a script asks people for this secret. Declaring a name again
/// replaces its label.
pub fn declare(script_uri: &str, name: &str, label: &str) -> Result<(), String> {
    validate(name, label)?;
    let label = label.trim().to_string();
    let mut guard = match DECLARED.write() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let declared = guard
        .get_or_insert_with(HashMap::new)
        .entry(script_uri.to_string())
        .or_default();
    match declared.iter_mut().find(|(existing, _)| existing == name) {
        Some(entry) => entry.1 = label,
        None => declared.push((name.to_string(), label)),
    }
    Ok(())
}

/// Forget what a script declared, because it is about to declare again (or
/// has been renamed).
pub fn clear_for_script(script_uri: &str) {
    let mut guard = match DECLARED.write() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if let Some(map) = guard.as_mut() {
        map.remove(script_uri);
    }
}

/// The label of a secret the script declared, or `None` if it did not.
pub fn label_of(script_uri: &str, name: &str) -> Option<String> {
    let guard = match DECLARED.read() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    guard
        .as_ref()?
        .get(script_uri)?
        .iter()
        .find(|(existing, _)| existing == name)
        .map(|(_, label)| label.clone())
}

/// Where a person enters, changes or removes this secret for this script.
/// A script appends `&redirect=<path>` to bring them back.
pub fn entry_url(script_uri: &str, name: &str) -> String {
    format!(
        "/auth/secret?script={}&name={}",
        urlencoding::encode(script_uri),
        urlencoding::encode(name)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_declared_secret_has_its_label_until_the_script_declares_again() {
        let script = "declared-secrets-test-a";
        declare(script, "FINNHUB_API_KEY", "  Finnhub API key ").unwrap();
        assert_eq!(
            label_of(script, "FINNHUB_API_KEY").as_deref(),
            Some("Finnhub API key")
        );
        declare(script, "FINNHUB_API_KEY", "Market data key").unwrap();
        assert_eq!(
            label_of(script, "FINNHUB_API_KEY").as_deref(),
            Some("Market data key")
        );
        clear_for_script(script);
        assert_eq!(label_of(script, "FINNHUB_API_KEY"), None);
    }

    #[test]
    fn only_the_declaring_script_has_it() {
        declare("declared-secrets-test-b", "KEY", "A key").unwrap();
        assert_eq!(label_of("declared-secrets-test-c", "KEY"), None);
        assert_eq!(label_of("declared-secrets-test-b", "OTHER"), None);
    }

    #[test]
    fn refuses_names_and_labels_the_page_could_not_show_plainly() {
        assert!(validate("", "Label").is_err());
        assert!(validate("has space", "Label").is_err());
        assert!(validate("a/b", "Label").is_err());
        assert!(validate(&"x".repeat(MAX_NAME_CHARS + 1), "Label").is_err());
        assert!(validate("KEY", "  ").is_err());
        assert!(validate("KEY", "two\nlines").is_err());
        assert!(validate("KEY", &"x".repeat(MAX_LABEL_CHARS + 1)).is_err());
        assert!(validate("API_KEY.v2-x", "Key").is_ok());
    }

    #[test]
    fn the_entry_url_names_script_and_secret_encoded() {
        assert_eq!(
            entry_url("my app", "KEY"),
            "/auth/secret?script=my%20app&name=KEY"
        );
    }
}
