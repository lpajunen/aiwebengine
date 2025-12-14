use regex::Regex;
use sha2::{Digest, Sha256};
use std::sync::OnceLock;

/// Maximum number of tables per script
pub const MAX_TABLES_PER_SCRIPT: usize = 10;

/// Maximum number of columns per table
pub const MAX_COLUMNS_PER_TABLE: usize = 50;

/// Maximum length for table and column names
pub const MAX_IDENTIFIER_LENGTH: usize = 63; // PostgreSQL limit

/// Error types for database schema operations
#[derive(Debug, thiserror::Error)]
pub enum SchemaError {
    #[error("Invalid identifier '{0}': must match pattern ^[a-z][a-z0-9_]*$")]
    InvalidIdentifier(String),

    #[error("Identifier too long: {0} exceeds {1} characters")]
    IdentifierTooLong(usize, usize),

    #[error("Script has reached maximum table limit of {0}")]
    MaxTablesExceeded(usize),

    #[error("Table has reached maximum column limit of {0}")]
    MaxColumnsExceeded(usize),

    #[error("Invalid column type: {0}")]
    InvalidColumnType(String),

    #[error("Invalid default value for type {column_type}: {value}")]
    InvalidDefaultValue { column_type: String, value: String },

    #[error("Table '{0}' already exists for this script")]
    TableAlreadyExists(String),

    #[error("Table '{0}' not found for this script")]
    TableNotFound(String),

    #[error("Column '{0}' already exists in table '{1}'")]
    ColumnAlreadyExists(String, String),

    #[error("Referenced table '{0}' not found")]
    ReferencedTableNotFound(String),
}

/// Supported column types for script-created tables
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnType {
    Integer,
    Text,
    Boolean,
    Timestamp,
}

impl ColumnType {
    pub fn to_sql(&self) -> &'static str {
        match self {
            ColumnType::Integer => "INTEGER",
            ColumnType::Text => "TEXT",
            ColumnType::Boolean => "BOOLEAN",
            ColumnType::Timestamp => "TIMESTAMPTZ",
        }
    }

    pub fn from_str(s: &str) -> Result<Self, SchemaError> {
        match s.to_lowercase().as_str() {
            "integer" | "int" => Ok(ColumnType::Integer),
            "text" | "string" => Ok(ColumnType::Text),
            "boolean" | "bool" => Ok(ColumnType::Boolean),
            "timestamp" | "timestamptz" => Ok(ColumnType::Timestamp),
            _ => Err(SchemaError::InvalidColumnType(s.to_string())),
        }
    }
}

/// Validates a SQL identifier (table or column name)
/// Must match: ^[a-z][a-z0-9_]*$ and be <= 63 characters
pub fn validate_identifier(name: &str) -> Result<(), SchemaError> {
    static IDENTIFIER_REGEX: OnceLock<Regex> = OnceLock::new();
    let regex = IDENTIFIER_REGEX
        .get_or_init(|| Regex::new(r"^[a-z][a-z0-9_]*$").expect("Valid identifier regex"));

    if name.len() > MAX_IDENTIFIER_LENGTH {
        return Err(SchemaError::IdentifierTooLong(
            name.len(),
            MAX_IDENTIFIER_LENGTH,
        ));
    }

    if !regex.is_match(name) {
        return Err(SchemaError::InvalidIdentifier(name.to_string()));
    }

    // Additional check: prevent reserved SQL keywords
    let reserved_keywords = [
        "select",
        "insert",
        "update",
        "delete",
        "drop",
        "create",
        "alter",
        "table",
        "index",
        "view",
        "database",
        "schema",
        "user",
        "role",
        "grant",
        "revoke",
        "where",
        "from",
        "join",
        "on",
        "as",
        "and",
        "or",
        "not",
        "null",
        "true",
        "false",
        "default",
        "primary",
        "foreign",
        "key",
        "references",
        "constraint",
        "unique",
        "check",
        "cascade",
    ];

    if reserved_keywords.contains(&name.to_lowercase().as_str()) {
        return Err(SchemaError::InvalidIdentifier(format!(
            "{} is a reserved keyword",
            name
        )));
    }

    Ok(())
}

/// Generates a physical table name from script URI and logical table name
/// Format: script_{hash}_{table_name}
/// The hash is the first 8 characters of SHA256(script_uri)
pub fn generate_physical_table_name(script_uri: &str, logical_name: &str) -> String {
    // Generate hash from script URI
    let mut hasher = Sha256::new();
    hasher.update(script_uri.as_bytes());
    let hash_result = hasher.finalize();
    let hash_hex = format!("{:x}", hash_result);
    let hash_prefix = &hash_hex[..8]; // First 8 characters

    format!("script_{}_{}", hash_prefix, logical_name)
}

/// Validates a default value for a given column type
pub fn validate_default_value(
    column_type: &ColumnType,
    default_value: &str,
) -> Result<String, SchemaError> {
    match column_type {
        ColumnType::Integer => {
            // Validate it's a valid integer
            default_value
                .parse::<i64>()
                .map_err(|_| SchemaError::InvalidDefaultValue {
                    column_type: "INTEGER".to_string(),
                    value: default_value.to_string(),
                })?;
            Ok(default_value.to_string())
        }
        ColumnType::Text => {
            // Text values need to be quoted
            Ok(format!("'{}'", default_value.replace('\'', "''")))
        }
        ColumnType::Boolean => {
            // Validate it's a valid boolean
            match default_value.to_lowercase().as_str() {
                "true" | "t" | "yes" | "y" | "1" => Ok("true".to_string()),
                "false" | "f" | "no" | "n" | "0" => Ok("false".to_string()),
                _ => Err(SchemaError::InvalidDefaultValue {
                    column_type: "BOOLEAN".to_string(),
                    value: default_value.to_string(),
                }),
            }
        }
        ColumnType::Timestamp => {
            // For timestamps, support NOW() or specific timestamps
            if default_value.to_uppercase() == "NOW()" {
                Ok("NOW()".to_string())
            } else {
                // Try to parse as a timestamp
                // For now, we'll just accept it and let PostgreSQL validate
                // TODO: Add proper timestamp validation
                Ok(format!("'{}'", default_value.replace('\'', "''")))
            }
        }
    }
}

/// Escapes a SQL identifier by wrapping it in double quotes
/// This allows identifiers that might conflict with keywords
pub fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_identifier_valid() {
        assert!(validate_identifier("users").is_ok());
        assert!(validate_identifier("user_profiles").is_ok());
        assert!(validate_identifier("data123").is_ok());
        assert!(validate_identifier("a").is_ok());
    }

    #[test]
    fn test_validate_identifier_invalid() {
        // Must start with lowercase letter
        assert!(validate_identifier("Users").is_err());
        assert!(validate_identifier("1users").is_err());
        assert!(validate_identifier("_users").is_err());

        // No special characters except underscore
        assert!(validate_identifier("user-profiles").is_err());
        assert!(validate_identifier("user.profiles").is_err());
        assert!(validate_identifier("user profiles").is_err());

        // Reserved keywords
        assert!(validate_identifier("select").is_err());
        assert!(validate_identifier("table").is_err());
        assert!(validate_identifier("user").is_err());
    }

    #[test]
    fn test_validate_identifier_too_long() {
        let long_name = "a".repeat(64);
        assert!(matches!(
            validate_identifier(&long_name),
            Err(SchemaError::IdentifierTooLong(64, 63))
        ));
    }

    #[test]
    fn test_generate_physical_table_name() {
        let script_uri = "https://example.com/myscript";
        let logical_name = "users";

        let physical = generate_physical_table_name(script_uri, logical_name);

        // Should start with script_
        assert!(physical.starts_with("script_"));

        // Should contain the logical name
        assert!(physical.ends_with("_users"));

        // Should be deterministic
        let physical2 = generate_physical_table_name(script_uri, logical_name);
        assert_eq!(physical, physical2);

        // Different script URIs should generate different names
        let physical3 = generate_physical_table_name("https://example.com/other", logical_name);
        assert_ne!(physical, physical3);
    }

    #[test]
    fn test_validate_default_value_integer() {
        assert!(validate_default_value(&ColumnType::Integer, "42").is_ok());
        assert!(validate_default_value(&ColumnType::Integer, "-100").is_ok());
        assert!(validate_default_value(&ColumnType::Integer, "0").is_ok());
        assert!(validate_default_value(&ColumnType::Integer, "not_a_number").is_err());
    }

    #[test]
    fn test_validate_default_value_text() {
        let result = validate_default_value(&ColumnType::Text, "hello");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "'hello'");

        // Test SQL injection prevention
        let result = validate_default_value(&ColumnType::Text, "it's");
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "'it''s'");
    }

    #[test]
    fn test_validate_default_value_boolean() {
        assert_eq!(
            validate_default_value(&ColumnType::Boolean, "true").unwrap(),
            "true"
        );
        assert_eq!(
            validate_default_value(&ColumnType::Boolean, "false").unwrap(),
            "false"
        );
        assert_eq!(
            validate_default_value(&ColumnType::Boolean, "1").unwrap(),
            "true"
        );
        assert_eq!(
            validate_default_value(&ColumnType::Boolean, "0").unwrap(),
            "false"
        );
        assert!(validate_default_value(&ColumnType::Boolean, "maybe").is_err());
    }

    #[test]
    fn test_validate_default_value_timestamp() {
        assert_eq!(
            validate_default_value(&ColumnType::Timestamp, "NOW()").unwrap(),
            "NOW()"
        );
        assert_eq!(
            validate_default_value(&ColumnType::Timestamp, "now()").unwrap(),
            "NOW()"
        );
    }

    #[test]
    fn test_quote_identifier() {
        assert_eq!(quote_identifier("users"), "\"users\"");
        assert_eq!(quote_identifier("user\"name"), "\"user\"\"name\"");
    }

    #[test]
    fn test_column_type_conversions() {
        assert_eq!(ColumnType::Integer.to_sql(), "INTEGER");
        assert_eq!(ColumnType::Text.to_sql(), "TEXT");
        assert_eq!(ColumnType::Boolean.to_sql(), "BOOLEAN");
        assert_eq!(ColumnType::Timestamp.to_sql(), "TIMESTAMPTZ");

        assert_eq!(
            ColumnType::from_str("integer").unwrap(),
            ColumnType::Integer
        );
        assert_eq!(ColumnType::from_str("text").unwrap(), ColumnType::Text);
        assert!(ColumnType::from_str("invalid").is_err());
    }
}
