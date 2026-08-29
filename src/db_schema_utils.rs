use regex::Regex;
use sha2::{Digest, Sha256};
use std::str::FromStr;
use std::sync::OnceLock;

/// Maximum number of tables per script
pub const MAX_TABLES_PER_SCRIPT: usize = 50;

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColumnType {
    Integer,
    /// For whole numbers past `INTEGER`'s ~2.1 billion — epoch milliseconds
    /// being the one every script reaches for. JavaScript integers are exact
    /// to 2^53, so `int8` is the width that matches the language.
    Bigint,
    /// JavaScript has one numeric type and this is it. `DOUBLE PRECISION`
    /// rather than `NUMERIC`: an `f64` round-trips through a JS number
    /// exactly, where `NUMERIC`'s precision would be lost at the boundary
    /// anyway. Money belongs in `Integer` minor units, not here.
    Float,
    Text,
    Boolean,
    Timestamp,
}

impl ColumnType {
    /// How this type is recorded in `script_tables.schema_json`.
    ///
    /// The engine's own name for the type, not any backend's spelling of it.
    /// What is stored has to outlive the backend that stored it: a column
    /// recorded as `DOUBLE PRECISION` names a Postgres type, and reading that
    /// back on a backend with no such type means guessing. The names here are
    /// the ones [`ColumnType::from_str`] parses, so the round trip is exact.
    pub fn canonical(&self) -> &'static str {
        match self {
            ColumnType::Integer => "integer",
            ColumnType::Bigint => "bigint",
            ColumnType::Float => "float",
            ColumnType::Text => "text",
            ColumnType::Boolean => "boolean",
            ColumnType::Timestamp => "timestamp",
        }
    }

    /// The value type a column of this type holds.
    pub fn bind_type(&self) -> BindType {
        match self {
            ColumnType::Integer => BindType::Int4,
            ColumnType::Bigint => BindType::Int8,
            ColumnType::Float => BindType::Float8,
            ColumnType::Text => BindType::Text,
            ColumnType::Boolean => BindType::Bool,
            ColumnType::Timestamp => BindType::Timestamptz,
        }
    }

    /// Read a script-supplied default into a value this type can hold.
    ///
    /// The result is a value, not a fragment of SQL. What a backend has to
    /// write to mean "this value" is that backend's business — see
    /// `SqlDialect::render_default`.
    pub fn parse_default(&self, raw: &str) -> Result<ColumnDefault, SchemaError> {
        let rejected = || SchemaError::InvalidDefaultValue {
            column_type: self.canonical().to_string(),
            value: raw.to_string(),
        };

        match self {
            ColumnType::Integer => {
                let n: i64 = raw.parse().map_err(|_| rejected())?;
                let narrowed = i32::try_from(n).map_err(|_| rejected())?;
                Ok(ColumnDefault::Integer(i64::from(narrowed)))
            }
            ColumnType::Bigint => Ok(ColumnDefault::Integer(raw.parse().map_err(|_| rejected())?)),
            ColumnType::Float => {
                // Postgres accepts NaN and Infinity here; JSON cannot express
                // either, so a column defaulted to one would read back as null.
                match raw.parse::<f64>() {
                    Ok(f) if f.is_finite() => Ok(ColumnDefault::Float(f)),
                    _ => Err(rejected()),
                }
            }
            ColumnType::Text => Ok(ColumnDefault::Text(raw.to_string())),
            ColumnType::Boolean => match raw.to_lowercase().as_str() {
                "true" | "t" | "yes" | "y" | "1" => Ok(ColumnDefault::Boolean(true)),
                "false" | "f" | "no" | "n" | "0" => Ok(ColumnDefault::Boolean(false)),
                _ => Err(rejected()),
            },
            ColumnType::Timestamp => {
                // The one default that is an instruction rather than a value:
                // "whenever the row is written". Every backend spells it
                // differently, so it is carried as intent and rendered later.
                let normalized = raw.trim().to_uppercase();
                if normalized == "NOW()" || normalized == "CURRENT_TIMESTAMP" {
                    return Ok(ColumnDefault::Now);
                }
                parse_instant(raw)
                    .map(ColumnDefault::Timestamp)
                    .ok_or_else(rejected)
            }
        }
    }
}

/// A column default, held as the value it means rather than the SQL that
/// would produce it.
///
/// Defaults used to leave this module as SQL text — `NOW()`, `'quoted'`,
/// `true` — which made every caller that spliced one into a statement a place
/// where a dialect had leaked. Keeping the value lets each backend write it
/// the way that backend spells it, and lets the engine compare two defaults
/// without parsing SQL.
#[derive(Debug, Clone, PartialEq)]
pub enum ColumnDefault {
    /// A whole number, for `Integer` and `Bigint` columns alike.
    Integer(i64),
    Float(f64),
    Text(String),
    Boolean(bool),
    /// The moment the row is written.
    Now,
    /// A fixed instant.
    Timestamp(chrono::DateTime<chrono::Utc>),
}

/// Read a fixed instant out of a script-supplied default.
///
/// Deliberately a short list of unambiguous spellings rather than whatever a
/// particular backend's timestamp parser happens to take. A default is stored
/// once and read back by whichever engine holds the table later, so the set of
/// strings that mean a time has to be the engine's, not one database's. A
/// value without a zone is read as UTC, which is the zone every instant the
/// engine hands a script is already in.
fn parse_instant(raw: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    use chrono::{NaiveDate, NaiveDateTime, TimeZone, Utc};

    let raw = raw.trim();

    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Some(dt.with_timezone(&Utc));
    }

    for format in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%d %H:%M:%S"] {
        if let Ok(naive) = NaiveDateTime::parse_from_str(raw, format) {
            return Some(Utc.from_utc_datetime(&naive));
        }
    }

    if let Ok(date) = NaiveDate::parse_from_str(raw, "%Y-%m-%d") {
        return Some(Utc.from_utc_datetime(&date.and_hms_opt(0, 0, 0)?));
    }

    None
}

/// The value type a parameter is bound as.
///
/// Picking this from the shape of the JSON that carried the value — an `i64`
/// for `2`, an `f64` for `1.57` — is what let one SQL string arrive with
/// different parameter types on different calls. sqlx caches a prepared
/// statement under that string alone: `get_or_prepare` returns the cached
/// entry before it looks at the argument types, so the types inferred by the
/// first call are the types every later call binds against, for as long as
/// that pooled connection lives. Bind ships the encoded bytes unchecked, so a
/// float sent to a parameter prepared as `int8` is not rejected — it is
/// reinterpreted, bit for bit, and `1.57` arrives as 4609081767789723156.
///
/// Deciding the type from the column instead makes it a function of the
/// column names, which are already in the SQL text — the thing the cache is
/// keyed on. The same statement can then only ever be bound the same way.
///
/// It is also what a row is decoded *back* to. Reading a value by trying each
/// Rust type in turn works only where the wire format carries the column's
/// type; on a backend that stores a boolean as 0 or 1, the integer attempt
/// succeeds first and `true` comes back as `1`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BindType {
    Int4,
    Int8,
    Float8,
    Text,
    Bool,
    Timestamptz,
}

impl BindType {
    /// How the type is named back to the script, in the words it declared it with.
    pub fn describe(self) -> &'static str {
        match self {
            BindType::Int4 => "INTEGER",
            BindType::Int8 => "BIGINT",
            BindType::Float8 => "FLOAT",
            BindType::Text => "TEXT",
            BindType::Bool => "BOOLEAN",
            BindType::Timestamptz => "TIMESTAMP",
        }
    }

    /// Resolve a column type recorded in `script_tables.schema_json`.
    ///
    /// Accepts both the canonical names written today and the SQL type names
    /// written before defaults and types were separated from any one dialect,
    /// so a table created by an older engine still reads back typed.
    ///
    /// Returns `None` for anything unrecognised, which leaves the value's own
    /// shape to decide — see [`BindType::infer`].
    pub fn from_declared(declared: &str) -> Option<Self> {
        match declared.to_uppercase().as_str() {
            "INTEGER" | "INT" | "INT4" | "SERIAL" => Some(BindType::Int4),
            "BIGINT" | "INT8" | "BIGSERIAL" | "LONG" => Some(BindType::Int8),
            "DOUBLE PRECISION" | "FLOAT8" | "FLOAT" | "REAL" | "DOUBLE" => Some(BindType::Float8),
            "TEXT" | "STRING" | "VARCHAR" => Some(BindType::Text),
            "BOOLEAN" | "BOOL" => Some(BindType::Bool),
            "TIMESTAMPTZ" | "TIMESTAMP" => Some(BindType::Timestamptz),
            _ => None,
        }
    }

    /// The type to bind a value as when the column's own type is unknown.
    ///
    /// Tables predating the schema metadata — and lease tables, which record
    /// no columns — have nothing to look the column up in. The value's shape
    /// is all that is left, which is the old behaviour; what keeps it safe is
    /// that the guess is pinned by the cast the dialect writes, so two
    /// different guesses land on two different cached statements.
    pub fn infer(value: &serde_json::Value) -> Self {
        match value {
            serde_json::Value::Number(n) if n.as_i64().is_some() => BindType::Int8,
            serde_json::Value::Number(_) => BindType::Float8,
            serde_json::Value::Bool(_) => BindType::Bool,
            _ => BindType::Text,
        }
    }
}

impl FromStr for ColumnType {
    type Err = SchemaError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "integer" | "int" | "int4" => Ok(ColumnType::Integer),
            "bigint" | "int8" | "long" => Ok(ColumnType::Bigint),
            "float" | "double" | "real" | "float8" | "double precision" => Ok(ColumnType::Float),
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
    let hash_hex = hex::encode(hash_result);
    let hash_prefix = &hash_hex[..8]; // First 8 characters

    format!("script_{}_{}", hash_prefix, logical_name)
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
    fn an_integer_default_is_read_as_a_number() {
        assert_eq!(
            ColumnType::Integer.parse_default("42").unwrap(),
            ColumnDefault::Integer(42)
        );
        assert_eq!(
            ColumnType::Integer.parse_default("-100").unwrap(),
            ColumnDefault::Integer(-100)
        );
        assert!(ColumnType::Integer.parse_default("not_a_number").is_err());
    }

    #[test]
    fn an_integer_default_too_wide_for_the_column_is_refused_here() {
        // The column is int4. Letting this through only moves the failure to
        // the ALTER TABLE, where it arrives as a database error rather than as
        // a validation error naming the value.
        assert!(ColumnType::Integer.parse_default("5000000000").is_err());
        assert_eq!(
            ColumnType::Bigint.parse_default("5000000000").unwrap(),
            ColumnDefault::Integer(5_000_000_000)
        );
    }

    #[test]
    fn a_bigint_default_is_a_whole_number() {
        assert_eq!(
            ColumnType::Bigint.parse_default("1700000000000").unwrap(),
            ColumnDefault::Integer(1_700_000_000_000)
        );
        assert!(ColumnType::Bigint.parse_default("1.5").is_err());
    }

    #[test]
    fn a_float_default_must_be_expressible_as_json() {
        assert_eq!(
            ColumnType::Float.parse_default("1.57").unwrap(),
            ColumnDefault::Float(1.57)
        );

        // Postgres would take these; JSON cannot express either, so a column
        // defaulted to one would read back as null.
        assert!(ColumnType::Float.parse_default("NaN").is_err());
        assert!(ColumnType::Float.parse_default("Infinity").is_err());
        assert!(ColumnType::Float.parse_default("not_a_number").is_err());
    }

    #[test]
    fn a_text_default_is_carried_unquoted() {
        // The quoting is the backend's business now. What is kept is the value
        // the script asked for, apostrophe and all.
        assert_eq!(
            ColumnType::Text.parse_default("it's").unwrap(),
            ColumnDefault::Text("it's".to_string())
        );
    }

    #[test]
    fn a_boolean_default_takes_the_spellings_a_script_uses() {
        for raw in ["true", "t", "yes", "y", "1"] {
            assert_eq!(
                ColumnType::Boolean.parse_default(raw).unwrap(),
                ColumnDefault::Boolean(true),
                "{raw}"
            );
        }
        for raw in ["false", "f", "no", "n", "0"] {
            assert_eq!(
                ColumnType::Boolean.parse_default(raw).unwrap(),
                ColumnDefault::Boolean(false),
                "{raw}"
            );
        }
        assert!(ColumnType::Boolean.parse_default("maybe").is_err());
    }

    #[test]
    fn now_is_kept_as_intent_rather_than_as_a_time() {
        // Both spellings mean the same thing and neither is stored as SQL:
        // what a backend writes for "when the row is written" is its own.
        assert_eq!(
            ColumnType::Timestamp.parse_default("NOW()").unwrap(),
            ColumnDefault::Now
        );
        assert_eq!(
            ColumnType::Timestamp.parse_default("now()").unwrap(),
            ColumnDefault::Now
        );
        assert_eq!(
            ColumnType::Timestamp
                .parse_default("CURRENT_TIMESTAMP")
                .unwrap(),
            ColumnDefault::Now
        );
    }

    #[test]
    fn a_fixed_timestamp_default_is_parsed_not_forwarded() {
        let expected = chrono::DateTime::parse_from_rfc3339("2024-03-01T12:30:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        for raw in [
            "2024-03-01T12:30:00Z",
            "2024-03-01T12:30:00+00:00",
            "2024-03-01T12:30:00",
            "2024-03-01 12:30:00",
        ] {
            assert_eq!(
                ColumnType::Timestamp.parse_default(raw).unwrap(),
                ColumnDefault::Timestamp(expected),
                "{raw}"
            );
        }

        // A zone-less value is read as UTC, not as the server's local time.
        assert_eq!(
            ColumnType::Timestamp.parse_default("2024-03-01").unwrap(),
            ColumnDefault::Timestamp(
                chrono::DateTime::parse_from_rfc3339("2024-03-01T00:00:00Z")
                    .unwrap()
                    .with_timezone(&chrono::Utc)
            )
        );

        // Previously anything at all was forwarded for the database to judge.
        assert!(ColumnType::Timestamp.parse_default("whenever").is_err());
    }

    #[test]
    fn a_column_type_is_named_the_way_a_script_would_name_it() {
        use std::str::FromStr;
        assert_eq!(ColumnType::from_str("bigint").unwrap(), ColumnType::Bigint);
        assert_eq!(ColumnType::from_str("LONG").unwrap(), ColumnType::Bigint);
        assert_eq!(ColumnType::from_str("float").unwrap(), ColumnType::Float);
        assert_eq!(ColumnType::from_str("double").unwrap(), ColumnType::Float);
        assert_eq!(ColumnType::Float.canonical(), "float");
        assert_eq!(ColumnType::Bigint.canonical(), "bigint");
    }

    #[test]
    fn a_declared_type_survives_the_round_trip_through_metadata() {
        // What `canonical()` writes is what `from_str` reads, for every type.
        for ty in [
            ColumnType::Integer,
            ColumnType::Bigint,
            ColumnType::Float,
            ColumnType::Text,
            ColumnType::Boolean,
            ColumnType::Timestamp,
        ] {
            assert_eq!(ColumnType::from_str(ty.canonical()).unwrap(), ty);
            assert_eq!(
                BindType::from_declared(ty.canonical()),
                Some(ty.bind_type()),
                "{}",
                ty.canonical()
            );
        }
    }

    #[test]
    fn the_sql_type_names_written_by_older_engines_still_read_back() {
        // Tables created before the type names were made the engine's own
        // carry Postgres spellings in their metadata. Losing them would leave
        // those columns untyped, and an untyped column is one decoded by
        // guessing.
        assert_eq!(
            BindType::from_declared("DOUBLE PRECISION"),
            Some(BindType::Float8)
        );
        assert_eq!(
            BindType::from_declared("TIMESTAMPTZ"),
            Some(BindType::Timestamptz)
        );
        assert_eq!(BindType::from_declared("SERIAL"), Some(BindType::Int4));
        assert_eq!(BindType::from_declared("BIGSERIAL"), Some(BindType::Int8));
        assert_eq!(BindType::from_declared("nonsense"), None);
    }

    #[test]
    fn test_quote_identifier() {
        assert_eq!(quote_identifier("users"), "\"users\"");
        assert_eq!(quote_identifier("user\"name"), "\"user\"\"name\"");
    }

    #[test]
    fn test_column_type_conversions() {
        assert_eq!(
            ColumnType::from_str("integer").unwrap(),
            ColumnType::Integer
        );
        assert_eq!(ColumnType::from_str("text").unwrap(), ColumnType::Text);
        assert!(ColumnType::from_str("invalid").is_err());
    }
}
