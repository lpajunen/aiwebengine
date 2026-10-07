//! How a host function answers JavaScript: values, refusals and errors, and the
//! argument readers they share.

use crate::security::{Capability, UserContext};
use rquickjs::{Result as JsResult, function::Opt};

/// A `{"error": "..."}` answer, built by the serializer rather than by string
/// formatting.
///
/// The messages that reach here carry quotes and newlines of their own — a
/// Postgres duplicate-key error names the constraint in quotes, and a
/// JavaScript exception arrives with its stack attached. Formatting one
/// straight into a JSON literal produced a string `JSON.parse` rejects, so
/// `.json()` threw on exactly the errors a script most needs to read, and the
/// only way to see one was to treat the answer as a string.
pub(super) fn error_answer(message: impl std::fmt::Display) -> String {
    serde_json::json!({ "error": message.to_string() }).to_string()
}

/// Read an optional argument a script may pass as `null` to skip it.
///
/// `Opt<T>` treats a *missing* argument as absent but refuses a literal
/// `null`, and the documented calling convention uses `null` to skip a
/// positional one — `query(table, null, 100, null, "asc")`. Skipping an
/// argument that way raised a type error naming a conversion the script never
/// asked for, so the only way to reach a later argument was to pass a value
/// for every earlier one.
pub(super) fn optional_arg<'js, T: rquickjs::FromJs<'js>>(
    value: Opt<rquickjs::Value<'js>>,
    name: &str,
) -> Result<Option<T>, String> {
    let Some(value) = value.0 else {
        return Ok(None);
    };
    if value.is_null() || value.is_undefined() {
        return Ok(None);
    }
    // Taken from the value rather than passed in: a separately supplied
    // context is a second lifetime, and the two are invariant.
    let ctx = value.ctx().clone();
    T::from_js(&ctx, value)
        .map(Some)
        .map_err(|e| format!("{} is not valid: {}", name, e))
}

/// Read an argument a script may pass either as JSON text or as the value that
/// text describes.
///
/// The host bindings behind `sendStreamMessage` and
/// `sendStreamMessageFiltered` took a `String`, while the type declarations
/// typed the same argument `any` and every example passed an object — so the
/// documented call raised `TypeError: Error converting from js 'object' into
/// type 'string'` out of the binding, QuickJS having no coercion to offer it.
/// Serializing here is what the declarations already promise ("will be JSON
/// serialized"), and a string is passed through untouched rather than being
/// wrapped in quotes, because every script written against the binding as it
/// was sends `JSON.stringify(...)` already.
pub(super) fn json_arg<'js>(value: rquickjs::Value<'js>, name: &str) -> JsResult<String> {
    if let Some(string) = value.as_string() {
        return string.to_string();
    }
    if value.is_undefined() || value.is_null() {
        return Ok(String::new());
    }
    let ctx = value.ctx().clone();
    match ctx.json_stringify(value) {
        Ok(Some(string)) => string.to_string(),
        // `JSON.stringify` answers `undefined` for a function, a symbol, and
        // for `undefined` itself. Naming the argument is the whole of the
        // diagnosis, so the error says which one could not be serialized.
        Ok(None) => Err(rquickjs::Error::new_from_js_message(
            "value",
            "json",
            &format!("{} cannot be serialized as JSON", name),
        )),
        Err(e) => Err(e),
    }
}

/// A `{"success": true, ...}` answer carrying `fields`.
///
/// Serialised for the same reason [`error_answer`] is: the table and column
/// names echoed back come from the script, and a name the engine would reject
/// still has to produce a readable answer saying so.
///
/// `fields` is expected to be an object; anything else contributes nothing, so
/// every call site passes a `json!({ ... })` literal.
pub(super) fn success_answer(fields: serde_json::Value) -> String {
    let mut answer = serde_json::Map::new();
    answer.insert("success".to_string(), serde_json::Value::Bool(true));
    if let serde_json::Value::Object(fields) = fields {
        answer.extend(fields);
    }
    serde_json::Value::Object(answer).to_string()
}

/// Why an API refused, when what it refused on was a capability.
///
/// Names the capability rather than saying "insufficient permissions",
/// because the caller of a narrowed context is usually the script itself and
/// the missing name is the whole of what it needs to know. And it says
/// *narrowed* when the context was attenuated: an administrator's script
/// being told it may not write a row is otherwise the most confusing message
/// the engine produces, and the reason is that it asked for this.
pub(super) fn capability_refusal(api: &str, capability: &Capability, user: &UserContext) -> String {
    if user.attenuated {
        format!(
            "{}: refused - this execution was narrowed and does not hold '{}'",
            api,
            capability.as_str()
        )
    } else {
        format!(
            "{}: refused - this caller does not hold '{}'",
            api,
            capability.as_str()
        )
    }
}

/// Refusal for a name that is not one of a fixed set.
///
/// Names what was asked for *and* what there is, because the caller who typed
/// `sha-256` cannot otherwise tell whether the problem is the hyphen or the
/// algorithm.
pub(super) fn unknown_name_error(
    api: &str,
    what: &str,
    given: &str,
    known: &[&str],
) -> rquickjs::Error {
    rquickjs::Error::new_from_js_message(
        "crypto",
        "type_error",
        &format!(
            "{}: '{}' is not a known {} — one of {}",
            api,
            given,
            what,
            known.join(", ")
        ),
    )
}

/// [`capability_refusal`] as a thrown JavaScript error, for the APIs that
/// throw rather than returning an envelope.
pub(super) fn capability_error(
    api: &'static str,
    capability: &Capability,
    user: &UserContext,
) -> rquickjs::Error {
    rquickjs::Error::new_from_js_message(
        api,
        "capability",
        &capability_refusal(api, capability, user),
    )
}

/// Reply for a registration call made outside the registration phase.
///
/// Registration APIs stay callable everywhere so that top-level script code
/// keeps working, but only the registration phase writes to the registry.
pub(super) fn registration_inactive(api: &str, name: &str) -> String {
    format!(
        "{}: '{}' not registered - registration only takes effect during script \
         startup and init()",
        api, name
    )
}

/// `{ok: value}` — the envelope every host-object prelude unwraps into a
/// return value.
pub(super) fn host_ok(value: serde_json::Value) -> String {
    serde_json::json!({ "ok": value }).to_string()
}

/// `{error: {name, message}}` — the envelope a prelude turns into a thrown
/// error of that name. A host binding cannot throw a JavaScript exception of
/// the right type, which is why the throw happens on the JavaScript side.
pub(super) fn host_failure(name: &str, message: &str) -> String {
    serde_json::json!({ "error": { "name": name, "message": message } }).to_string()
}

/// `{ ok: false, reason }` in the host envelope: a registration refused for
/// a reason the script is told about rather than thrown at.
pub(super) fn refusal_answer(reason: String) -> String {
    host_ok(serde_json::json!({ "ok": false, "reason": reason }))
}

#[cfg(test)]
mod json_arg_tests {
    use super::json_arg;
    use rquickjs::{Context, Runtime};

    /// Evaluate `expr` and marshal the result the way a host binding does.
    fn marshal(expr: &str) -> Result<String, String> {
        let rt = Runtime::new().expect("runtime");
        let ctx = Context::full(&rt).expect("context");
        ctx.with(|ctx| {
            let value = ctx.eval::<rquickjs::Value<'_>, _>(expr).expect("eval");
            json_arg(value, "data").map_err(|e| e.to_string())
        })
    }

    #[test]
    fn an_object_is_serialized_the_way_the_declarations_promise() {
        assert_eq!(
            marshal("({ type: 'alert', n: 1 })"),
            Ok(r#"{"type":"alert","n":1}"#.to_string())
        );
    }

    /// The scripts that exist send `JSON.stringify(...)`, and re-encoding that
    /// would deliver a quoted string to every listener reading it.
    #[test]
    fn a_string_is_the_message_rather_than_a_value_to_encode() {
        assert_eq!(
            marshal(r#"JSON.stringify({ a: 1 })"#),
            Ok(r#"{"a":1}"#.to_string())
        );
        assert_eq!(marshal("'plain text'"), Ok("plain text".to_string()));
    }

    #[test]
    fn arrays_and_scalars_serialize_as_themselves() {
        assert_eq!(marshal("[1, 2]"), Ok("[1,2]".to_string()));
        assert_eq!(marshal("42"), Ok("42".to_string()));
        assert_eq!(marshal("true"), Ok("true".to_string()));
    }

    #[test]
    fn nothing_at_all_is_the_empty_message() {
        assert_eq!(marshal("undefined"), Ok(String::new()));
        assert_eq!(marshal("null"), Ok(String::new()));
    }

    /// `JSON.stringify` has no answer for a function, and the caller is told
    /// which argument it could not serialize rather than being handed the
    /// conversion error QuickJS would have raised.
    #[test]
    fn a_value_json_cannot_describe_names_the_argument() {
        let error = marshal("(function () {})").expect_err("a function has no JSON");
        assert!(
            error.contains("data"),
            "the refusal should name the argument, got: {}",
            error
        );
    }
}
