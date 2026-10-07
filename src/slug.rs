//! What a script is called.
//!
//! A script's identifier is a name. Where it publishes is `script_hosts`, and
//! what language its entrypoint is in is the entrypoint file's name
//! (`main.ts`), so nothing about either belongs in the identifier.
//!
//! A slug is flat — `shop`, not `acme/shop`. A name with a slash is a path, and
//! a path invites the question of what the prefix means, which the engine has
//! deliberately not answered (there are no mounts: every script publishes at
//! `/` of the hosts it is bound to). Two people who both want `shop` take it in
//! turn; the one who comes second picks another name.
//!
//! **Enforced where a script comes into being** (create, pull, rename), not
//! where it is stored, so a stored identifier that is not a slug keeps working
//! and `rename_script` moves it to one when its owner chooses.

/// Longest slug accepted.
pub const MAX_SLUG_LEN: usize = 64;

/// Names the engine uses for itself.
///
/// `core` holds the engine's compiled-in assets (the favicon, the type
/// definitions) and `server` is what the engine's own log lines are filed
/// under. A script by either name would be mistaken for the engine's.
pub const RESERVED: &[&str] = &["core", "server", "engine", "native", "system"];

/// Whether `name` is an acceptable name for a new script.
///
/// Lower-case letters, digits, `-` and `_`, starting with a letter or digit.
/// Lower-case only so `Shop` and `shop` cannot be two scripts that look like
/// one.
pub fn validate(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("a script's name cannot be empty".to_string());
    }
    if name.len() > MAX_SLUG_LEN {
        return Err(format!(
            "'{}…' is not a valid script name: at most {} characters",
            &name[..name.char_indices().nth(24).map_or(name.len(), |(i, _)| i)],
            MAX_SLUG_LEN
        ));
    }
    let mut chars = name.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    let rest_ok = name
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_');
    if !first_ok || !rest_ok {
        return Err(format!(
            "'{}' is not a valid script name: use lower-case letters, digits, '-' and '_', \
             starting with a letter or digit (a slug such as 'shop', not a URL or a path)",
            name
        ));
    }
    if RESERVED.contains(&name) {
        return Err(format!(
            "'{}' is not a valid script name: it is reserved for the engine",
            name
        ));
    }
    Ok(())
}

/// The slug a legacy identifier would be given: its last path segment without
/// the extension, lower-cased, with anything else turned into `-`.
///
/// For tools that propose a rename. It is a suggestion and can collide with
/// another script's, which is for the caller to notice.
pub fn suggest(identifier: &str) -> String {
    let last = identifier
        .trim_end_matches('/')
        .rsplit('/')
        .next()
        .unwrap_or(identifier);
    let stem = match last.rsplit_once('.') {
        Some((stem, ext)) if matches!(ext, "ts" | "js" | "tsx" | "jsx") && !stem.is_empty() => stem,
        _ => last,
    };
    let mut slug: String = stem
        .chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    slug.truncate(MAX_SLUG_LEN);
    slug.trim_start_matches(['-', '_']).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_flat_lowercase_name_is_a_slug() {
        for name in ["shop", "virtual-world", "chat_app", "a1", "2fa"] {
            assert!(validate(name).is_ok(), "{name}");
        }
    }

    #[test]
    fn urls_paths_and_extensions_are_not() {
        for name in [
            "",
            "https://example.com/shop",
            "acme/shop",
            "shop.ts",
            "Shop",
            "-shop",
            "_shop",
            "has space",
            "test://x",
        ] {
            assert!(validate(name).is_err(), "{name}");
        }
        assert!(validate(&"a".repeat(65)).is_err());
        assert!(validate(&"a".repeat(64)).is_ok());
    }

    #[test]
    fn the_engines_own_names_are_reserved() {
        for name in RESERVED {
            let message = validate(name).expect_err(name);
            assert!(message.contains("reserved"), "{message}");
        }
    }

    #[test]
    fn a_legacy_identifier_suggests_its_name() {
        assert_eq!(
            suggest("https://example.com/virtual-world"),
            "virtual-world"
        );
        assert_eq!(
            suggest("https://softagen.com/aiwebengine-agent/agent.ts"),
            "agent"
        );
        assert_eq!(suggest("https://example.com/Chat App.js"), "chat-app");
        assert_eq!(suggest("https://example.com/"), "example-com");
    }
}
