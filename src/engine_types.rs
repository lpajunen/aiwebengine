//! The engine's operations, as TypeScript.
//!
//! `engine.call(name, args)` takes any string and any object, which is right
//! for a function that dispatches into a table but leaves a script author to
//! find an argument's name by reading a document. The table already says what
//! every operation takes — it is what `tools/list` and `/engine/openapi.json`
//! publish — so the declarations are rendered from it too, at the moment the
//! type definitions are served. They describe the engine that serves them: an
//! operation this deployment does not have is not in them.

use serde_json::Value;

/// The marker in `aiwebengine.d.ts` the members are rendered in place of.
pub const OPERATIONS_PLACEHOLDER: &str = "// {{engine.operations}}";

/// `EngineOperations` members, one per operation, indented to sit inside the
/// interface.
pub fn render_operations() -> String {
    let mut operations = crate::engine_api::native_mcp_tool_descriptors();
    operations.sort_by_key(|operation| operation.name);
    operations
        .iter()
        .map(|operation| {
            let summary = operation
                .description
                .split(". ")
                .next()
                .unwrap_or(operation.description)
                .replace("*/", "*\\/");
            format!(
                "  /** {} */\n  {}: {};",
                shorten(summary.trim_end_matches('.'), 110),
                quote(operation.name),
                object_type(&operation.input_schema, 2)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The type definitions with the operations filled in.
pub fn render(declarations: &str) -> String {
    match declarations.find(OPERATIONS_PLACEHOLDER) {
        Some(_) => declarations.replace(OPERATIONS_PLACEHOLDER, render_operations().trim_start()),
        None => declarations.to_string(),
    }
}

/// `text` cut at a word boundary, for a one-line summary.
fn shorten(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    let cut = cut.rsplit_once(' ').map_or(cut.as_str(), |(head, _)| head);
    format!("{}…", cut.trim_end_matches([',', ':', ';']))
}

fn quote(name: &str) -> String {
    serde_json::to_string(name).unwrap_or_else(|_| format!("\"{}\"", name))
}

/// An object schema as an inline TypeScript type.
fn object_type(schema: &Value, indent: usize) -> String {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return "Record<string, unknown>".to_string();
    };
    if properties.is_empty() {
        return "Record<string, never>".to_string();
    }
    let required: Vec<&str> = schema
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let pad = " ".repeat(indent + 2);
    let mut members: Vec<(&String, &Value)> = properties.iter().collect();
    // Required first, then by name: the order a person reads an argument list.
    members.sort_by_key(|(name, _)| (!required.contains(&name.as_str()), name.to_string()));
    let body = members
        .iter()
        .map(|(name, schema)| {
            let doc = schema
                .get("description")
                .and_then(Value::as_str)
                .map(|d| format!("{}/** {} */\n", pad, d.replace("*/", "*\\/")))
                .unwrap_or_default();
            format!(
                "{}{}{}{}: {};",
                doc,
                pad,
                member_name(name),
                if required.contains(&name.as_str()) {
                    ""
                } else {
                    "?"
                },
                type_of(schema, indent + 2)
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("{{\n{}\n{}}}", body, " ".repeat(indent))
}

fn member_name(name: &str) -> String {
    let plain = name
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    if plain { name.to_string() } else { quote(name) }
}

fn type_of(schema: &Value, indent: usize) -> String {
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        let literals: Vec<String> = values
            .iter()
            .filter_map(|v| match v {
                Value::String(s) => Some(quote(s)),
                Value::Number(n) => Some(n.to_string()),
                Value::Bool(b) => Some(b.to_string()),
                _ => None,
            })
            .collect();
        if !literals.is_empty() {
            return literals.join(" | ");
        }
    }
    match schema.get("type").and_then(Value::as_str) {
        Some("string") => "string".to_string(),
        Some("integer") | Some("number") => "number".to_string(),
        Some("boolean") => "boolean".to_string(),
        Some("array") => {
            let item = schema
                .get("items")
                .map(|items| type_of(items, indent))
                .unwrap_or_else(|| "unknown".to_string());
            if item.contains(" | ") || item.contains('{') {
                format!("({})[]", item)
            } else {
                format!("{}[]", item)
            }
        }
        Some("object") => object_type(schema, indent),
        _ => "unknown".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_object_schema_becomes_a_type_with_required_first() {
        let schema = json!({
            "type": "object",
            "properties": {
                "limit": { "type": "integer", "description": "How many" },
                "script": { "type": "string" },
                "level": { "type": "string", "enum": ["info", "error"] }
            },
            "required": ["script"]
        });
        let rendered = object_type(&schema, 0);
        let script = rendered.find("script: string").expect("script");
        let level = rendered
            .find("level?: \"info\" | \"error\"")
            .expect("level");
        let limit = rendered.find("limit?: number").expect("limit");
        assert!(script < level && level < limit, "{rendered}");
        assert!(rendered.contains("/** How many */"));
    }

    #[test]
    fn arrays_and_nested_objects_nest() {
        let schema = json!({
            "type": "object",
            "properties": {
                "hosts": { "type": "array", "items": { "type": "string" } },
                "edits": { "type": "array", "items": {
                    "type": "object",
                    "properties": { "old_string": { "type": "string" } },
                    "required": ["old_string"]
                } }
            }
        });
        let rendered = object_type(&schema, 0);
        assert!(rendered.contains("hosts?: string[]"), "{rendered}");
        assert!(rendered.contains("edits?: ({"), "{rendered}");
    }

    #[test]
    fn a_schema_with_no_properties_takes_anything_or_nothing() {
        assert_eq!(
            object_type(&json!({ "type": "object" }), 0),
            "Record<string, unknown>"
        );
        assert_eq!(
            object_type(&json!({ "type": "object", "properties": {} }), 0),
            "Record<string, never>"
        );
    }

    #[test]
    fn every_operation_is_declared() {
        let rendered = render_operations();
        for operation in crate::engine_api::native_mcp_tool_descriptors() {
            assert!(
                rendered.contains(&format!("\"{}\":", operation.name)),
                "{} is not declared",
                operation.name
            );
        }
    }

    #[test]
    #[ignore = "writes the rendered declarations to $DUMP_DTS for a tsc check by hand"]
    fn dump_for_tsc() {
        if let Ok(path) = std::env::var("DUMP_DTS") {
            let text = include_str!("../assets/aiwebengine.d.ts");
            std::fs::write(path, render(&crate::limits::render_placeholders(text)))
                .expect("the declarations should be written");
        }
    }

    #[test]
    fn a_long_summary_is_cut_at_a_word() {
        let cut = shorten(&"word ".repeat(40), 20);
        assert!(cut.ends_with('…') && cut.chars().count() <= 21, "{cut}");
        assert_eq!(shorten("short", 20), "short");
    }

    #[test]
    fn the_declarations_are_left_alone_without_the_marker() {
        assert_eq!(render("declare var x: 1;"), "declare var x: 1;");
    }
}
