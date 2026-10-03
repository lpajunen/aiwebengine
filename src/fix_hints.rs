//! A sentence naming the usual fix for a runtime error a script author meets.
//!
//! QuickJS errors say what went wrong in the language's terms: `x is not
//! defined`, `cannot read property 'query' of undefined`. They do not say what
//! that means on this engine, which is the part a small model cannot guess. The
//! hints here are appended to the errors a script's author reads —
//! `check_script`'s `init-failed` and a failing test — and only where the
//! message matches a pattern whose usual cause is known; anything else is left
//! as it was.

/// The error with a `Hint:` line appended when one applies.
pub fn with_hint(error: &str) -> String {
    match hint_for(error) {
        Some(hint) => format!("{}\nHint: {}", error.trim_end(), hint),
        None => error.to_string(),
    }
}

fn hint_for(error: &str) -> Option<&'static str> {
    let lower = error.to_lowercase();
    if lower.contains("hint:") {
        return None;
    }
    if lower.contains("is not defined") {
        return Some(
            "that name is not a global of the engine or of this file. Declare it, or import it \
             from another file of the script with its extension \
             (`import { name } from \"./lib/name.ts\"`). The engine's globals are listed in \
             script-primer.md and aiwebengine.d.ts.",
        );
    }
    if lower.contains("cannot read propert")
        || lower.contains("of undefined") && lower.contains("read")
    {
        return Some(
            "a value was undefined or null. `context.request` and `context.request.auth` may be \
             absent, so write `context.request?.query.name`. A test passes a handler only the \
             context it builds, so include the fields the handler reads.",
        );
    }
    if lower.contains("not a function") {
        return Some(
            "something that is not a function was called. If it is a name imported from \
             another file, that file does not export it under that name (an import of a \
             missing export is undefined, not an error), so check the export and the \
             import. Otherwise check the name against aiwebengine.d.ts: \
             `ResponseBuilder.json`, `routeRegistry.registerRoute`, `scriptStorage.getItem` — \
             and note `fetch` answers directly, with no `await`.",
        );
    }
    if lower.contains("securityerror") {
        return Some(
            "`personalStorage` needs a signed-in user. Check `context.request.auth.isAuthenticated` \
             first and answer 401 otherwise; in a test there is no signed-in user.",
        );
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_error_gets_a_hint_on_its_own_line() {
        let text = with_hint("ReferenceError: helper is not defined\n    at init (main.ts:3:5)");
        assert!(text.starts_with("ReferenceError: helper is not defined"));
        assert!(text.contains("\nHint: that name is not a global"));
    }

    #[test]
    fn quickjs_bare_not_a_function_gets_the_import_hint() {
        let text = with_hint("not a function, \nStack: at <anonymous> (x.test.ts:3:5)");
        assert!(
            text.contains("Hint: something that is not a function"),
            "{text}"
        );
        assert!(text.contains("does not export it"), "{text}");
    }

    #[test]
    fn an_unrecognised_error_is_left_alone() {
        assert_eq!(with_hint("Expected 0, got -1"), "Expected 0, got -1");
    }

    #[test]
    fn a_hint_is_not_added_twice() {
        let once = with_hint("x is not defined");
        assert_eq!(with_hint(&once), once);
    }
}
