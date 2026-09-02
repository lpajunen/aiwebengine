# Database Schema Management API

This document describes the new database schema management API that allows JavaScript scripts to create and manage their own database tables.

## Overview

Scripts can now dynamically create database tables, add columns of various types, create foreign key relationships, and drop tables when needed. All tables are automatically namespaced per script and cleaned up when the script is deleted.

## Security

- Schema operations (`createTable`, `ensureTable`, `add*Column`, `dropColumn`,
  `dropTable`, `addUniqueIndex`, `createLeaseTable`, `generateGraphQLForTable`)
  require the `ManageScriptDatabase` capability, granted to editors and admins.
  Changing the shape of a solution's data is authoring, not using.
- Row operations (`query`, `insert`, `update`, `delete`, `upsert`, `deleteWhere`,
  `acquireLease`) require `UseScriptDatabase`, which every authenticated user
  holds — a script serving a request runs under the requesting user's context,
  so its ordinary data access has to work for the people using the solution.
- Each script's tables are isolated using a hash-based prefix
- Maximum limits: 50 tables per script, 50 columns per table

## API Reference

### `database.createTable(tableName)`

Creates a new table for the current script with an automatic, auto-incrementing integer `id` primary key.

**Parameters:**

- `tableName` (string): Logical table name (must match `^[a-z][a-z0-9_]*$`)

**Returns:** JSON string with `success` and `tableName` or `error`

**Example:**

```javascript
const result = database.createTable("users");
const data = JSON.parse(result);
if (data.error) {
  log("Error: " + data.error);
} else {
  log("Created table: " + data.tableName);
}
```

### `database.addIntegerColumn(tableName, columnName, nullable, defaultValue)`

Adds an INTEGER column to an existing table.

**Parameters:**

- `tableName` (string): Table name
- `columnName` (string): Column name (must match `^[a-z][a-z0-9_]*$`)
- `nullable` (boolean, optional): Whether column can be NULL (default: true)
- `defaultValue` (string, optional): Default value as string (e.g., "42")

**Returns:** JSON string with `success` or `error`

**Example:**

```javascript
// Add age column, not nullable, default 0
database.addIntegerColumn("users", "age", false, "0");

// Add score column, nullable, no default
database.addIntegerColumn("users", "score", true, null);
```

### `database.addBigintColumn(tableName, columnName, nullable, defaultValue)`

Adds a BIGINT column to an existing table.

Whole numbers past INTEGER's ~2.1 billion limit — epoch milliseconds most of
all, since `Date.now()` is already past 1.7 trillion. JavaScript integers are
exact to 2^53, so anything a script can count with round-trips exactly.

**Parameters:** as `addIntegerColumn`.

**Example:**

```javascript
database.addBigintColumn("events", "occurred_at_ms", false, "0");
database.insert("events", JSON.stringify({ occurred_at_ms: Date.now() }));
```

### `database.addFloatColumn(tableName, columnName, nullable, defaultValue)`

Adds a `float` column to an existing table — a double, which is what a JavaScript number already is.

The column type that holds a JavaScript number as it is — rates, ratios,
scores, measurements. The value round-trips exactly, because the column is a
double and so is a JavaScript number.

**Not for money.** `0.1 + 0.2` is not `0.3` in any double. Store amounts as
whole minor units — cents, not euros — in an INTEGER or BIGINT column.

**Parameters:** as `addIntegerColumn`.

**Example:**

```javascript
database.addFloatColumn("readings", "celsius", true);
database.insert("readings", JSON.stringify({ celsius: 21.5 }));
```

### `database.addTextColumn(tableName, columnName, nullable, defaultValue)`

Adds a TEXT column to an existing table.

**Parameters:**

- `tableName` (string): Table name
- `columnName` (string): Column name
- `nullable` (boolean, optional): Whether column can be NULL (default: true)
- `defaultValue` (string, optional): Default value (will be automatically quoted)

**Returns:** JSON string with `success` or `error`

**Example:**

```javascript
// Add name column with default
database.addTextColumn("users", "name", false, "Anonymous");

// Add description column, nullable
database.addTextColumn("users", "description", true, null);
```

### `database.addBooleanColumn(tableName, columnName, nullable, defaultValue)`

Adds a BOOLEAN column to an existing table.

**Parameters:**

- `tableName` (string): Table name
- `columnName` (string): Column name
- `nullable` (boolean, optional): Whether column can be NULL (default: true)
- `defaultValue` (string, optional): "true", "false", "1", "0", etc.

**Returns:** JSON string with `success` or `error`

**Example:**

```javascript
// Add active flag, default true
database.addBooleanColumn("users", "active", false, "true");

// Add verified flag, nullable
database.addBooleanColumn("users", "verified", true, null);
```

### `database.addTimestampColumn(tableName, columnName, nullable, defaultValue)`

Adds a timestamp column to an existing table.

**Parameters:**

- `tableName` (string): Table name
- `columnName` (string): Column name
- `nullable` (boolean, optional): Whether column can be NULL (default: true)
- `defaultValue` (string, optional): the moment the row is written — `"NOW()"`
  or `"CURRENT_TIMESTAMP"`, which mean the same thing — or a fixed instant as
  `"YYYY-MM-DD"`, `"YYYY-MM-DD HH:MM:SS"`, or ISO 8601. Anything else is
  refused rather than passed to the database to judge

**Returns:** JSON string with `success` or `error`

**Example:**

```javascript
// Add created_at with automatic timestamp
database.addTimestampColumn("users", "created_at", false, "NOW()");

// Add last_login, nullable
database.addTimestampColumn("users", "last_login", true, null);
```

### `database.addReferenceColumn(tableName, columnName, referencedTableName, nullable)`

Adds an INTEGER column with a foreign key constraint to another script-owned table. This is a convenience method that combines adding an INTEGER column and creating a foreign key relationship in one step.

**Parameters:**

- `tableName` (string): Source table name
- `columnName` (string): Column name in source table (will be created as INTEGER)
- `referencedTableName` (string): Target table name (references the `id` column)
- `nullable` (boolean, optional): Whether the column can be NULL (default: true)

**Returns:** JSON string with `success`, `foreignKey` description, and `nullable` flag, or `error`

**Example:**

```javascript
// Create tables
database.createTable("authors");
database.createTable("books");

// Add foreign key column (nullable by default)
const result = database.addReferenceColumn("books", "author_id", "authors");
// Result: {"success": true, "foreignKey": "books.author_id -> authors", "nullable": true}

// Add non-nullable foreign key column
database.addReferenceColumn("posts", "user_id", "users", false);
```

### `database.dropColumn(tableName, columnName)`

Drops a column from a script-owned table. Cannot drop the `id` column.

**Parameters:**

- `tableName` (string): Table name
- `columnName` (string): Column name to drop

**Returns:** JSON string with `success`, `dropped` (boolean indicating if column existed), or `error`

**Example:**

```javascript
const result = database.dropColumn("users", "age");
const data = JSON.parse(result);
if (data.dropped) {
  log("Column was dropped");
} else {
  log("Column did not exist");
}
```

### `database.dropTable(tableName)`

Drops a script-owned table and all its data.

**Parameters:**

- `tableName` (string): Table name to drop

**Returns:** JSON string with `success`, `dropped` (boolean indicating if table existed), or `error`

**Example:**

```javascript
const result = database.dropTable("users");
const data = JSON.parse(result);
if (data.dropped) {
  log("Table was dropped");
} else {
  log("Table did not exist");
}
```

## Table Naming and Isolation

- **Logical names**: What you use in the API (e.g., "users")
- **Physical names**: Actual PostgreSQL table names (e.g., "script_a1b2c3d4_users")
- Tables are prefixed with `script_{hash}_` where hash is derived from the script URI
- Multiple scripts can create tables with the same logical name without conflicts
- Physical table names are tracked in the `script_tables` metadata table

## Automatic Cleanup

When a script is deleted:

1. All tables owned by that script are automatically dropped
2. Metadata entries in `script_tables` are removed via CASCADE
3. Foreign key relationships are cleaned up

When a script is updated:

- Tables are NOT affected
- Schema changes must be done explicitly via the API

## Limits and Constraints

- **Maximum tables per script**: 50
- **Maximum columns per table**: 50
- **Identifier requirements**: Must match `^[a-z][a-z0-9_]*$` (lowercase, alphanumeric + underscore)
- **Reserved keywords**: Cannot use SQL reserved words as identifiers
- **Column types**: Only `integer`, `bigint`, `float`, `text`, `boolean`, and
  `timestamp` are supported. These are the engine's own types, not any one
  database's: what a column accepts and what it gives back is decided by the
  engine, and how it is stored is the storage backend's business
- **An argument that cannot mean what it says is refused**: a sort direction
  that is neither `asc` nor `desc`, and an unrecognised entry in the `options`
  object, are both errors rather than being quietly ignored
- **Values read back as their column's type**: a `boolean` column returns
  `true`/`false`, a `timestamp` returns an ISO 8601 string, a `float` returns
  a number. A column is decoded as the type it was declared with, not as
  whatever the stored bytes happen to look like
- **Values are typed by their column**: a value that the column cannot hold is
  refused, naming the column, rather than being coerced. A fraction in an
  INTEGER column is not rounded — use a FLOAT column to keep it — and a number
  past a column's range is not wrapped

## Complete Example

```javascript
function init() {
  // Create a blog system with authors and posts

  // Authors table
  database.createTable("authors");
  database.addTextColumn("authors", "name", false, "Anonymous");
  database.addTextColumn("authors", "email", false, "unknown@example.com");
  database.addTimestampColumn("authors", "joined_at", false, "NOW()");

  // Posts table
  database.createTable("posts");
  database.addTextColumn("posts", "title", false, "Untitled");
  database.addTextColumn("posts", "content", true, null);
  database.addIntegerColumn("posts", "author_id", false, null);
  database.addBooleanColumn("posts", "published", false, "false");
  database.addTimestampColumn("posts", "created_at", false, "NOW()");
  database.addTimestampColumn("posts", "updated_at", true, null);

  // Create foreign key relationship
  database.createReference("posts", "author_id", "authors");

  log("Blog database schema created successfully");
}
```

## Error Handling

All API functions return JSON strings. Always parse the result and check for errors:

```javascript
const result = database.createTable("users");
const data = JSON.parse(result);

if (data.error) {
  log("Error creating table: " + data.error);
  // Handle error (common errors: permission denied, table exists, limit exceeded)
} else {
  log("Success: " + JSON.stringify(data));
}
```

## Future Enhancements

The current API only supports schema management (DDL operations). Future versions will add:

- Data manipulation: INSERT, SELECT, UPDATE, DELETE operations
- Query builder API for safe SQL generation
- Transaction support
- Bulk operations
- Table introspection (list tables, describe columns)

## Testing

A comprehensive test script is available at `scripts/test_database_schema.js`. To run the tests:

1. Make sure the migration has been applied: `sqlx migrate run`
2. Upload the test script to the system
3. Access the test endpoints:
   - `/test/db/create` - Test table creation
   - `/test/db/columns` - Test column additions
   - `/test/db/references` - Test foreign keys
   - `/test/db/drop` - Test table deletion
   - `/test/db/full` - Run full workflow test
