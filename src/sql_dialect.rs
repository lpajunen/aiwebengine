//! How a backend spells the SQL the engine generates.
//!
//! Scripts never write SQL. Every statement against a script-owned table is
//! built here and in `repository`, from a schema the engine recorded itself,
//! which is what makes a second backend a possibility rather than a rewrite.
//! What stops it being free is the handful of places where the *same*
//! operation is written differently by different databases — an
//! auto-incrementing key, the expression for "now", how a parameter is spelled
//! and typed.
//!
//! Those places live in this trait and nowhere else. Everything a script can
//! observe — which values a column accepts, what comes back out of it, when an
//! operation is refused — is decided in [`crate::db_schema_utils`] against the
//! engine's own types, above any dialect. A backend chooses spelling, never
//! meaning.
//!
//! # Adding a backend
//!
//! Implementing this trait is necessary but not sufficient. A second backend
//! also has to answer, in its own `Repository` implementation:
//!
//! - **Type affinity.** Postgres rejects a value its column cannot hold.
//!   A backend that stores by affinity will not, so the check has to happen
//!   before the statement runs — which is what `bind_value` already does, and
//!   why decoding a row consults the declared type rather than the value that
//!   comes back.
//! - **Concurrency.** Postgres readers do not block writers. A single-writer
//!   backend makes a transaction held across a host call a stall for every
//!   other script, not just for the one holding it.
//! - **`ALTER TABLE`.** `dropColumn` is a rewrite on some backends and
//!   refused outright on columns that are indexed or referenced.
//!
//! None of those are spelling, so none of them belong in this trait. They are
//! the reason a conformance suite run against both backends is what actually
//! proves the two agree.

use crate::db_schema_utils::{BindType, ColumnDefault, ColumnType};

/// The SQL spellings one backend uses for the statements the engine builds.
pub trait SqlDialect: Send + Sync {
    /// Name for logs and diagnostics.
    fn name(&self) -> &'static str;

    /// The column type to declare for one of the engine's types.
    fn column_type(&self, column_type: ColumnType) -> &'static str;

    /// The full `id` column definition for a newly created script table.
    ///
    /// One string rather than a type plus a set of flags: the backends that
    /// differ here differ in more than the type name.
    fn identity_primary_key(&self) -> &'static str;

    /// The expression meaning "the moment this statement runs".
    fn now(&self) -> &'static str;

    /// A default value, written as this backend would write it in DDL.
    fn render_default(&self, default: &ColumnDefault) -> String;

    /// The placeholder for the `position`-th parameter, typed as `bind_type`.
    ///
    /// `position` is 1-based, matching the order values are bound in.
    fn placeholder(&self, position: usize, bind_type: BindType) -> String;

    /// The clause that holds the rows a `SELECT` returned until the
    /// transaction ends, if this backend needs one written.
    ///
    /// `None` is not "no locking". It means the backend's write transactions
    /// already exclude each other, so the rows a reader saw cannot be changed
    /// under it by another writer — a single-writer backend is in that
    /// position, and writing a lock clause there would be noise at best.
    /// A backend that returns `None` while still allowing concurrent writers
    /// would silently reintroduce the lost update this exists to prevent.
    fn row_lock_clause(&self) -> Option<&'static str>;
}

/// Postgres.
pub struct PostgresDialect;

impl PostgresDialect {
    /// Wrap a value in single quotes for use as a literal in DDL.
    ///
    /// Only ever reached with values that have already been parsed into a
    /// [`ColumnDefault`], so this is not the boundary that keeps a script's
    /// input out of the SQL — it is the last step after it.
    fn quote_literal(value: &str) -> String {
        format!("'{}'", value.replace('\'', "''"))
    }
}

impl SqlDialect for PostgresDialect {
    fn name(&self) -> &'static str {
        "postgres"
    }

    fn column_type(&self, column_type: ColumnType) -> &'static str {
        match column_type {
            ColumnType::Integer => "INTEGER",
            ColumnType::Bigint => "BIGINT",
            // Not `NUMERIC`: an `f64` round-trips through a JS number exactly,
            // where `NUMERIC`'s precision would be lost at the boundary
            // anyway. Money belongs in `Integer` minor units.
            ColumnType::Float => "DOUBLE PRECISION",
            ColumnType::Text => "TEXT",
            ColumnType::Boolean => "BOOLEAN",
            ColumnType::Timestamp => "TIMESTAMPTZ",
        }
    }

    fn identity_primary_key(&self) -> &'static str {
        "id SERIAL PRIMARY KEY"
    }

    fn now(&self) -> &'static str {
        "NOW()"
    }

    fn render_default(&self, default: &ColumnDefault) -> String {
        match default {
            ColumnDefault::Integer(n) => n.to_string(),
            ColumnDefault::Float(f) => f.to_string(),
            ColumnDefault::Text(s) => Self::quote_literal(s),
            ColumnDefault::Boolean(b) => b.to_string(),
            ColumnDefault::Now => self.now().to_string(),
            ColumnDefault::Timestamp(instant) => Self::quote_literal(&instant.to_rfc3339()),
        }
    }

    fn placeholder(&self, position: usize, bind_type: BindType) -> String {
        // The cast is not decoration. It stops Postgres inferring the
        // parameter's type from the surrounding statement — inference is what
        // quietly rounded `1.57` to `2` through a `float8 → int4` assignment
        // cast — and it puts the type into the statement text, which is the
        // key sqlx caches a prepared statement under. A column whose declared
        // type is unknown, and whose bind type therefore had to be guessed
        // from the value, still cannot collide with a differently typed guess.
        let cast = match bind_type {
            BindType::Int4 => "int4",
            BindType::Int8 => "int8",
            BindType::Float8 => "float8",
            BindType::Text => "text",
            BindType::Bool => "bool",
            BindType::Timestamptz => "timestamptz",
        };
        format!("${}::{}", position, cast)
    }

    fn row_lock_clause(&self) -> Option<&'static str> {
        // Postgres runs scripts at READ COMMITTED, where a plain SELECT takes
        // no lock at all: two transactions can read one row, both compute from
        // what they read, and both commit, with the second write silently
        // replacing the first. `FOR UPDATE` is what makes the second reader
        // wait for the first to commit and then see what it wrote.
        Some("FOR UPDATE")
    }
}

/// The dialect the engine is generating SQL for.
///
/// The single place a second backend gets switched in.
pub fn dialect() -> &'static dyn SqlDialect {
    &PostgresDialect
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_default_is_rendered_from_its_value_not_from_the_script_text() {
        let d = dialect();

        assert_eq!(d.render_default(&ColumnDefault::Integer(42)), "42");
        assert_eq!(d.render_default(&ColumnDefault::Boolean(true)), "true");
        assert_eq!(d.render_default(&ColumnDefault::Boolean(false)), "false");
        assert_eq!(d.render_default(&ColumnDefault::Now), "NOW()");
        assert_eq!(
            d.render_default(&ColumnDefault::Text("hello".to_string())),
            "'hello'"
        );
    }

    #[test]
    fn an_apostrophe_in_a_text_default_is_escaped() {
        assert_eq!(
            dialect().render_default(&ColumnDefault::Text("it's".to_string())),
            "'it''s'"
        );
    }

    #[test]
    fn a_timestamp_default_is_written_as_an_unambiguous_instant() {
        let instant = chrono::DateTime::parse_from_rfc3339("2024-03-01T12:30:00Z")
            .expect("valid test timestamp")
            .with_timezone(&chrono::Utc);

        assert_eq!(
            dialect().render_default(&ColumnDefault::Timestamp(instant)),
            "'2024-03-01T12:30:00+00:00'"
        );
    }

    #[test]
    fn a_placeholder_carries_the_type_it_is_bound_as() {
        let d = dialect();

        assert_eq!(d.placeholder(1, BindType::Int4), "$1::int4");
        assert_eq!(d.placeholder(2, BindType::Float8), "$2::float8");
        assert_eq!(d.placeholder(3, BindType::Timestamptz), "$3::timestamptz");
    }

    #[test]
    fn postgres_needs_a_lock_clause_written_out() {
        // READ COMMITTED gives a plain SELECT no lock, so the clause is the
        // only thing standing between a read-modify-write and a lost update.
        assert_eq!(dialect().row_lock_clause(), Some("FOR UPDATE"));
    }

    #[test]
    fn every_engine_type_has_a_column_type() {
        let d = dialect();

        assert_eq!(d.column_type(ColumnType::Integer), "INTEGER");
        assert_eq!(d.column_type(ColumnType::Bigint), "BIGINT");
        assert_eq!(d.column_type(ColumnType::Float), "DOUBLE PRECISION");
        assert_eq!(d.column_type(ColumnType::Text), "TEXT");
        assert_eq!(d.column_type(ColumnType::Boolean), "BOOLEAN");
        assert_eq!(d.column_type(ColumnType::Timestamp), "TIMESTAMPTZ");
    }
}
