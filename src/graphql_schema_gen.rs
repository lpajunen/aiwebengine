//! GraphQL schema and resolver generation from database tables
//!
//! This module automatically generates GraphQL type definitions, queries, and mutations
//! for script-created database tables, including support for foreign key relationships.
use crate::repository::{ForeignKeyInfo, TableSchema};

#[cfg(test)]
use crate::repository::ColumnInfo;

/// Generated GraphQL operation definition
#[derive(Debug, Clone)]
pub struct GeneratedOperation {
    pub name: String,
    pub sdl: String,
    pub resolver_function_name: String,
    pub resolver_code: String,
}

/// Complete set of generated GraphQL operations for a table
#[derive(Debug, Clone)]
pub struct GeneratedTableOperations {
    pub queries: Vec<GeneratedOperation>,
    pub mutations: Vec<GeneratedOperation>,
    pub types_sdl: String,
}

/// Generate complete GraphQL operations for a database table
pub fn generate_table_operations(
    table_name: &str,
    schema: &TableSchema,
    foreign_keys: &[ForeignKeyInfo],
) -> GeneratedTableOperations {
    let type_name = to_pascal_case(table_name);
    let input_type_name = format!("{}Input", type_name);

    // Generate type definitions
    let types_sdl = generate_type_definitions(table_name, schema, foreign_keys);

    // Generate queries
    let get_query = generate_get_query(table_name, &type_name, schema);
    let list_query = generate_list_query(table_name, &type_name, schema);

    // Generate mutations
    let create_mutation =
        generate_create_mutation(table_name, &type_name, &input_type_name, schema);
    let update_mutation =
        generate_update_mutation(table_name, &type_name, &input_type_name, schema);
    let delete_mutation = generate_delete_mutation(table_name, &type_name, schema);

    GeneratedTableOperations {
        queries: vec![get_query, list_query],
        mutations: vec![create_mutation, update_mutation, delete_mutation],
        types_sdl,
    }
}

/// Generate GraphQL type definitions for a table
fn generate_type_definitions(
    table_name: &str,
    schema: &TableSchema,
    foreign_keys: &[ForeignKeyInfo],
) -> String {
    let type_name = to_pascal_case(table_name);
    let input_type_name = format!("{}Input", type_name);

    let mut fields = Vec::new();
    let mut input_fields = Vec::new();

    for column in &schema.columns {
        if column.is_primary_key {
            // ID field is not nullable and auto-generated
            fields.push(format!("  {}: Int!", column.name));
            continue;
        }

        let graphql_type = map_column_to_graphql_type(&column.data_type);
        let nullable_suffix = if column.nullable { "" } else { "!" };

        // Check if this column is a foreign key
        if let Some(fk) = foreign_keys.iter().find(|fk| fk.column_name == column.name) {
            let referenced_type = to_pascal_case(&fk.referenced_table_logical);
            fields.push(format!(
                "  {}: {}{}",
                column.name, referenced_type, nullable_suffix
            ));
            input_fields.push(format!("  {}: Int{}", column.name, nullable_suffix));
        } else {
            fields.push(format!(
                "  {}: {}{}",
                column.name, graphql_type, nullable_suffix
            ));
            input_fields.push(format!(
                "  {}: {}{}",
                column.name, graphql_type, nullable_suffix
            ));
        }
    }

    format!(
        "type {} {{\n{}\n}}\n\ninput {} {{\n{}\n}}",
        type_name,
        fields.join("\n"),
        input_type_name,
        input_fields.join("\n")
    )
}

/// Generate get-by-ID query
fn generate_get_query(
    table_name: &str,
    type_name: &str,
    _schema: &TableSchema,
) -> GeneratedOperation {
    let query_name = format!("get{}", type_name);
    let resolver_name = format!("{}Resolver", query_name);

    let sdl = format!(
        "type Query {{\n  {}(id: Int!): {}\n}}",
        query_name, type_name
    );

    let resolver_code = format!(
        r#"function {resolver_name}(args, context) {{
  const result = database.query("{table_name}", {{ id: args.id }}, 1);
  return result.length > 0 ? result[0] : null;
}}"#
    );

    GeneratedOperation {
        name: query_name.clone(),
        sdl,
        resolver_function_name: resolver_name,
        resolver_code,
    }
}

/// Generate list query
fn generate_list_query(
    table_name: &str,
    type_name: &str,
    _schema: &TableSchema,
) -> GeneratedOperation {
    let query_name = format!("list{}s", type_name);
    let resolver_name = format!("{}Resolver", query_name);

    let sdl = format!(
        "type Query {{\n  {}(limit: Int): [{}!]!\n}}",
        query_name, type_name
    );

    let resolver_code = format!(
        r#"function {resolver_name}(args, context) {{
  const limit = args.limit || 100;
  return database.query("{table_name}", null, limit);
}}"#
    );

    GeneratedOperation {
        name: query_name.clone(),
        sdl,
        resolver_function_name: resolver_name,
        resolver_code,
    }
}

/// Generate create mutation
fn generate_create_mutation(
    table_name: &str,
    type_name: &str,
    input_type_name: &str,
    _schema: &TableSchema,
) -> GeneratedOperation {
    let mutation_name = format!("create{}", type_name);
    let resolver_name = format!("{}Resolver", mutation_name);

    let sdl = format!(
        "type Mutation {{\n  {}(input: {}!): {}!\n}}",
        mutation_name, input_type_name, type_name
    );

    let resolver_code = format!(
        r#"function {resolver_name}(args, context) {{
  return database.insert("{table_name}", args.input);
}}"#
    );

    GeneratedOperation {
        name: mutation_name.clone(),
        sdl,
        resolver_function_name: resolver_name,
        resolver_code,
    }
}

/// Generate update mutation
fn generate_update_mutation(
    table_name: &str,
    type_name: &str,
    input_type_name: &str,
    _schema: &TableSchema,
) -> GeneratedOperation {
    let mutation_name = format!("update{}", type_name);
    let resolver_name = format!("{}Resolver", mutation_name);

    let sdl = format!(
        "type Mutation {{\n  {}(id: Int!, input: {}!): {}!\n}}",
        mutation_name, input_type_name, type_name
    );

    let resolver_code = format!(
        r#"function {resolver_name}(args, context) {{
  return database.update("{table_name}", args.id, args.input);
}}"#
    );

    GeneratedOperation {
        name: mutation_name.clone(),
        sdl,
        resolver_function_name: resolver_name,
        resolver_code,
    }
}

/// Generate delete mutation
fn generate_delete_mutation(
    table_name: &str,
    type_name: &str,
    _schema: &TableSchema,
) -> GeneratedOperation {
    let mutation_name = format!("delete{}", type_name);
    let resolver_name = format!("{}Resolver", mutation_name);

    let sdl = format!(
        "type Mutation {{\n  {}(id: Int!): Boolean!\n}}",
        mutation_name
    );

    let resolver_code = format!(
        r#"function {resolver_name}(args, context) {{
  return database.delete("{table_name}", args.id);
}}"#
    );

    GeneratedOperation {
        name: mutation_name.clone(),
        sdl,
        resolver_function_name: resolver_name,
        resolver_code,
    }
}

/// Map database column type to GraphQL scalar type
fn map_column_to_graphql_type(column_type: &str) -> &'static str {
    match column_type.to_uppercase().as_str() {
        "INTEGER" | "INT" | "SERIAL" => "Int",
        "TEXT" | "STRING" => "String",
        "BOOLEAN" | "BOOL" => "Boolean",
        "TIMESTAMPTZ" | "TIMESTAMP" => "String", // ISO 8601 string
        _ => "String",                           // Default fallback
    }
}

/// Convert snake_case to PascalCase
fn to_pascal_case(s: &str) -> String {
    s.split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_pascal_case() {
        assert_eq!(to_pascal_case("user"), "User");
        assert_eq!(to_pascal_case("user_profile"), "UserProfile");
        assert_eq!(to_pascal_case("blog_post"), "BlogPost");
    }

    #[test]
    fn test_map_column_to_graphql_type() {
        assert_eq!(map_column_to_graphql_type("INTEGER"), "Int");
        assert_eq!(map_column_to_graphql_type("TEXT"), "String");
        assert_eq!(map_column_to_graphql_type("BOOLEAN"), "Boolean");
        assert_eq!(map_column_to_graphql_type("TIMESTAMPTZ"), "String");
    }

    #[test]
    fn test_generate_type_definitions() {
        let schema = TableSchema {
            table_name: "users".to_string(),
            columns: vec![
                ColumnInfo {
                    name: "id".to_string(),
                    data_type: "SERIAL".to_string(),
                    nullable: false,
                    default_value: None,
                    is_primary_key: true,
                },
                ColumnInfo {
                    name: "name".to_string(),
                    data_type: "TEXT".to_string(),
                    nullable: false,
                    default_value: None,
                    is_primary_key: false,
                },
                ColumnInfo {
                    name: "email".to_string(),
                    data_type: "TEXT".to_string(),
                    nullable: true,
                    default_value: None,
                    is_primary_key: false,
                },
            ],
        };

        let types = generate_type_definitions("users", &schema, &[]);

        assert!(types.contains("type Users {"));
        assert!(types.contains("  id: Int!"));
        assert!(types.contains("  name: String!"));
        assert!(types.contains("  email: String"));
        assert!(types.contains("input UsersInput {"));
    }
}
